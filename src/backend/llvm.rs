//! Textual LLVM IR emitter.
//!
//! We emit LLVM 15+ compatible IR (opaque pointers) and shell out to
//! `opt`/`lli` when those tools are present. Linking against libLLVM is
//! deliberately avoided so the compiler stays portable. The bytecode VM is
//! the reference semantics: for every program the frontend accepts, the
//! emitted module prints the same bytes and fails with the same runtime
//! errors (see `docs/llvm.md` for the contract and the documented limits).
//!
//! Aether IR is not SSA: a virtual register may be assigned any number of
//! times. We therefore emit the classic mem2reg-ready form: every register
//! gets one `alloca` in the `entry` block, every use is a `load` into a fresh
//! SSA temporary and every definition is a `store`. `opt -O2` (or just
//! `-passes=mem2reg`) turns this back into proper SSA.
//!
//! Values:
//! * `i32`/`i64` map 1:1, `f64` -> `double`, `bool` -> `i1`, `char` -> `i32`
//!   (a Unicode scalar), `unit` has no storage (`{}` inside aggregates).
//! * `string` -> `ptr` to an immutable `{ i64 len, [len x i8] bytes, NUL }`
//!   block (literals are private constants, results of `+` / `to_string`
//!   are `malloc`ed and never freed). Lengths are explicit, so strings may
//!   contain `\0`.
//! * Arrays, structs, tuples and enums are stored *inline*: an aggregate
//!   register's `alloca` is the value itself, nested aggregates live inside
//!   their parent's storage, and every copy (`Move`, loading an element or
//!   field, storing one, passing an argument, returning) is an
//!   `llvm.memcpy` of the type's size. That gives the VM's value semantics:
//!   no two registers ever share storage.
//! * Enums are `{ i32 tag, slot1, ... }`. A slot that all variants type the
//!   same way keeps that type; a slot that different variants type
//!   differently is a byte buffer (`[n x i64|i32|i8]`) with the maximum size
//!   and alignment of the candidates, accessed through its address with the
//!   matched variant's type.
//! * Aggregate arguments are passed as `ptr` to the caller's storage and
//!   copied into the callee's own register on entry; aggregate results are
//!   written through a leading `ptr` (sret-style) argument.
//!
//! The runtime (string helpers, Rust-`Display` float formatting, runtime
//! errors, the natives of `crate::runtime`) is a fixed block of IR appended
//! to every module ([`RUNTIME_IR`], compiled from the C source in
//! `docs/llvm.md`). Its helpers are `internal` and named `ae.*`, so they
//! never clash with user functions; user functions that would clash with a
//! libc symbol the runtime uses are renamed `<name>.ae`.

use crate::ast::{BinOp, UnOp};
use crate::ir::{ConstValue, Inst, IrFunction, IrModule, Reg, Terminator};
use crate::ty::Type;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::process::{Command, Stdio};

/// Natives provided by the runtime, as `(name, param types, return type)`.
/// `len` also accepts arrays (folded to the constant length).
fn native_sig(name: &str) -> Option<(Vec<Type>, Type)> {
    Some(match name {
        "print" | "println" => (vec![Type::String], Type::Unit),
        "print_i32" => (vec![Type::I32], Type::Unit),
        "print_i64" => (vec![Type::I64], Type::Unit),
        "print_f64" => (vec![Type::F64], Type::Unit),
        "print_bool" | "assert" => (vec![Type::Bool], Type::Unit),
        "len" => (vec![Type::String], Type::I32),
        "print_char" => (vec![Type::Char], Type::Unit),
        "to_string" => (vec![Type::I32], Type::String),
        "i64_to_string" => (vec![Type::I64], Type::String),
        "f64_to_string" => (vec![Type::F64], Type::String),
        "char_to_string" => (vec![Type::Char], Type::String),
        "abs" => (vec![Type::I32], Type::I32),
        "min" | "max" => (vec![Type::I32, Type::I32], Type::I32),
        "clamp" => (vec![Type::I32, Type::I32, Type::I32], Type::I32),
        "sqrt" | "floor" | "ceil" => (vec![Type::F64], Type::F64),
        "pow_i32" => (vec![Type::I32, Type::I32], Type::I32),
        _ => return None,
    })
}

/// Module-wide emission state shared by all functions.
struct ModuleCtx<'a> {
    module: &'a IrModule,
    /// Interned string literals (`@.s.N`), in order of first appearance.
    strings: Vec<String>,
    /// Interned NUL-terminated messages for the runtime (`@.m.N`).
    messages: Vec<String>,
    /// Types that need an equality helper `@ae.eq.N` (index = N).
    eq_types: Vec<Type>,
    /// Symbols the runtime block declares or defines; user functions avoid them.
    reserved: BTreeSet<String>,
}

impl ModuleCtx<'_> {
    fn message(&mut self, s: &str) -> String {
        let i = match self.messages.iter().position(|x| x == s) {
            Some(i) => i,
            None => {
                self.messages.push(s.to_string());
                self.messages.len() - 1
            }
        };
        format!("@.m.{i}")
    }

    fn string(&mut self, s: &str) -> String {
        let i = match self.strings.iter().position(|x| x == s) {
            Some(i) => i,
            None => {
                self.strings.push(s.to_string());
                self.strings.len() - 1
            }
        };
        format!("@.s.{i}")
    }

    /// Name of the structural equality helper for `ty` (generated at the end).
    fn eq_fn(&mut self, ty: &Type) -> String {
        let i = match self.eq_types.iter().position(|t| t == ty) {
            Some(i) => i,
            None => {
                self.eq_types.push(ty.clone());
                self.eq_types.len() - 1
            }
        };
        format!("@ae.eq.{i}")
    }

    /// LLVM symbol (with `@`) of a function of the module.
    fn symbol(&self, f: &IrFunction) -> String {
        if f.name == "main" && !f.is_extern {
            return "@ae.user.main".into();
        }
        let simple = f
            .name
            .chars()
            .next()
            .map_or(false, |c| c.is_ascii_alphabetic() || c == '_')
            && f.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !simple {
            return format!("@\"{}\"", quote_name(&f.name));
        }
        if self.reserved.contains(&f.name) || f.name == "main" {
            format!("@{}.ae", f.name)
        } else {
            format!("@{}", f.name)
        }
    }
}

pub fn emit_llvm_ir(module: &IrModule) -> String {
    let mut cx = ModuleCtx {
        module,
        strings: Vec::new(),
        messages: Vec::new(),
        eq_types: Vec::new(),
        reserved: runtime_symbols(),
    };

    let mut body = String::new();
    for f in &module.functions {
        if f.is_extern {
            emit_extern_decl(&mut body, f, &cx);
        } else {
            emit_function(&mut body, f, &mut cx);
        }
    }
    // Entry point: lli / the C runtime call `i32 main()`.
    if let Some(m) = module.functions.iter().find(|f| f.name == "main" && !f.is_extern) {
        let sym = cx.symbol(m);
        body.push_str("define i32 @main() {\nentry:\n");
        match &m.return_ty {
            Type::I32 => {
                let _ = writeln!(body, "  %v = call i32 {sym}()\n  ret i32 %v");
            }
            t if is_agg(t) => {
                let _ = writeln!(
                    body,
                    "  %v = alloca {}\n  call void {sym}(ptr %v)\n  ret i32 0",
                    mem_ty(t)
                );
            }
            t => {
                let _ = writeln!(body, "  call {} {sym}()\n  ret i32 0", ret_ty(t));
            }
        }
        body.push_str("}\n\n");
    }
    // Equality helpers (generating one may request more).
    let mut done = 0;
    while done < cx.eq_types.len() {
        let ty = cx.eq_types[done].clone();
        emit_eq_fn(&mut body, done, &ty, &mut cx);
        done += 1;
    }

    let mut out = String::new();
    out.push_str("; ModuleID = 'aether'\n");
    out.push_str("source_filename = \"aether\"\n");
    out.push_str("target datalayout = \"e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128\"\n");
    out.push_str("target triple = \"x86_64-pc-linux-gnu\"\n\n");

    // Named struct / enum types, from the module and from every type the
    // emitted code mentions (nested ones included), dependencies first.
    let mut named: BTreeMap<String, Type> = BTreeMap::new();
    for (_, ty) in &module.structs {
        collect_named(ty, &mut named);
    }
    for f in &module.functions {
        for (_, t, _) in &f.params {
            collect_named(t, &mut named);
        }
        collect_named(&f.return_ty, &mut named);
        for bb in &f.blocks {
            for inst in &bb.insts {
                for t in inst_types(inst) {
                    collect_named(t, &mut named);
                }
            }
        }
    }
    for t in &cx.eq_types {
        collect_named(t, &mut named);
    }
    for (name, ty) in &named {
        let parts: Vec<String> = match ty {
            Type::Enum { .. } => enum_slots(ty),
            _ => ty
                .layout_fields()
                .unwrap_or_default()
                .iter()
                .map(mem_ty)
                .collect(),
        };
        let _ = writeln!(out, "{} = type {{ {} }}", named_ty(name), parts.join(", "));
    }
    if !named.is_empty() {
        out.push('\n');
    }

    for (i, s) in cx.strings.iter().enumerate() {
        let n = s.len();
        let _ = writeln!(
            out,
            "@.s.{i} = private unnamed_addr constant <{{ i64, [{} x i8] }}> <{{ i64 {n}, [{} x i8] c\"{}\" }}>, align 8",
            n + 1,
            n + 1,
            llvm_string_bytes(s)
        );
    }
    for (i, s) in cx.messages.iter().enumerate() {
        let _ = writeln!(
            out,
            "@.m.{i} = private unnamed_addr constant [{} x i8] c\"{}\"",
            s.len() + 1,
            llvm_string_bytes(s)
        );
    }
    for decl in [
        "declare i32 @llvm.fptosi.sat.i32.f64(double)",
        "declare i64 @llvm.fptosi.sat.i64.f64(double)",
        "declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)",
    ] {
        let name = decl.split('@').nth(1).and_then(|s| s.split('(').next()).unwrap_or("");
        if !RUNTIME_IR.contains(&format!("@{name}(")) {
            out.push_str(decl);
            out.push('\n');
        }
    }
    out.push('\n');
    out.push_str(&body);
    out.push_str(RUNTIME_IR);
    out
}

/// Symbols the runtime block declares (libc) or defines; `main` included.
fn runtime_symbols() -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    set.insert("main".to_string());
    for line in RUNTIME_IR.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix('@') {
            if let Some(name) = rest.split([' ', '=']).next() {
                set.insert(name.to_string());
            }
        } else if line.starts_with("declare") || line.starts_with("define") {
            if let Some(rest) = line.split('@').nth(1) {
                if let Some(name) = rest.split('(').next() {
                    set.insert(name.to_string());
                }
            }
        }
    }
    set
}

/// `extern fn` without a body: an `extern_weak` declaration. It binds at
/// link time; when nothing provides the symbol it is null and calling it is
/// a runtime error, like the VM's "no implementation" error.
fn emit_extern_decl(out: &mut String, f: &IrFunction, cx: &ModuleCtx) {
    let (ret, params) = signature(f);
    let _ = writeln!(
        out,
        "declare extern_weak {ret} {}({})\n",
        cx.symbol(f),
        params.join(", ")
    );
}

/// LLVM return type and parameter types of a function (aggregate results
/// become a leading `ptr` argument, unit parameters vanish).
fn signature(f: &IrFunction) -> (String, Vec<String>) {
    let mut params = Vec::new();
    if is_agg(&f.return_ty) {
        params.push("ptr".to_string());
    }
    for (_, t, _) in &f.params {
        if !is_unit(t) {
            params.push(slot_ty(t));
        }
    }
    (ret_ty(&f.return_ty), params)
}

/// Every type an instruction carries.
fn inst_types(inst: &Inst) -> Vec<&Type> {
    match inst {
        Inst::Bin { ty, .. } | Inst::Un { ty, .. } => vec![ty],
        Inst::Cast { from, to, .. } => vec![from, to],
        Inst::IndexLoad { elem, .. } | Inst::IndexStore { elem, .. } | Inst::AllocArray { elem, .. } => {
            vec![elem]
        }
        Inst::FieldLoad { ty, .. } | Inst::FieldStore { ty, .. } | Inst::AllocStruct { ty, .. } => vec![ty],
        _ => Vec::new(),
    }
}

fn collect_named(ty: &Type, named: &mut BTreeMap<String, Type>) {
    match ty {
        Type::Array { elem, .. } => collect_named(elem, named),
        Type::Tuple(elems) => {
            for e in elems {
                collect_named(e, named);
            }
        }
        Type::Struct { name, fields } => {
            if !named.contains_key(name) {
                named.insert(name.clone(), ty.clone());
                for (_, t) in fields {
                    collect_named(t, named);
                }
            }
        }
        Type::Enum { name, variants } => {
            if !named.contains_key(name) {
                named.insert(name.clone(), ty.clone());
                for (_, p) in variants {
                    for t in p {
                        collect_named(t, named);
                    }
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Types and layout
// ---------------------------------------------------------------------------

fn is_agg(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Array { .. } | Type::Struct { .. } | Type::Tuple(_) | Type::Enum { .. }
    )
}

fn is_unit(ty: &Type) -> bool {
    matches!(ty, Type::Unit | Type::Error)
}

/// LLVM type of a value of type `ty` *as stored in memory* (array elements,
/// struct fields, register `alloca`s). Aggregates are spelled out inline.
fn mem_ty(ty: &Type) -> String {
    match ty {
        Type::Unit | Type::Error => "{}".into(),
        Type::Bool => "i1".into(),
        Type::I32 | Type::Char => "i32".into(),
        Type::I64 => "i64".into(),
        Type::F64 => "double".into(),
        Type::String | Type::Fn { .. } => "ptr".into(),
        Type::Array { elem, len } => format!("[{} x {}]", (*len).max(0), mem_ty(elem)),
        Type::Struct { name, .. } | Type::Enum { name, .. } => named_ty(name),
        Type::Tuple(elems) => {
            let parts: Vec<String> = elems.iter().map(mem_ty).collect();
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

/// Type of a parameter / argument: aggregates travel as a pointer.
fn slot_ty(ty: &Type) -> String {
    if is_agg(ty) {
        "ptr".into()
    } else {
        mem_ty(ty)
    }
}

/// Return type: aggregates are returned through a pointer argument.
fn ret_ty(ty: &Type) -> String {
    if is_unit(ty) || is_agg(ty) {
        "void".into()
    } else {
        mem_ty(ty)
    }
}

/// Body of an enum's named type: the `i32` tag, then one entry per payload
/// slot (see the module docs).
fn enum_slots(ty: &Type) -> Vec<String> {
    let Type::Enum { variants, .. } = ty else {
        return Vec::new();
    };
    let slots = variants.iter().map(|(_, p)| p.len()).max().unwrap_or(0);
    let mut out = vec!["i32".to_string()];
    for i in 0..slots {
        let cands: Vec<&Type> = variants.iter().filter_map(|(_, p)| p.get(i)).collect();
        if cands.iter().all(|t| *t == cands[0]) {
            out.push(mem_ty(cands[0]));
            continue;
        }
        let size = cands.iter().map(|t| layout(t).0).max().unwrap_or(0);
        let align = cands.iter().map(|t| layout(t).1).max().unwrap_or(1);
        let unit = match align {
            8 => "i64",
            4 => "i32",
            _ => "i8",
        };
        let align = match unit {
            "i64" => 8,
            "i32" => 4,
            _ => 1,
        };
        out.push(format!("[{} x {unit}]", size.div_ceil(align)));
    }
    out
}

/// `(alloc size, ABI alignment)` of `mem_ty(ty)` under the x86-64 data layout.
fn layout(ty: &Type) -> (u64, u64) {
    fn record(fields: &[(u64, u64)]) -> (u64, u64) {
        let mut off = 0u64;
        let mut align = 1u64;
        for (s, a) in fields {
            off = off.div_ceil(*a) * *a + s;
            align = align.max(*a);
        }
        (off.div_ceil(align) * align, align)
    }
    match ty {
        Type::Unit | Type::Error => (0, 1),
        Type::Bool => (1, 1),
        Type::I32 | Type::Char => (4, 4),
        Type::I64 | Type::F64 | Type::String | Type::Fn { .. } => (8, 8),
        Type::Array { elem, len } => {
            let (s, a) = layout(elem);
            (s * (*len).max(0) as u64, a)
        }
        Type::Struct { .. } | Type::Tuple(_) => {
            let f: Vec<(u64, u64)> = ty.layout_fields().unwrap_or_default().iter().map(layout).collect();
            record(&f)
        }
        Type::Enum { variants, .. } => {
            let slots = variants.iter().map(|(_, p)| p.len()).max().unwrap_or(0);
            let mut f = vec![(4u64, 4u64)];
            for i in 0..slots {
                let cands: Vec<&Type> = variants.iter().filter_map(|(_, p)| p.get(i)).collect();
                if cands.iter().all(|t| *t == cands[0]) {
                    f.push(layout(cands[0]));
                } else {
                    let size = cands.iter().map(|t| layout(t).0).max().unwrap_or(0);
                    let a = cands.iter().map(|t| layout(t).1).max().unwrap_or(1);
                    let a = if a >= 8 { 8 } else if a >= 4 { 4 } else { 1 };
                    f.push((size.div_ceil(a) * a, a));
                }
            }
            record(&f)
        }
    }
}

fn named_ty(name: &str) -> String {
    format!("%\"T.{}\"", quote_name(name))
}

/// Characters that cannot appear inside a quoted LLVM name are escaped.
fn quote_name(name: &str) -> String {
    let mut s = String::new();
    for b in name.bytes() {
        if b == b'"' || b == b'\\' || !(32..127).contains(&b) {
            let _ = write!(s, "\\{b:02X}");
        } else {
            s.push(b as char);
        }
    }
    s
}

/// Exact IEEE-754 bit pattern; LLVM accepts this for any double, including
/// values whose shortest decimal form would not round-trip.
fn llvm_f64(v: f64) -> String {
    format!("0x{:016X}", v.to_bits())
}

fn llvm_string_bytes(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'\\' => out.push_str("\\5C"),
            b'"' => out.push_str("\\22"),
            0..=31 | 127..=255 => {
                let _ = write!(out, "\\{b:02X}");
            }
            _ => out.push(b as char),
        }
    }
    out.push_str("\\00");
    out
}

/// A zero of the given scalar LLVM type.
fn zero_of(lty: &str) -> &'static str {
    match lty {
        "double" => "0.0",
        "i1" => "false",
        "ptr" => "null",
        _ => "0",
    }
}

// ---------------------------------------------------------------------------
// Register typing
// ---------------------------------------------------------------------------

/// Per-function register information inferred from the IR.
struct RegInfo {
    /// Inferred type per register; `None` when the register is never defined.
    types: Vec<Option<Type>>,
    /// Whether the register appears anywhere in the function.
    referenced: Vec<bool>,
}

impl RegInfo {
    fn ty(&self, r: Reg) -> Option<&Type> {
        self.types.get(r.0 as usize).and_then(|t| t.as_ref())
    }

    /// LLVM type of the register's `alloca`. Untyped registers default to
    /// `i32` so references to them still assemble.
    fn slot(&self, r: Reg) -> String {
        match self.ty(r) {
            Some(t) => mem_ty(t),
            None => "i32".into(),
        }
    }

    fn is_unit(&self, r: Reg) -> bool {
        self.ty(r).map_or(false, is_unit)
    }

    fn is_agg(&self, r: Reg) -> bool {
        self.ty(r).map_or(false, is_agg)
    }
}

fn infer_regs(f: &IrFunction, cx: &ModuleCtx) -> RegInfo {
    let mut max = f.reg_count as usize;
    let mut note = |r: Reg| {
        max = max.max(r.0 as usize + 1);
    };
    for (_, _, r) in &f.params {
        note(*r);
    }
    for bb in &f.blocks {
        for inst in &bb.insts {
            if let Some(d) = inst.dest_reg() {
                note(d);
            }
            for u in inst.uses() {
                note(u);
            }
        }
        match &bb.term {
            Terminator::Branch { cond, .. } => note(*cond),
            Terminator::Return { value: Some(r) } => note(*r),
            _ => {}
        }
    }

    let mut info = RegInfo {
        types: vec![None; max],
        referenced: vec![false; max],
    };
    for (_, ty, r) in &f.params {
        info.types[r.0 as usize] = Some(ty.clone());
        info.referenced[r.0 as usize] = true;
    }
    for bb in &f.blocks {
        for inst in &bb.insts {
            for u in inst.uses() {
                info.referenced[u.0 as usize] = true;
            }
            if let Some(d) = inst.dest_reg() {
                info.referenced[d.0 as usize] = true;
            }
        }
        match &bb.term {
            Terminator::Branch { cond, .. } => info.referenced[cond.0 as usize] = true,
            Terminator::Return { value: Some(r) } => info.referenced[r.0 as usize] = true,
            _ => {}
        }
    }

    // `Move` copies the type of its source, which may be defined later in
    // block order (loops), so iterate to a fixpoint. A `unit` definition
    // (the placeholder of a `match` expression of aggregate type) yields to
    // any real type.
    loop {
        let mut changed = false;
        for bb in &f.blocks {
            for inst in &bb.insts {
                let (dest, ty) = match inst {
                    Inst::LoadConst { dest, value } => (*dest, Some(value.ty())),
                    Inst::Move { dest, src } => (*dest, info.ty(*src).cloned()),
                    Inst::Bin { dest, op, ty, .. } => {
                        let t = if op.is_cmp() { Type::Bool } else { ty.clone() };
                        (*dest, Some(t))
                    }
                    Inst::Un { dest, ty, .. } => (*dest, Some(ty.clone())),
                    Inst::Call { dest: Some(d), func, .. } => (*d, callee_ret(cx, func)),
                    Inst::Cast { dest, to, .. } => (*dest, Some(to.clone())),
                    Inst::IndexLoad { dest, elem, .. } => (*dest, Some(elem.clone())),
                    Inst::FieldLoad { dest, ty, .. } => (*dest, Some(ty.clone())),
                    Inst::AllocArray { dest, elem, len } => (
                        *dest,
                        Some(Type::Array {
                            elem: Box::new(elem.clone()),
                            len: *len,
                        }),
                    ),
                    Inst::AllocStruct { dest, ty } => (*dest, Some(ty.clone())),
                    Inst::Call { dest: None, .. }
                    | Inst::IndexStore { .. }
                    | Inst::FieldStore { .. }
                    | Inst::Yield
                    | Inst::Nop => continue,
                };
                let Some(ty) = ty else { continue };
                let slot = &mut info.types[dest.0 as usize];
                let replace = match slot {
                    None => true,
                    Some(old) => is_unit(old) && !is_unit(&ty),
                };
                if replace {
                    *slot = Some(ty);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    info
}

fn callee_ret(cx: &ModuleCtx, name: &str) -> Option<Type> {
    if let Some(f) = cx.module.function(name) {
        return Some(f.return_ty.clone());
    }
    native_sig(name).map(|(_, ret)| ret)
}

// ---------------------------------------------------------------------------
// Function emission
// ---------------------------------------------------------------------------

/// Per-function emission state.
struct FnCtx<'a, 'm> {
    out: String,
    /// Extra `alloca`s (call temporaries) hoisted into the entry block.
    entry: String,
    regs: RegInfo,
    next_tmp: u32,
    next_label: u32,
    cx: &'a mut ModuleCtx<'m>,
}

impl FnCtx<'_, '_> {
    fn tmp(&mut self) -> String {
        let t = format!("%t{}", self.next_tmp);
        self.next_tmp += 1;
        t
    }

    fn label(&mut self, prefix: &str) -> String {
        let l = format!("{prefix}.{}", self.next_label);
        self.next_label += 1;
        l
    }

    fn line(&mut self, s: &str) {
        self.out.push_str("  ");
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn block(&mut self, label: &str) {
        self.out.push_str(label);
        self.out.push_str(":\n");
    }

    fn reg_ptr(r: Reg) -> String {
        format!("%r{}", r.0)
    }

    /// Fresh storage for a value of type `ty`, allocated once in `entry`.
    fn temp_alloca(&mut self, ty: &Type) -> String {
        let t = self.tmp();
        let _ = writeln!(self.entry, "  {t} = alloca {}", mem_ty(ty));
        t
    }

    /// The value of register `r`: a fresh SSA load for scalars, the
    /// register's own storage for aggregates.
    fn val(&mut self, r: Reg) -> String {
        if self.regs.is_agg(r) {
            return Self::reg_ptr(r);
        }
        if self.regs.is_unit(r) {
            return String::new();
        }
        let t = self.tmp();
        let ty = self.regs.slot(r);
        self.line(&format!("{t} = load {ty}, ptr %r{}", r.0));
        t
    }

    /// Define register `r` from `value` (an SSA scalar of the register's type,
    /// or a pointer to an aggregate of the register's type, which is copied).
    fn set(&mut self, r: Reg, value: &str) {
        let Some(ty) = self.regs.ty(r).cloned() else {
            self.line(&format!("store i32 {value}, ptr %r{}", r.0));
            return;
        };
        if is_unit(&ty) {
            return;
        }
        if is_agg(&ty) {
            self.copy(&Self::reg_ptr(r), value, &ty);
        } else {
            self.line(&format!("store {} {value}, ptr %r{}", mem_ty(&ty), r.0));
        }
    }

    /// Byte copy of a whole value of type `ty` (aggregates are inline, so this
    /// is a deep copy).
    fn copy(&mut self, dst: &str, src: &str, ty: &Type) {
        let (size, _) = layout(ty);
        if dst == src || size == 0 {
            return;
        }
        self.line(&format!(
            "call void @llvm.memcpy.p0.p0.i64(ptr {dst}, ptr {src}, i64 {size}, i1 false)"
        ));
    }

    /// Store `v` (SSA scalar or pointer to an aggregate) of type `ty` at `p`.
    fn store_at(&mut self, p: &str, v: &str, ty: &Type) {
        if is_unit(ty) {
            return;
        }
        if is_agg(ty) {
            self.copy(p, v, ty);
        } else {
            self.line(&format!("store {} {v}, ptr {p}", mem_ty(ty)));
        }
    }

    /// Load the value of type `ty` at `p` into register `dest`.
    fn load_at(&mut self, p: &str, dest: Reg, ty: &Type) {
        if is_unit(ty) || self.regs.is_unit(dest) {
            return;
        }
        if is_agg(ty) {
            if self.regs.is_agg(dest) {
                self.copy(&Self::reg_ptr(dest), p, ty);
            }
        } else {
            let t = self.tmp();
            self.line(&format!("{t} = load {}, ptr {p}", mem_ty(ty)));
            self.set(dest, &t);
        }
    }

    /// Continue only when `ok` holds; otherwise run `fail` (a call that
    /// never returns).
    fn guard(&mut self, ok: &str, fail: &str) {
        let good = self.label("ok");
        let bad = self.label("fail");
        self.line(&format!("br i1 {ok}, label %{good}, label %{bad}"));
        self.block(&bad);
        self.line(fail);
        self.line("unreachable");
        self.block(&good);
    }

    /// Unconditional runtime error with the VM's message; emission goes on in
    /// a fresh (dead) block.
    fn rt_error(&mut self, msg: &str) {
        let m = self.cx.message(msg);
        self.line(&format!("call void @ae.rt_error(ptr {m})"));
        self.line("unreachable");
        let dead = self.label("dead");
        self.block(&dead);
    }
}

fn emit_function(out: &mut String, f: &IrFunction, cx: &mut ModuleCtx<'_>) {
    let regs = infer_regs(f, cx);
    let sym = cx.symbol(f);
    let mut fx = FnCtx {
        out: String::new(),
        entry: String::new(),
        regs,
        next_tmp: 0,
        next_label: 0,
        cx,
    };

    let sret = is_agg(&f.return_ty);
    let mut params = Vec::new();
    if sret {
        params.push("ptr %ret".to_string());
    }
    for (i, (_, t, _)) in f.params.iter().enumerate() {
        if !is_unit(t) {
            params.push(format!("{} %p{i}", slot_ty(t)));
        }
    }

    // Parameters are copied into the callee's own registers: aggregates by
    // value, so a callee never mutates its caller's storage.
    for (i, (_, t, r)) in f.params.iter().enumerate() {
        if is_unit(t) || fx.regs.is_unit(*r) {
            continue;
        }
        if is_agg(t) {
            fx.copy(&FnCtx::reg_ptr(*r), &format!("%p{i}"), t);
        } else {
            fx.set(*r, &format!("%p{i}"));
        }
    }
    match f.blocks.first() {
        Some(bb) => fx.line(&format!("br label %bb{}", bb.id.0)),
        None => emit_return(&mut fx, None, &f.return_ty),
    }
    for bb in &f.blocks {
        let _ = writeln!(fx.out, "bb{}:", bb.id.0);
        for inst in &bb.insts {
            emit_inst(&mut fx, inst);
        }
        emit_term(&mut fx, &bb.term, &f.return_ty);
    }

    let _ = writeln!(
        out,
        "define {} {sym}({}) {{",
        ret_ty(&f.return_ty),
        params.join(", ")
    );
    out.push_str("entry:\n");
    for i in 0..fx.regs.types.len() {
        let r = Reg(i as u32);
        if !fx.regs.referenced[i] || fx.regs.is_unit(r) {
            continue;
        }
        let _ = writeln!(out, "  %r{i} = alloca {}", fx.regs.slot(r));
    }
    out.push_str(&fx.entry);
    out.push_str(&fx.out);
    out.push_str("}\n\n");
}

fn emit_inst(fx: &mut FnCtx, inst: &Inst) {
    match inst {
        Inst::LoadConst { dest, value } => {
            if fx.regs.is_unit(*dest) || fx.regs.is_agg(*dest) {
                return;
            }
            let slot = fx.regs.slot(*dest);
            let v = match value {
                ConstValue::I32(v) => v.to_string(),
                ConstValue::I64(v) => v.to_string(),
                ConstValue::F64(v) => llvm_f64(*v),
                ConstValue::Bool(v) => v.to_string(),
                ConstValue::String(s) => fx.cx.string(s),
                ConstValue::Char(c) => (*c as u32).to_string(),
                ConstValue::Unit => zero_of(&slot).to_string(),
            };
            let v = if mem_ty(&value.ty()) == slot || *value == ConstValue::Unit {
                v
            } else {
                zero_of(&slot).to_string()
            };
            fx.set(*dest, &v);
        }
        Inst::Move { dest, src } => {
            if dest == src || fx.regs.is_unit(*dest) {
                return;
            }
            let (dt, st) = (fx.regs.ty(*dest).cloned(), fx.regs.ty(*src).cloned());
            match (dt, st) {
                (Some(d), Some(s)) if d == s => {
                    let v = fx.val(*src);
                    fx.set(*dest, &v);
                }
                (Some(d), Some(s)) if !is_agg(&d) && !is_agg(&s) && !is_unit(&s) => {
                    let v = fx.val(*src);
                    let c = emit_cast(fx, &v, &s, &d);
                    fx.set(*dest, &c);
                }
                (_, None) if !fx.regs.is_agg(*dest) => {
                    let v = fx.val(*src);
                    let slot = fx.regs.slot(*dest);
                    let c = if slot == "i32" { v } else { zero_of(&slot).to_string() };
                    fx.set(*dest, &c);
                }
                _ => {}
            }
        }
        Inst::Bin {
            dest,
            op,
            ty,
            lhs,
            rhs,
        } => emit_bin(fx, *dest, *op, ty, *lhs, *rhs),
        Inst::Un { dest, op, ty, src } => {
            let v = fx.val(*src);
            let t = fx.tmp();
            match (op, ty) {
                (UnOp::Neg, Type::F64) => fx.line(&format!("{t} = fneg double {v}")),
                (UnOp::Neg, _) => fx.line(&format!("{t} = sub {} 0, {v}", mem_ty(ty))),
                (UnOp::Not, Type::Bool) => fx.line(&format!("{t} = xor i1 {v}, true")),
                (UnOp::Not, Type::F64) => {
                    let i = fx.tmp();
                    let x = fx.tmp();
                    fx.line(&format!("{i} = bitcast double {v} to i64"));
                    fx.line(&format!("{x} = xor i64 {i}, -1"));
                    fx.line(&format!("{t} = bitcast i64 {x} to double"));
                }
                (UnOp::Not, _) => fx.line(&format!("{t} = xor {} {v}, -1", mem_ty(ty))),
            }
            fx.set(*dest, &t);
        }
        Inst::Call { dest, func, args } => emit_call(fx, *dest, func, args),
        Inst::Cast { dest, src, from, to } => {
            if fx.regs.is_unit(*dest) {
                return;
            }
            let v = fx.val(*src);
            if from == to || is_agg(to) {
                if fx.regs.ty(*src) == fx.regs.ty(*dest) {
                    fx.set(*dest, &v);
                }
                return;
            }
            let r = emit_cast(fx, &v, from, to);
            fx.set(*dest, &r);
        }
        Inst::IndexLoad {
            dest,
            base,
            index,
            elem,
        } => match fx.regs.ty(*base).cloned() {
            Some(Type::String) => {
                let s = fx.val(*base);
                let i = fx.val(*index);
                let t = fx.tmp();
                fx.line(&format!("{t} = call i32 @ae.str_index(ptr {s}, i32 {i})"));
                fx.set(*dest, &t);
            }
            Some(arr @ Type::Array { .. }) => {
                let p = elem_ptr(fx, *base, *index, &arr);
                fx.load_at(&p, *dest, elem);
            }
            other => fx.rt_error(&format!("cannot index a value of type {}", vm_type_name(other.as_ref()))),
        },
        Inst::IndexStore {
            base,
            index,
            value,
            elem,
        } => match fx.regs.ty(*base).cloned() {
            Some(arr @ Type::Array { .. }) => {
                let p = elem_ptr(fx, *base, *index, &arr);
                let v = fx.val(*value);
                fx.store_at(&p, &v, elem);
            }
            other => fx.rt_error(&format!(
                "cannot store into a value of type {}",
                vm_type_name(other.as_ref())
            )),
        },
        Inst::FieldLoad {
            dest,
            base,
            index,
            ty,
        } => match field_ptr(fx, *base, *index) {
            Some(p) => fx.load_at(&p, *dest, ty),
            None => fx.rt_error(&format!("cannot read field {index}")),
        },
        Inst::FieldStore {
            base,
            index,
            value,
            ty,
        } => match field_ptr(fx, *base, *index) {
            Some(p) => {
                let v = fx.val(*value);
                fx.store_at(&p, &v, ty);
            }
            None => fx.rt_error(&format!("cannot write field {index}")),
        },
        // The destination register's own storage *is* the new object; the
        // stores that follow initialize it.
        Inst::AllocArray { .. } | Inst::AllocStruct { .. } => {}
        Inst::Yield => fx.line("; yield (no-op outside the VM)"),
        Inst::Nop => {}
    }
}

/// The VM's name for a value of this type, for error messages.
fn vm_type_name(ty: Option<&Type>) -> &'static str {
    match ty {
        Some(Type::I32) => "i32",
        Some(Type::I64) => "i64",
        Some(Type::F64) => "f64",
        Some(Type::Bool) => "bool",
        Some(Type::String) => "string",
        Some(Type::Char) => "char",
        Some(Type::Array { .. }) => "array",
        Some(Type::Struct { .. } | Type::Tuple(_) | Type::Enum { .. }) => "struct",
        _ => "unit",
    }
}

/// Bounds-checked pointer to element `index` of the array register `base`.
fn elem_ptr(fx: &mut FnCtx, base: Reg, index: Reg, arr: &Type) -> String {
    let len = match arr {
        Type::Array { len, .. } => (*len).max(0),
        _ => 0,
    };
    let i = fx.val(index);
    let ok = fx.tmp();
    fx.line(&format!("{ok} = icmp ult i32 {i}, {len}"));
    fx.guard(&ok, &format!("call void @ae.index_oob(i32 {i})"));
    let p = fx.tmp();
    fx.line(&format!(
        "{p} = getelementptr inbounds {}, ptr %r{}, i64 0, i32 {i}",
        mem_ty(arr),
        base.0
    ));
    p
}

/// Pointer to field `index` of the struct / tuple / enum register `base`
/// (`None` when the register holds no such aggregate).
fn field_ptr(fx: &mut FnCtx, base: Reg, index: usize) -> Option<String> {
    let ty = fx.regs.ty(base).cloned()?;
    let n = match &ty {
        Type::Enum { .. } => enum_slots(&ty).len(),
        Type::Struct { .. } | Type::Tuple(_) => ty.layout_fields()?.len(),
        _ => return None,
    };
    if index >= n {
        return None;
    }
    let p = fx.tmp();
    fx.line(&format!(
        "{p} = getelementptr inbounds {}, ptr %r{}, i32 0, i32 {index}",
        mem_ty(&ty),
        base.0
    ));
    Some(p)
}

fn emit_bin(fx: &mut FnCtx, dest: Reg, op: BinOp, ty: &Type, lhs: Reg, rhs: Reg) {
    if is_unit(ty) {
        // `()` compares equal to itself (the VM's `cmp_values`).
        let v = matches!(op, BinOp::Eq | BinOp::Le | BinOp::Ge);
        fx.set(dest, if v { "true" } else { "false" });
        return;
    }
    let a = fx.val(lhs);
    let b = fx.val(rhs);
    let t = fx.tmp();
    match ty {
        Type::String => match op {
            BinOp::Add => fx.line(&format!("{t} = call ptr @ae.concat(ptr {a}, ptr {b})")),
            BinOp::Eq | BinOp::Ne => {
                let e = fx.tmp();
                fx.line(&format!("{e} = call i1 @ae.str_eq(ptr {a}, ptr {b})"));
                let flip = if op == BinOp::Eq { "false" } else { "true" };
                fx.line(&format!("{t} = xor i1 {e}, {flip}"));
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let c = fx.tmp();
                fx.line(&format!("{c} = call i32 @ae.str_cmp(ptr {a}, ptr {b})"));
                fx.line(&format!("{t} = icmp {} i32 {c}, 0", icmp_pred(op, true)));
            }
            _ => {
                fx.rt_error("cannot concatenate string with string");
                return;
            }
        },
        _ if is_agg(ty) => match op {
            BinOp::Eq | BinOp::Ne => {
                let f = fx.cx.eq_fn(ty);
                let e = fx.tmp();
                fx.line(&format!("{e} = call i1 {f}(ptr {a}, ptr {b})"));
                let flip = if op == BinOp::Eq { "false" } else { "true" };
                fx.line(&format!("{t} = xor i1 {e}, {flip}"));
            }
            _ => {
                let msg = format!("cannot order two values of type {}", vm_type_name(Some(ty)));
                fx.rt_error(&msg);
                return;
            }
        },
        Type::F64 => {
            let opcode = match op {
                BinOp::Add => "fadd",
                BinOp::Sub => "fsub",
                BinOp::Mul => "fmul",
                BinOp::Div => "fdiv",
                BinOp::Rem => "frem",
                BinOp::Eq => "fcmp oeq",
                BinOp::Ne => "fcmp une",
                BinOp::Lt => "fcmp olt",
                BinOp::Le => "fcmp ole",
                BinOp::Gt => "fcmp ogt",
                BinOp::Ge => "fcmp oge",
                // Not typeable on floats; defined as the integer operation on
                // the bit patterns so the module stays well-formed.
                _ => {
                    let ai = fx.tmp();
                    let bi = fx.tmp();
                    fx.line(&format!("{ai} = bitcast double {a} to i64"));
                    fx.line(&format!("{bi} = bitcast double {b} to i64"));
                    let ri = int_bin(fx, op, "i64", 63, &ai, &bi, true);
                    let r = fx.tmp();
                    if op.is_cmp() {
                        fx.set(dest, &ri);
                    } else {
                        fx.line(&format!("{r} = bitcast i64 {ri} to double"));
                        fx.set(dest, &r);
                    }
                    return;
                }
            };
            fx.line(&format!("{t} = {opcode} double {a}, {b}"));
        }
        _ => {
            // i32, i64, char (i32), bool (i1).
            let lty = mem_ty(ty);
            let mask = if *ty == Type::I64 { 63 } else { 31 };
            let signed = !matches!(ty, Type::Bool | Type::Char);
            let r = int_bin(fx, op, &lty, mask, &a, &b, signed);
            fx.set(dest, &r);
            return;
        }
    }
    fx.set(dest, &t);
}

/// Integer binary operation with the VM's semantics: wrapping arithmetic,
/// division by zero is a runtime error, `MIN / -1` wraps and `MIN % -1` is
/// 0, shift amounts are masked. Returns the SSA result.
fn int_bin(fx: &mut FnCtx, op: BinOp, lty: &str, mask: u32, a: &str, b: &str, signed: bool) -> String {
    let t = fx.tmp();
    let simple = match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::And | BinOp::BitAnd => "and",
        BinOp::Or | BinOp::BitOr => "or",
        BinOp::BitXor => "xor",
        BinOp::Shl | BinOp::Shr => {
            let m = fx.tmp();
            fx.line(&format!("{m} = and {lty} {b}, {mask}"));
            let opc = if op == BinOp::Shl { "shl" } else { "ashr" };
            fx.line(&format!("{t} = {opc} {lty} {a}, {m}"));
            return t;
        }
        BinOp::Div | BinOp::Rem => {
            let z = fx.tmp();
            fx.line(&format!("{z} = icmp ne {lty} {b}, 0"));
            fx.guard(&z, "call void @ae.div_zero()");
            let m1 = fx.tmp();
            let safe = fx.tmp();
            let q = fx.tmp();
            fx.line(&format!("{m1} = icmp eq {lty} {b}, -1"));
            fx.line(&format!("{safe} = select i1 {m1}, {lty} 1, {lty} {b}"));
            if op == BinOp::Div {
                let neg = fx.tmp();
                fx.line(&format!("{q} = sdiv {lty} {a}, {safe}"));
                fx.line(&format!("{neg} = sub {lty} 0, {a}"));
                fx.line(&format!("{t} = select i1 {m1}, {lty} {neg}, {lty} {q}"));
            } else {
                fx.line(&format!("{q} = srem {lty} {a}, {safe}"));
                fx.line(&format!("{t} = select i1 {m1}, {lty} 0, {lty} {q}"));
            }
            return t;
        }
        cmp => {
            fx.line(&format!("{t} = icmp {} {lty} {a}, {b}", icmp_pred(cmp, signed)));
            return t;
        }
    };
    fx.line(&format!("{t} = {simple} {lty} {a}, {b}"));
    t
}

fn icmp_pred(op: BinOp, signed: bool) -> &'static str {
    match (op, signed) {
        (BinOp::Eq, _) => "eq",
        (BinOp::Ne, _) => "ne",
        (BinOp::Lt, true) => "slt",
        (BinOp::Le, true) => "sle",
        (BinOp::Gt, true) => "sgt",
        (BinOp::Ge, true) => "sge",
        (BinOp::Lt, false) => "ult",
        (BinOp::Le, false) => "ule",
        (BinOp::Gt, false) => "ugt",
        (BinOp::Ge, false) => "uge",
        _ => "ne",
    }
}

fn emit_call(fx: &mut FnCtx, dest: Option<Reg>, func: &str, args: &[Reg]) {
    if let Some(f) = fx.cx.module.function(func) {
        let sym = fx.cx.symbol(f);
        let params: Vec<Type> = f.params.iter().map(|(_, t, _)| t.clone()).collect();
        let ret = f.return_ty.clone();
        if f.is_extern {
            // Unbound `extern_weak` symbols are null: the VM's "no
            // implementation" runtime error.
            let ok = fx.tmp();
            fx.line(&format!("{ok} = icmp ne ptr {sym}, null"));
            let m = fx.cx.message(func);
            fx.guard(&ok, &format!("call void @ae.extern_missing(ptr {m})"));
        }
        let mut argv = Vec::new();
        if is_agg(&ret) {
            let out = match dest {
                Some(d) if fx.regs.ty(d) == Some(&ret) => FnCtx::reg_ptr(d),
                _ => fx.temp_alloca(&ret),
            };
            argv.push(format!("ptr {out}"));
        }
        for (r, pt) in args.iter().zip(&params) {
            if is_unit(pt) {
                continue;
            }
            let v = arg_value(fx, *r, pt);
            argv.push(format!("{} {v}", slot_ty(pt)));
        }
        let call = format!("{sym}({})", argv.join(", "));
        finish_call(fx, dest, &ret, &call);
        return;
    }
    let Some((params, ret)) = native_sig(func) else {
        let m = fx.cx.message(func);
        fx.line(&format!("call void @ae.extern_missing(ptr {m})"));
        fx.line("unreachable");
        let dead = fx.label("dead");
        fx.block(&dead);
        return;
    };
    if func == "len" {
        if let Some(Type::Array { len, .. }) = args.first().and_then(|r| fx.regs.ty(*r)).cloned() {
            if let Some(d) = dest {
                fx.set(d, &len.max(0).to_string());
            }
            return;
        }
    }
    let mut argv = Vec::new();
    for (r, pt) in args.iter().zip(&params) {
        let v = arg_value(fx, *r, pt);
        argv.push(format!("{} {v}", slot_ty(pt)));
    }
    let name = if func == "len" { "str_len" } else { func };
    let call = format!("@ae.{name}({})", argv.join(", "));
    finish_call(fx, dest, &ret, &call);
}

/// The value of argument register `r` for a parameter of type `pt`.
fn arg_value(fx: &mut FnCtx, r: Reg, pt: &Type) -> String {
    match fx.regs.ty(r).cloned() {
        Some(t) if t == *pt => fx.val(r),
        Some(t) if !is_agg(&t) && !is_unit(&t) && !is_agg(pt) => {
            let v = fx.val(r);
            emit_cast(fx, &v, &t, pt)
        }
        _ if is_agg(pt) => fx.temp_alloca(pt),
        _ => zero_of(&slot_ty(pt)).to_string(),
    }
}

fn finish_call(fx: &mut FnCtx, dest: Option<Reg>, ret: &Type, call: &str) {
    if is_unit(ret) || is_agg(ret) {
        fx.line(&format!("call void {call}"));
        return;
    }
    let rty = mem_ty(ret);
    match dest {
        Some(d) if fx.regs.ty(d) == Some(ret) => {
            let t = fx.tmp();
            fx.line(&format!("{t} = call {rty} {call}"));
            fx.set(d, &t);
        }
        _ => fx.line(&format!("call {rty} {call}")),
    }
}

/// Convert scalar `v` from `from` to `to` (the casts sema allows, with the
/// VM's semantics), returning the resulting SSA value.
fn emit_cast(fx: &mut FnCtx, v: &str, from: &Type, to: &Type) -> String {
    let fty = mem_ty(from);
    let tty = mem_ty(to);
    if from == to {
        return v.to_string();
    }
    let t = fx.tmp();
    match (from, to) {
        // Rust `as`: saturating, NaN -> 0.
        (Type::F64, Type::I32 | Type::I64) => {
            fx.line(&format!("{t} = call {tty} @llvm.fptosi.sat.{tty}.f64(double {v})"));
        }
        (Type::F64, Type::Char) => {
            let i = fx.tmp();
            fx.line(&format!("{i} = call i32 @llvm.fptosi.sat.i32.f64(double {v})"));
            fx.line(&format!("{t} = call i32 @ae.to_char(i32 {i})"));
        }
        (Type::I32, Type::Char) => fx.line(&format!("{t} = call i32 @ae.to_char(i32 {v})")),
        (Type::I64, Type::Char) => {
            let i = fx.tmp();
            fx.line(&format!("{i} = trunc i64 {v} to i32"));
            fx.line(&format!("{t} = call i32 @ae.to_char(i32 {i})"));
        }
        (_, Type::Bool) if fty == "double" => fx.line(&format!("{t} = fcmp une double {v}, 0.0")),
        (_, Type::Bool) if fty == "ptr" => fx.line(&format!("{t} = icmp ne ptr {v}, null")),
        (_, Type::Bool) => fx.line(&format!("{t} = icmp ne {fty} {v}, 0")),
        (Type::Bool | Type::Char, Type::F64) => fx.line(&format!("{t} = uitofp {fty} {v} to double")),
        (_, Type::F64) if fty != "ptr" => fx.line(&format!("{t} = sitofp {fty} {v} to double")),
        (Type::Bool | Type::Char, _) if tty == "i64" => fx.line(&format!("{t} = zext {fty} {v} to i64")),
        (Type::Bool, _) if tty == "i32" => fx.line(&format!("{t} = zext i1 {v} to i32")),
        _ if fty == tty => return v.to_string(),
        _ if fty == "i32" && tty == "i64" => fx.line(&format!("{t} = sext i32 {v} to i64")),
        _ if fty == "i64" && tty == "i32" => fx.line(&format!("{t} = trunc i64 {v} to i32")),
        _ => return zero_of(&tty).to_string(),
    }
    t
}

fn emit_return(fx: &mut FnCtx, value: Option<Reg>, ret: &Type) {
    if is_unit(ret) {
        fx.line("ret void");
        return;
    }
    if is_agg(ret) {
        if let Some(r) = value {
            if fx.regs.ty(r) == Some(ret) {
                fx.copy("%ret", &FnCtx::reg_ptr(r), ret);
            }
        }
        fx.line("ret void");
        return;
    }
    let rty = mem_ty(ret);
    let v = match value {
        Some(r) => arg_value(fx, r, ret),
        None => zero_of(&rty).to_string(),
    };
    fx.line(&format!("ret {rty} {v}"));
}

fn emit_term(fx: &mut FnCtx, term: &Terminator, ret: &Type) {
    match term {
        Terminator::Jump { target } => fx.line(&format!("br label %bb{}", target.0)),
        Terminator::Branch {
            cond,
            then_bb,
            else_bb,
        } => {
            let c = fx.val(*cond);
            let c = match fx.regs.ty(*cond).cloned() {
                Some(Type::Bool) => c,
                Some(t) if !is_agg(&t) && !is_unit(&t) => emit_cast(fx, &c, &t, &Type::Bool),
                // Untyped: tested against zero; anything else is never true.
                None => {
                    let t = fx.tmp();
                    fx.line(&format!("{t} = icmp ne i32 {c}, 0"));
                    t
                }
                Some(_) => "false".to_string(),
            };
            fx.line(&format!(
                "br i1 {c}, label %bb{}, label %bb{}",
                then_bb.0, else_bb.0
            ));
        }
        Terminator::Return { value } => emit_return(fx, *value, ret),
        Terminator::Unreachable => fx.line("unreachable"),
    }
}

// ---------------------------------------------------------------------------
// Structural equality helpers
// ---------------------------------------------------------------------------

/// `define internal i1 @ae.eq.N(ptr %a, ptr %b)`: `==` on two values of an
/// aggregate type, element by element with each element type's own `==`
/// (IEEE for floats, contents for strings, active payload for enums).
fn emit_eq_fn(out: &mut String, idx: usize, ty: &Type, cx: &mut ModuleCtx) {
    let mut n = 0u32;
    let mut b = String::new();
    let lty = mem_ty(ty);
    match ty {
        Type::Array { elem, len } if *len > 0 => {
            b.push_str("  br label %loop\nloop:\n");
            b.push_str("  %i = phi i64 [ 0, %entry ], [ %i1, %next ]\n");
            let _ = writeln!(b, "  %done = icmp eq i64 %i, {len}");
            b.push_str("  br i1 %done, label %yes, label %body\nbody:\n");
            let _ = writeln!(b, "  %pa = getelementptr inbounds {lty}, ptr %a, i64 0, i64 %i");
            let _ = writeln!(b, "  %pb = getelementptr inbounds {lty}, ptr %b, i64 0, i64 %i");
            let e = eq_at(&mut b, &mut n, cx, "%pa", "%pb", elem);
            let _ = writeln!(b, "  br i1 {e}, label %next, label %no");
            b.push_str("next:\n  %i1 = add i64 %i, 1\n  br label %loop\n");
            b.push_str("yes:\n  ret i1 true\nno:\n  ret i1 false\n");
        }
        Type::Struct { .. } | Type::Tuple(_) => {
            let fields = ty.layout_fields().unwrap_or_default();
            let acc = eq_fields(&mut b, &mut n, cx, &lty, &fields, 0);
            let _ = writeln!(b, "  ret i1 {acc}");
        }
        Type::Enum { variants, .. } => {
            b.push_str("  %ta = load i32, ptr %a\n  %tb = load i32, ptr %b\n");
            b.push_str("  %same = icmp eq i32 %ta, %tb\n  br i1 %same, label %tags, label %no\n");
            b.push_str("no:\n  ret i1 false\ntags:\n");
            let cases: Vec<String> = variants
                .iter()
                .enumerate()
                .filter(|(_, (_, p))| !p.is_empty())
                .map(|(k, _)| format!("i32 {k}, label %v{k}"))
                .collect();
            let _ = writeln!(b, "  switch i32 %ta, label %yes [ {} ]", cases.join(" "));
            for (k, (_, p)) in variants.iter().enumerate() {
                if p.is_empty() {
                    continue;
                }
                let _ = writeln!(b, "v{k}:");
                let acc = eq_fields(&mut b, &mut n, cx, &lty, p, 1);
                let _ = writeln!(b, "  ret i1 {acc}");
            }
            b.push_str("yes:\n  ret i1 true\n");
        }
        _ => b.push_str("  ret i1 true\n"),
    }
    let _ = writeln!(out, "define internal i1 @ae.eq.{idx}(ptr %a, ptr %b) {{\nentry:\n{b}}}\n");
}

/// `and` of the equality of fields `first..` (typed `fields`) of two `lty`s.
fn eq_fields(b: &mut String, n: &mut u32, cx: &mut ModuleCtx, lty: &str, fields: &[Type], first: usize) -> String {
    let mut acc = "true".to_string();
    for (i, t) in fields.iter().enumerate() {
        if is_unit(t) {
            continue;
        }
        let pa = format!("%f{n}");
        let pb = format!("%g{n}");
        *n += 1;
        let _ = writeln!(b, "  {pa} = getelementptr inbounds {lty}, ptr %a, i32 0, i32 {}", first + i);
        let _ = writeln!(b, "  {pb} = getelementptr inbounds {lty}, ptr %b, i32 0, i32 {}", first + i);
        let e = eq_at(b, n, cx, &pa, &pb, t);
        let c = format!("%c{n}");
        *n += 1;
        let _ = writeln!(b, "  {c} = and i1 {acc}, {e}");
        acc = c;
    }
    acc
}

/// Straight-line `==` of the two values of type `ty` stored at `pa` / `pb`.
fn eq_at(b: &mut String, n: &mut u32, cx: &mut ModuleCtx, pa: &str, pb: &str, ty: &Type) -> String {
    if is_unit(ty) {
        return "true".into();
    }
    let r = format!("%e{n}");
    *n += 1;
    if is_agg(ty) {
        let f = cx.eq_fn(ty);
        let _ = writeln!(b, "  {r} = call i1 {f}(ptr {pa}, ptr {pb})");
        return r;
    }
    let lty = mem_ty(ty);
    let (x, y) = (format!("%x{n}"), format!("%y{n}"));
    *n += 1;
    let _ = writeln!(b, "  {x} = load {lty}, ptr {pa}");
    let _ = writeln!(b, "  {y} = load {lty}, ptr {pb}");
    match ty {
        Type::F64 => {
            let _ = writeln!(b, "  {r} = fcmp oeq double {x}, {y}");
        }
        Type::String => {
            let _ = writeln!(b, "  {r} = call i1 @ae.str_eq(ptr {x}, ptr {y})");
        }
        Type::Fn { .. } => {
            let _ = writeln!(b, "  {r} = icmp eq ptr {x}, {y}");
        }
        _ => {
            let _ = writeln!(b, "  {r} = icmp eq {lty} {x}, {y}");
        }
    }
    r
}

// ---------------------------------------------------------------------------
// External tools
// ---------------------------------------------------------------------------

/// Run `opt` + `lli` if available. Returns captured stdout of the program.
pub fn run_with_lli(ir: &str) -> Result<String, String> {
    let opt = find_tool("opt-18").or_else(|| find_tool("opt"));
    let lli = find_tool("lli-18").or_else(|| find_tool("lli"));
    let lli = lli.ok_or_else(|| "lli not found on PATH".to_string())?;

    let processed = if let Some(opt) = opt {
        let mut child = Command::new(&opt)
            .args(["-S", "-O2"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(ir.as_bytes()).map_err(|e| e.to_string())?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else {
        ir.to_string()
    };

    let mut child = Command::new(&lli)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        stdin
            .write_all(processed.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "lli failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn find_tool(name: &str) -> Option<String> {
    Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// The runtime appended to every module: generated from the C source in
/// `docs/llvm.md` (clang -O1, attributes and metadata stripped, `ae_` renamed
/// to `ae.`, every definition `internal`). Runtime errors print
/// `runtime error: <the VM's message>` on stderr after flushing stdout and
/// `abort()`.
const RUNTIME_IR: &str = r##"%ae.Str = type { i64, [0 x i8] }

@stdout = external global ptr, align 8
@.aert.str = private unnamed_addr constant [16 x i8] c"runtime error: \00", align 1
@stderr = external global ptr, align 8
@.aert.str.1 = private unnamed_addr constant [29 x i8] c"array index %d out of bounds\00", align 1
@.aert.str.2 = private unnamed_addr constant [17 x i8] c"division by zero\00", align 1
@.aert.str.3 = private unnamed_addr constant [63 x i8] c"extern function `%s` has no implementation (unresolved symbol)\00", align 1
@.aert.str.4 = private unnamed_addr constant [17 x i8] c"assertion failed\00", align 1
@.aert.str.5 = private unnamed_addr constant [14 x i8] c"out of memory\00", align 1
@.aert.str.6 = private unnamed_addr constant [45 x i8] c"string of %lld bytes exceeds the limit of %d\00", align 1
@.aert.str.7 = private unnamed_addr constant [27 x i8] c"string index out of bounds\00", align 1
@.aert.str.8 = private unnamed_addr constant [4 x i8] c"NaN\00", align 1
@.aert.str.9 = private unnamed_addr constant [4 x i8] c"inf\00", align 1
@.aert.str.10 = private unnamed_addr constant [5 x i8] c"%.*e\00", align 1
@.aert.str.11 = private unnamed_addr constant [6 x i8] c"%.16e\00", align 1
@.aert.str.12 = private unnamed_addr constant [4 x i8] c"%d\0A\00", align 1
@.aert.str.13 = private unnamed_addr constant [6 x i8] c"%lld\0A\00", align 1
@.aert.str.14 = private unnamed_addr constant [6 x i8] c"true\0A\00", align 1
@.aert.str.15 = private unnamed_addr constant [7 x i8] c"false\0A\00", align 1
@.aert.str.16 = private unnamed_addr constant [3 x i8] c"%d\00", align 1
@.aert.str.17 = private unnamed_addr constant [5 x i8] c"%lld\00", align 1
@.aert.str.18 = private unnamed_addr constant [4 x i8] c"e%d\00", align 1

define internal void @ae.rt_error(ptr %0) noreturn {
  %2 = load ptr, ptr @stdout, align 8
  %3 = tail call i32 @fflush(ptr %2)
  %4 = load ptr, ptr @stderr, align 8
  %5 = tail call i64 @fwrite(ptr @.aert.str, i64 15, i64 1, ptr %4)
  %6 = load ptr, ptr @stderr, align 8
  %7 = tail call i32 @fputs(ptr %0, ptr %6)
  %8 = load ptr, ptr @stderr, align 8
  %9 = tail call i32 @fputc(i32 10, ptr %8)
  %10 = load ptr, ptr @stderr, align 8
  %11 = tail call i32 @fflush(ptr %10)
  tail call void @abort()
  unreachable
}

declare i32 @fflush(ptr)

declare i32 @fputs(ptr, ptr)

declare i32 @fputc(i32, ptr)

declare void @abort()

define internal void @ae.index_oob(i32 %0) noreturn {
  %2 = alloca [64 x i8], align 16
  call void @llvm.lifetime.start.p0(i64 64, ptr %2)
  %3 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %2, i64 64, ptr @.aert.str.1, i32 %0)
  call void @ae.rt_error(ptr %2)
  unreachable
}

declare void @llvm.lifetime.start.p0(i64, ptr)

declare i32 @snprintf(ptr, i64, ptr, ...)

define internal void @ae.div_zero() noreturn {
  tail call void @ae.rt_error(ptr @.aert.str.2)
  unreachable
}

define internal void @ae.extern_missing(ptr %0) noreturn {
  %2 = alloca [512 x i8], align 16
  call void @llvm.lifetime.start.p0(i64 512, ptr %2)
  %3 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %2, i64 512, ptr @.aert.str.3, ptr %0)
  call void @ae.rt_error(ptr %2)
  unreachable
}

define internal void @ae.assert(i1 %0) {
  br i1 %0, label %3, label %2

2:                                                ; preds = %1
  tail call void @ae.rt_error(ptr @.aert.str.4)
  unreachable

3:                                                ; preds = %1
  ret void
}

define internal ptr @ae.str_alloc(i64 %0) {
  %2 = add i64 %0, 9
  %3 = tail call ptr @malloc(i64 %2)
  %4 = icmp eq ptr %3, null
  br i1 %4, label %5, label %6

5:                                                ; preds = %1
  tail call void @ae.rt_error(ptr @.aert.str.5)
  unreachable

6:                                                ; preds = %1
  store i64 %0, ptr %3, align 8
  %7 = getelementptr inbounds %ae.Str, ptr %3, i64 0, i32 1, i64 %0
  store i8 0, ptr %7, align 1
  ret ptr %3
}

declare ptr @malloc(i64)

declare void @llvm.lifetime.end.p0(i64, ptr)

define internal ptr @ae.concat(ptr %0, ptr %1) {
  %3 = alloca [96 x i8], align 16
  %4 = load i64, ptr %0, align 8
  %5 = load i64, ptr %1, align 8
  %6 = add nsw i64 %5, %4
  %7 = icmp sgt i64 %6, 268435456
  br i1 %7, label %8, label %10

8:                                                ; preds = %2
  call void @llvm.lifetime.start.p0(i64 96, ptr %3)
  %9 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %3, i64 96, ptr @.aert.str.6, i64 %6, i32 268435456)
  call void @ae.rt_error(ptr %3)
  unreachable

10:                                               ; preds = %2
  %11 = add i64 %6, 9
  %12 = tail call ptr @malloc(i64 %11)
  %13 = icmp eq ptr %12, null
  br i1 %13, label %14, label %15

14:                                               ; preds = %10
  tail call void @ae.rt_error(ptr @.aert.str.5)
  unreachable

15:                                               ; preds = %10
  store i64 %6, ptr %12, align 8
  %16 = getelementptr inbounds %ae.Str, ptr %12, i64 0, i32 1, i64 %6
  store i8 0, ptr %16, align 1
  %17 = getelementptr inbounds %ae.Str, ptr %12, i64 0, i32 1
  %18 = getelementptr inbounds %ae.Str, ptr %0, i64 0, i32 1
  tail call void @llvm.memcpy.p0.p0.i64(ptr align 8 %17, ptr align 8 %18, i64 %4, i1 false)
  %19 = getelementptr inbounds i8, ptr %17, i64 %4
  %20 = getelementptr inbounds %ae.Str, ptr %1, i64 0, i32 1
  tail call void @llvm.memcpy.p0.p0.i64(ptr align 1 %19, ptr align 8 %20, i64 %5, i1 false)
  ret ptr %12
}

declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)

define internal i32 @ae.str_cmp(ptr %0, ptr %1) {
  %3 = load i64, ptr %0, align 8
  %4 = load i64, ptr %1, align 8
  %5 = tail call i64 @llvm.smin.i64(i64 %3, i64 %4)
  %6 = getelementptr inbounds %ae.Str, ptr %0, i64 0, i32 1
  %7 = getelementptr inbounds %ae.Str, ptr %1, i64 0, i32 1
  %8 = tail call i32 @memcmp(ptr %6, ptr %7, i64 %5)
  %9 = icmp eq i32 %8, 0
  br i1 %9, label %13, label %10

10:                                               ; preds = %2
  %11 = icmp sgt i32 %8, -1
  %12 = select i1 %11, i32 1, i32 -1
  br label %18

13:                                               ; preds = %2
  %14 = icmp slt i64 %3, %4
  %15 = icmp sgt i64 %3, %4
  %16 = zext i1 %15 to i32
  %17 = select i1 %14, i32 -1, i32 %16
  br label %18

18:                                               ; preds = %13, %10
  %19 = phi i32 [ %12, %10 ], [ %17, %13 ]
  ret i32 %19
}

declare i32 @memcmp(ptr, ptr, i64)

define internal i1 @ae.str_eq(ptr %0, ptr %1) {
  %3 = load i64, ptr %0, align 8
  %4 = load i64, ptr %1, align 8
  %5 = icmp eq i64 %3, %4
  br i1 %5, label %6, label %11

6:                                                ; preds = %2
  %7 = getelementptr inbounds %ae.Str, ptr %0, i64 0, i32 1
  %8 = getelementptr inbounds %ae.Str, ptr %1, i64 0, i32 1
  %9 = tail call i32 @bcmp(ptr %7, ptr %8, i64 %3)
  %10 = icmp eq i32 %9, 0
  br label %11

11:                                               ; preds = %6, %2
  %12 = phi i1 [ false, %2 ], [ %10, %6 ]
  ret i1 %12
}

define internal i32 @ae.str_len(ptr %0) {
  %2 = load i64, ptr %0, align 8
  %3 = icmp sgt i64 %2, 0
  br i1 %3, label %8, label %4

4:                                                ; preds = %8, %1
  %5 = phi i64 [ 0, %1 ], [ %16, %8 ]
  %6 = tail call i64 @llvm.smin.i64(i64 %5, i64 2147483647)
  %7 = trunc i64 %6 to i32
  ret i32 %7

8:                                                ; preds = %1, %8
  %9 = phi i64 [ %17, %8 ], [ 0, %1 ]
  %10 = phi i64 [ %16, %8 ], [ 0, %1 ]
  %11 = getelementptr inbounds %ae.Str, ptr %0, i64 0, i32 1, i64 %9
  %12 = load i8, ptr %11, align 1
  %13 = and i8 %12, -64
  %14 = icmp ne i8 %13, -128
  %15 = zext i1 %14 to i64
  %16 = add nuw nsw i64 %10, %15
  %17 = add nuw nsw i64 %9, 1
  %18 = icmp eq i64 %17, %2
  br i1 %18, label %4, label %8
}

define internal i32 @ae.str_index(ptr %0, i32 %1) {
  %3 = icmp sgt i32 %1, -1
  br i1 %3, label %4, label %64

4:                                                ; preds = %2
  %5 = load i64, ptr %0, align 8
  %6 = icmp sgt i64 %5, 0
  br i1 %6, label %7, label %61

7:                                                ; preds = %4
  %8 = zext i32 %1 to i64
  br label %12

9:                                                ; preds = %58
  %10 = add nuw nsw i64 %15, 1
  %11 = icmp slt i64 %59, %5
  br i1 %11, label %12, label %61

12:                                               ; preds = %7, %9
  %13 = phi i1 [ %6, %7 ], [ %11, %9 ]
  %14 = phi i32 [ undef, %7 ], [ %60, %9 ]
  %15 = phi i64 [ 0, %7 ], [ %10, %9 ]
  %16 = phi i64 [ 0, %7 ], [ %59, %9 ]
  %17 = getelementptr inbounds %ae.Str, ptr %0, i64 0, i32 1, i64 %16
  %18 = load i8, ptr %17, align 1
  %19 = zext i8 %18 to i32
  %20 = icmp sgt i8 %18, -1
  br i1 %20, label %26, label %21

21:                                               ; preds = %12
  %22 = icmp ult i8 %18, -32
  br i1 %22, label %26, label %23

23:                                               ; preds = %21
  %24 = icmp ult i8 %18, -16
  %25 = select i1 %24, i32 3, i32 4
  br label %26

26:                                               ; preds = %23, %21, %12
  %27 = phi i32 [ 1, %12 ], [ %25, %23 ], [ 2, %21 ]
  %28 = icmp eq i64 %15, %8
  br i1 %28, label %29, label %55

29:                                               ; preds = %26
  switch i32 %27, label %32 [
    i32 1, label %36
    i32 2, label %30
  ]

30:                                               ; preds = %29
  %31 = and i32 %19, 31
  br label %36

32:                                               ; preds = %29
  %33 = icmp eq i32 %27, 3
  %34 = select i1 %33, i32 15, i32 7
  %35 = and i32 %34, %19
  br label %36

36:                                               ; preds = %29, %30, %32
  %37 = phi i32 [ %31, %30 ], [ %35, %32 ], [ %19, %29 ]
  %38 = icmp ugt i32 %27, 1
  br i1 %38, label %39, label %58

39:                                               ; preds = %36
  %40 = zext i32 %27 to i64
  br label %41

41:                                               ; preds = %39, %46
  %42 = phi i64 [ 1, %39 ], [ %53, %46 ]
  %43 = phi i32 [ %37, %39 ], [ %52, %46 ]
  %44 = add nsw i64 %16, %42
  %45 = icmp slt i64 %44, %5
  br i1 %45, label %46, label %58

46:                                               ; preds = %41
  %47 = shl i32 %43, 6
  %48 = getelementptr inbounds %ae.Str, ptr %0, i64 0, i32 1, i64 %44
  %49 = load i8, ptr %48, align 1
  %50 = and i8 %49, 63
  %51 = zext i8 %50 to i32
  %52 = or i32 %47, %51
  %53 = add nuw nsw i64 %42, 1
  %54 = icmp eq i64 %53, %40
  br i1 %54, label %58, label %41

55:                                               ; preds = %26
  %56 = zext i32 %27 to i64
  %57 = add nsw i64 %16, %56
  br label %58

58:                                               ; preds = %46, %41, %36, %55
  %59 = phi i64 [ %57, %55 ], [ %16, %36 ], [ %16, %41 ], [ %16, %46 ]
  %60 = phi i32 [ %14, %55 ], [ %37, %36 ], [ %52, %46 ], [ %43, %41 ]
  br i1 %28, label %61, label %9

61:                                               ; preds = %58, %9, %4
  %62 = phi i1 [ %6, %4 ], [ %13, %58 ], [ %11, %9 ]
  %63 = phi i32 [ undef, %4 ], [ %60, %9 ], [ %60, %58 ]
  br i1 %62, label %65, label %64

64:                                               ; preds = %61, %2
  tail call void @ae.rt_error(ptr @.aert.str.7)
  unreachable

65:                                               ; preds = %61
  ret i32 %63
}

define internal i32 @ae.to_char(i32 %0) {
  %2 = icmp ugt i32 %0, 1114111
  %3 = and i32 %0, 2095104
  %4 = icmp eq i32 %3, 55296
  %5 = or i1 %2, %4
  %6 = select i1 %5, i32 65533, i32 %0
  ret i32 %6
}

define internal i32 @ae.fmt_f64(double %0, ptr %1) {
  %3 = alloca [64 x i8], align 16
  %4 = alloca [24 x i8], align 16
  %5 = alloca [24 x i8], align 16
  %6 = alloca [96 x i8], align 16
  %7 = alloca [80 x i8], align 16
  %8 = alloca i32, align 4
  %9 = fcmp uno double %0, 0.000000e+00
  br i1 %9, label %10, label %11

10:                                               ; preds = %2
  tail call void @llvm.memcpy.p0.p0.i64(ptr align 1 %1, ptr align 1 @.aert.str.8, i64 3, i1 false)
  br label %335

11:                                               ; preds = %2
  %12 = bitcast double %0 to i64
  %13 = icmp slt i64 %12, 0
  br i1 %13, label %14, label %16

14:                                               ; preds = %11
  store i8 45, ptr %1, align 1
  %15 = fneg double %0
  br label %16

16:                                               ; preds = %14, %11
  %17 = phi i32 [ 1, %14 ], [ 0, %11 ]
  %18 = phi double [ %15, %14 ], [ %0, %11 ]
  %19 = fcmp oeq double %18, 0x7FF0000000000000
  br i1 %19, label %20, label %24

20:                                               ; preds = %16
  %21 = zext i32 %17 to i64
  %22 = getelementptr inbounds i8, ptr %1, i64 %21
  tail call void @llvm.memcpy.p0.p0.i64(ptr align 1 %22, ptr align 1 @.aert.str.9, i64 3, i1 false)
  %23 = add nuw nsw i32 %17, 3
  br label %335

24:                                               ; preds = %16
  %25 = fcmp oeq double %18, 0.000000e+00
  br i1 %25, label %26, label %30

26:                                               ; preds = %24
  %27 = add nuw nsw i32 %17, 1
  %28 = zext i32 %17 to i64
  %29 = getelementptr inbounds i8, ptr %1, i64 %28
  store i8 48, ptr %29, align 1
  br label %335

30:                                               ; preds = %24
  call void @llvm.lifetime.start.p0(i64 64, ptr %3)
  call void @llvm.lifetime.start.p0(i64 24, ptr %4)
  call void @llvm.lifetime.start.p0(i64 24, ptr %5)
  %31 = getelementptr inbounds i8, ptr %5, i64 1
  %32 = getelementptr inbounds i8, ptr %5, i64 1
  %33 = getelementptr inbounds i8, ptr %3, i64 1
  %34 = getelementptr inbounds i8, ptr %3, i64 2
  br label %35

35:                                               ; preds = %30, %204
  %36 = phi i64 [ 2, %30 ], [ %207, %204 ]
  %37 = phi i32 [ 0, %30 ], [ %205, %204 ]
  %38 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %3, i64 64, ptr @.aert.str.10, i32 %37, double %18)
  br label %39

39:                                               ; preds = %50, %35
  %40 = phi i32 [ 0, %35 ], [ %51, %50 ]
  %41 = phi ptr [ %3, %35 ], [ %52, %50 ]
  %42 = load i8, ptr %41, align 1
  switch i8 %42, label %43 [
    i8 0, label %53
    i8 101, label %53
  ]

43:                                               ; preds = %39
  %44 = add i8 %42, -48
  %45 = icmp ult i8 %44, 10
  br i1 %45, label %46, label %50

46:                                               ; preds = %43
  %47 = add nsw i32 %40, 1
  %48 = sext i32 %40 to i64
  %49 = getelementptr inbounds i8, ptr %4, i64 %48
  store i8 %42, ptr %49, align 1
  br label %50

50:                                               ; preds = %46, %43
  %51 = phi i32 [ %47, %46 ], [ %40, %43 ]
  %52 = getelementptr inbounds i8, ptr %41, i64 1
  br label %39

53:                                               ; preds = %39, %39
  %54 = getelementptr inbounds i8, ptr %41, i64 1
  %55 = call i64 @strtol(ptr %54, ptr null, i32 10)
  %56 = trunc i64 %55 to i32
  %57 = call double @strtod(ptr %3, ptr null)
  %58 = fcmp oeq double %57, %18
  br i1 %58, label %73, label %59

59:                                               ; preds = %53
  %60 = sext i32 %40 to i64
  %61 = add nsw i32 %40, -1
  %62 = icmp sgt i32 %40, 0
  %63 = icmp eq i32 %40, 1
  %64 = add nsw i64 %60, -1
  %65 = sext i32 %61 to i64
  %66 = getelementptr inbounds i8, ptr %5, i64 %65
  %67 = icmp sgt i32 %40, 0
  %68 = add nsw i64 %60, -1
  %69 = icmp sgt i32 %40, 1
  %70 = add i32 %40, 1
  %71 = zext i32 %70 to i64
  %72 = add nsw i64 %71, -2
  br label %134

73:                                               ; preds = %53
  call void @llvm.lifetime.start.p0(i64 96, ptr %6)
  call void @llvm.lifetime.start.p0(i64 80, ptr %7)
  %74 = add nuw nsw i32 %37, 31
  %75 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %6, i64 96, ptr @.aert.str.10, i32 %74, double %18)
  br label %76

76:                                               ; preds = %87, %73
  %77 = phi i32 [ 0, %73 ], [ %88, %87 ]
  %78 = phi ptr [ %6, %73 ], [ %89, %87 ]
  %79 = load i8, ptr %78, align 1
  switch i8 %79, label %80 [
    i8 0, label %90
    i8 101, label %90
  ]

80:                                               ; preds = %76
  %81 = add i8 %79, -48
  %82 = icmp ult i8 %81, 10
  br i1 %82, label %83, label %87

83:                                               ; preds = %80
  %84 = add nsw i32 %77, 1
  %85 = sext i32 %77 to i64
  %86 = getelementptr inbounds i8, ptr %7, i64 %85
  store i8 %79, ptr %86, align 1
  br label %87

87:                                               ; preds = %83, %80
  %88 = phi i32 [ %84, %83 ], [ %77, %80 ]
  %89 = getelementptr inbounds i8, ptr %78, i64 1
  br label %76

90:                                               ; preds = %76, %76
  %91 = getelementptr inbounds i8, ptr %78, i64 1
  %92 = call i64 @strtol(ptr %91, ptr null, i32 10)
  %93 = trunc i64 %92 to i32
  %94 = or i32 %37, 32
  %95 = icmp eq i32 %77, %94
  br i1 %95, label %96, label %102

96:                                               ; preds = %90
  %97 = add nuw nsw i32 %37, 1
  %98 = zext i32 %97 to i64
  %99 = getelementptr inbounds [80 x i8], ptr %7, i64 0, i64 %98
  %100 = load i8, ptr %99, align 1
  %101 = icmp eq i8 %100, 53
  br label %102

102:                                              ; preds = %96, %90
  %103 = phi i1 [ false, %90 ], [ %101, %96 ]
  %104 = add nuw nsw i32 %37, 2
  %105 = icmp slt i32 %104, %77
  %106 = select i1 %103, i1 %105, i1 false
  br i1 %106, label %107, label %109

107:                                              ; preds = %102
  %108 = sext i32 %77 to i64
  br label %111

109:                                              ; preds = %111, %102
  %110 = phi i1 [ %103, %102 ], [ %115, %111 ]
  br i1 %110, label %119, label %132

111:                                              ; preds = %107, %111
  %112 = phi i64 [ %36, %107 ], [ %116, %111 ]
  %113 = getelementptr inbounds [80 x i8], ptr %7, i64 0, i64 %112
  %114 = load i8, ptr %113, align 1
  %115 = icmp eq i8 %114, 48
  %116 = add nuw nsw i64 %112, 1
  %117 = icmp slt i64 %116, %108
  %118 = select i1 %115, i1 %117, i1 false
  br i1 %118, label %111, label %109

119:                                              ; preds = %109
  %120 = sext i32 %40 to i64
  %121 = call i32 @bcmp(ptr %7, ptr %4, i64 %120)
  %122 = icmp eq i32 %121, 0
  %123 = icmp eq i32 %93, %56
  %124 = select i1 %122, i1 %123, i1 false
  br i1 %124, label %125, label %132

125:                                              ; preds = %119
  call void @llvm.lifetime.start.p0(i64 4, ptr %8)
  store i32 %56, ptr %8, align 4
  call void @llvm.memcpy.p0.p0.i64(ptr align 16 %5, ptr align 16 %4, i64 %120, i1 false)
  call fastcc void @ae.bump(ptr %5, i32 %40, ptr %8, i32 1)
  %126 = load i32, ptr %8, align 4
  call fastcc void @ae.build_e(ptr %3, ptr %5, i32 %40, i32 %126)
  %127 = call double @strtod(ptr %3, ptr null)
  %128 = fcmp oeq double %127, %18
  br i1 %128, label %129, label %130

129:                                              ; preds = %125
  call void @llvm.memcpy.p0.p0.i64(ptr align 16 %4, ptr align 16 %5, i64 %120, i1 false)
  br label %130

130:                                              ; preds = %129, %125
  %131 = phi i32 [ %126, %129 ], [ %56, %125 ]
  call void @llvm.lifetime.end.p0(i64 4, ptr %8)
  br label %132

132:                                              ; preds = %130, %119, %109
  %133 = phi i32 [ %131, %130 ], [ %56, %119 ], [ %56, %109 ]
  call void @llvm.lifetime.end.p0(i64 80, ptr %7)
  call void @llvm.lifetime.end.p0(i64 96, ptr %6)
  br label %208

134:                                              ; preds = %59, %198
  %135 = phi i32 [ -1, %59 ], [ %199, %198 ]
  %136 = phi i32 [ %56, %59 ], [ %196, %198 ]
  call void @llvm.memcpy.p0.p0.i64(ptr align 16 %5, ptr align 16 %4, i64 %60, i1 false)
  %137 = icmp eq i32 %135, 1
  br i1 %137, label %139, label %138

138:                                              ; preds = %134
  br i1 %62, label %159, label %168

139:                                              ; preds = %134
  br i1 %67, label %140, label %149

140:                                              ; preds = %139, %146
  %141 = phi i32 [ %147, %146 ], [ %61, %139 ]
  %142 = zext i32 %141 to i64
  %143 = getelementptr inbounds i8, ptr %5, i64 %142
  %144 = load i8, ptr %143, align 1
  %145 = icmp eq i8 %144, 57
  br i1 %145, label %146, label %149

146:                                              ; preds = %140
  %147 = add nsw i32 %141, -1
  store i8 48, ptr %143, align 1
  %148 = icmp sgt i32 %141, 0
  br i1 %148, label %140, label %149

149:                                              ; preds = %146, %140, %139
  %150 = phi i32 [ %61, %139 ], [ %141, %140 ], [ -1, %146 ]
  %151 = icmp slt i32 %150, 0
  br i1 %151, label %152, label %154

152:                                              ; preds = %149
  call void @llvm.memmove.p0.p0.i64(ptr align 1 %32, ptr align 16 %5, i64 %68, i1 false)
  store i8 49, ptr %5, align 16
  %153 = add nsw i32 %136, 1
  br label %182

154:                                              ; preds = %149
  %155 = zext i32 %150 to i64
  %156 = getelementptr inbounds i8, ptr %5, i64 %155
  %157 = load i8, ptr %156, align 1
  %158 = add i8 %157, 1
  store i8 %158, ptr %156, align 1
  br label %182

159:                                              ; preds = %138, %165
  %160 = phi i32 [ %166, %165 ], [ %61, %138 ]
  %161 = zext i32 %160 to i64
  %162 = getelementptr inbounds i8, ptr %5, i64 %161
  %163 = load i8, ptr %162, align 1
  %164 = icmp eq i8 %163, 48
  br i1 %164, label %165, label %168

165:                                              ; preds = %159
  %166 = add nsw i32 %160, -1
  store i8 57, ptr %162, align 1
  %167 = icmp sgt i32 %160, 0
  br i1 %167, label %159, label %168

168:                                              ; preds = %165, %159, %138
  %169 = phi i32 [ %61, %138 ], [ %160, %159 ], [ -1, %165 ]
  %170 = icmp slt i32 %169, 0
  br i1 %170, label %182, label %171

171:                                              ; preds = %168
  %172 = zext i32 %169 to i64
  %173 = getelementptr inbounds i8, ptr %5, i64 %172
  %174 = load i8, ptr %173, align 1
  %175 = add i8 %174, -1
  store i8 %175, ptr %173, align 1
  %176 = load i8, ptr %5, align 16
  %177 = icmp ne i8 %176, 48
  %178 = select i1 %177, i1 true, i1 %63
  %179 = xor i1 %177, true
  br i1 %178, label %182, label %180

180:                                              ; preds = %171
  call void @llvm.memmove.p0.p0.i64(ptr align 16 %5, ptr align 1 %31, i64 %64, i1 false)
  store i8 57, ptr %66, align 1
  %181 = add nsw i32 %136, -1
  br label %182

182:                                              ; preds = %171, %152, %154, %168, %180
  %183 = phi i32 [ %153, %152 ], [ %136, %154 ], [ %136, %168 ], [ %181, %180 ], [ %136, %171 ]
  %184 = phi i1 [ false, %152 ], [ false, %154 ], [ true, %168 ], [ false, %180 ], [ %179, %171 ]
  br i1 %184, label %195, label %185

185:                                              ; preds = %182
  %186 = load i8, ptr %5, align 16
  store i8 %186, ptr %3, align 16
  br i1 %69, label %187, label %188

187:                                              ; preds = %185
  store i8 46, ptr %33, align 1
  call void @llvm.memcpy.p0.p0.i64(ptr align 2 %34, ptr align 1 %32, i64 %72, i1 false)
  br label %188

188:                                              ; preds = %185, %187
  %189 = phi i64 [ %71, %187 ], [ 1, %185 ]
  %190 = getelementptr inbounds i8, ptr %3, i64 %189
  %191 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %190, i64 16, ptr @.aert.str.18, i32 %183)
  %192 = call double @strtod(ptr %3, ptr null)
  %193 = fcmp oeq double %192, %18
  br i1 %193, label %194, label %195

194:                                              ; preds = %188
  call void @llvm.memcpy.p0.p0.i64(ptr align 16 %4, ptr align 16 %5, i64 %60, i1 false)
  br label %195

195:                                              ; preds = %188, %182, %194
  %196 = phi i32 [ %136, %182 ], [ %183, %194 ], [ %136, %188 ]
  %197 = phi i32 [ 11, %182 ], [ 8, %194 ], [ 0, %188 ]
  switch i32 %197, label %201 [
    i32 0, label %198
    i32 11, label %198
  ]

198:                                              ; preds = %195, %195
  %199 = add nsw i32 %135, 2
  %200 = icmp slt i32 %135, 0
  br i1 %200, label %134, label %201

201:                                              ; preds = %198, %195
  %202 = phi i32 [ %197, %195 ], [ 9, %198 ]
  %203 = icmp eq i32 %202, 9
  br i1 %203, label %204, label %208

204:                                              ; preds = %201
  %205 = add nuw nsw i32 %37, 1
  %206 = icmp eq i32 %205, 17
  %207 = add nuw nsw i64 %36, 1
  br i1 %206, label %208, label %35

208:                                              ; preds = %204, %201, %132
  %209 = phi i32 [ %133, %132 ], [ %196, %201 ], [ %196, %204 ]
  %210 = phi i32 [ 8, %132 ], [ 2, %204 ], [ %202, %201 ]
  switch i32 %210, label %333 [
    i32 2, label %211
    i32 8, label %231
  ]

211:                                              ; preds = %208
  %212 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %3, i64 64, ptr @.aert.str.11, double %18)
  br label %213

213:                                              ; preds = %224, %211
  %214 = phi i32 [ 0, %211 ], [ %225, %224 ]
  %215 = phi ptr [ %3, %211 ], [ %226, %224 ]
  %216 = load i8, ptr %215, align 1
  switch i8 %216, label %217 [
    i8 0, label %227
    i8 101, label %227
  ]

217:                                              ; preds = %213
  %218 = add i8 %216, -48
  %219 = icmp ult i8 %218, 10
  br i1 %219, label %220, label %224

220:                                              ; preds = %217
  %221 = add nsw i32 %214, 1
  %222 = sext i32 %214 to i64
  %223 = getelementptr inbounds i8, ptr %4, i64 %222
  store i8 %216, ptr %223, align 1
  br label %224

224:                                              ; preds = %220, %217
  %225 = phi i32 [ %221, %220 ], [ %214, %217 ]
  %226 = getelementptr inbounds i8, ptr %215, i64 1
  br label %213

227:                                              ; preds = %213, %213
  %228 = getelementptr inbounds i8, ptr %215, i64 1
  %229 = call i64 @strtol(ptr %228, ptr null, i32 10)
  %230 = trunc i64 %229 to i32
  br label %231

231:                                              ; preds = %227, %208
  %232 = phi i32 [ %209, %208 ], [ %230, %227 ]
  %233 = phi i32 [ %40, %208 ], [ %214, %227 ]
  %234 = zext i32 %233 to i64
  %235 = tail call i32 @llvm.smin.i32(i32 %233, i32 1)
  br label %236

236:                                              ; preds = %240, %231
  %237 = phi i64 [ %242, %240 ], [ %234, %231 ]
  %238 = trunc i64 %237 to i32
  %239 = icmp sgt i32 %238, 1
  br i1 %239, label %240, label %248

240:                                              ; preds = %236
  %241 = add nsw i64 %237, -1
  %242 = add nsw i64 %237, -1
  %243 = getelementptr inbounds [24 x i8], ptr %4, i64 0, i64 %241
  %244 = load i8, ptr %243, align 1
  %245 = icmp eq i8 %244, 48
  br i1 %245, label %236, label %246

246:                                              ; preds = %240
  %247 = trunc i64 %237 to i32
  br label %248

248:                                              ; preds = %236, %246
  %249 = phi i32 [ %247, %246 ], [ %235, %236 ]
  %250 = add i32 %232, 1
  %251 = icmp slt i32 %232, 0
  br i1 %251, label %252, label %285

252:                                              ; preds = %248
  %253 = zext i32 %17 to i64
  %254 = getelementptr inbounds i8, ptr %1, i64 %253
  store i8 48, ptr %254, align 1
  %255 = or i32 %17, 2
  %256 = getelementptr i8, ptr %254, i64 1
  store i8 46, ptr %256, align 1
  %257 = icmp eq i32 %232, -1
  br i1 %257, label %273, label %258

258:                                              ; preds = %252
  %259 = xor i32 %232, -1
  %260 = or i32 %17, 2
  %261 = zext i32 %260 to i64
  %262 = getelementptr i8, ptr %1, i64 %261
  %263 = tail call i32 @llvm.smax.i32(i32 %259, i32 1)
  %264 = zext i32 %263 to i64
  tail call void @llvm.memset.p0.i64(ptr align 1 %262, i8 48, i64 %264, i1 false)
  %265 = or i32 %17, 2
  %266 = zext i32 %265 to i64
  %267 = tail call i32 @llvm.smax.i32(i32 %259, i32 1)
  %268 = add nuw i32 %17, %267
  %269 = add nuw i32 %268, 2
  %270 = zext i32 %269 to i64
  br label %281

271:                                              ; preds = %281
  %272 = trunc i64 %283 to i32
  br label %273

273:                                              ; preds = %271, %252
  %274 = phi i32 [ %255, %252 ], [ %272, %271 ]
  %275 = icmp sgt i32 %249, 0
  br i1 %275, label %276, label %333

276:                                              ; preds = %273
  %277 = zext i32 %274 to i64
  %278 = getelementptr i8, ptr %1, i64 %277
  %279 = zext i32 %249 to i64
  call void @llvm.memcpy.p0.p0.i64(ptr align 1 %278, ptr align 16 %4, i64 %279, i1 false)
  %280 = add i32 %274, %249
  br label %333

281:                                              ; preds = %258, %281
  %282 = phi i64 [ %266, %258 ], [ %283, %281 ]
  %283 = add nuw nsw i64 %282, 1
  %284 = icmp eq i64 %283, %270
  br i1 %284, label %271, label %281

285:                                              ; preds = %248
  %286 = icmp slt i32 %250, %249
  br i1 %286, label %294, label %287

287:                                              ; preds = %285
  %288 = icmp sgt i32 %249, 0
  br i1 %288, label %289, label %313

289:                                              ; preds = %287
  %290 = zext i32 %17 to i64
  %291 = getelementptr i8, ptr %1, i64 %290
  %292 = zext i32 %249 to i64
  call void @llvm.memcpy.p0.p0.i64(ptr align 1 %291, ptr align 16 %4, i64 %292, i1 false)
  %293 = add nuw i32 %17, %249
  br label %313

294:                                              ; preds = %285
  %295 = zext i32 %17 to i64
  %296 = getelementptr i8, ptr %1, i64 %295
  %297 = zext i32 %250 to i64
  call void @llvm.memcpy.p0.p0.i64(ptr align 1 %296, ptr align 16 %4, i64 %297, i1 false)
  %298 = add nuw i32 %17, %232
  %299 = add i32 %298, 1
  %300 = zext i32 %299 to i64
  %301 = getelementptr inbounds i8, ptr %1, i64 %300
  store i8 46, ptr %301, align 1
  %302 = add i32 %298, 2
  %303 = zext i32 %302 to i64
  %304 = getelementptr i8, ptr %1, i64 %303
  %305 = zext i32 %250 to i64
  %306 = getelementptr i8, ptr %4, i64 %305
  %307 = add i32 %249, -2
  %308 = sub i32 %307, %232
  %309 = zext i32 %308 to i64
  %310 = add nuw nsw i64 %309, 1
  call void @llvm.memcpy.p0.p0.i64(ptr align 1 %304, ptr align 1 %306, i64 %310, i1 false)
  %311 = zext i32 %302 to i64
  %312 = zext i32 %250 to i64
  br label %324

313:                                              ; preds = %289, %287
  %314 = phi i32 [ %17, %287 ], [ %293, %289 ]
  %315 = icmp sgt i32 %249, %232
  br i1 %315, label %333, label %316

316:                                              ; preds = %313
  %317 = zext i32 %314 to i64
  %318 = getelementptr i8, ptr %1, i64 %317
  %319 = sub i32 %232, %249
  %320 = zext i32 %319 to i64
  %321 = add nuw nsw i64 %320, 1
  tail call void @llvm.memset.p0.i64(ptr align 1 %318, i8 48, i64 %321, i1 false)
  %322 = add i32 %314, %319
  %323 = add i32 %322, 1
  br label %333

324:                                              ; preds = %294, %324
  %325 = phi i64 [ %312, %294 ], [ %327, %324 ]
  %326 = phi i64 [ %311, %294 ], [ %328, %324 ]
  %327 = add nuw nsw i64 %325, 1
  %328 = add nuw nsw i64 %326, 1
  %329 = trunc i64 %327 to i32
  %330 = icmp sgt i32 %249, %329
  br i1 %330, label %324, label %331

331:                                              ; preds = %324
  %332 = trunc i64 %328 to i32
  br label %333

333:                                              ; preds = %316, %331, %276, %313, %273, %208
  %334 = phi i32 [ undef, %208 ], [ %274, %273 ], [ %314, %313 ], [ %280, %276 ], [ %332, %331 ], [ %323, %316 ]
  call void @llvm.lifetime.end.p0(i64 24, ptr %5)
  call void @llvm.lifetime.end.p0(i64 24, ptr %4)
  call void @llvm.lifetime.end.p0(i64 64, ptr %3)
  br label %335

335:                                              ; preds = %333, %26, %20, %10
  %336 = phi i32 [ 3, %10 ], [ %23, %20 ], [ %27, %26 ], [ %334, %333 ]
  ret i32 %336
}

declare double @strtod(ptr, ptr)

define internal fastcc void @ae.bump(ptr %0, i32 %1, ptr %2, i32 %3) unnamed_addr {
  %5 = add nsw i32 %1, -1
  %6 = icmp sgt i32 %3, 0
  %7 = icmp sgt i32 %1, 0
  br i1 %6, label %9, label %8

8:                                                ; preds = %4
  br i1 %7, label %33, label %42

9:                                                ; preds = %4
  br i1 %7, label %10, label %19

10:                                               ; preds = %9, %16
  %11 = phi i32 [ %17, %16 ], [ %5, %9 ]
  %12 = zext i32 %11 to i64
  %13 = getelementptr inbounds i8, ptr %0, i64 %12
  %14 = load i8, ptr %13, align 1
  %15 = icmp eq i8 %14, 57
  br i1 %15, label %16, label %19

16:                                               ; preds = %10
  %17 = add nsw i32 %11, -1
  store i8 48, ptr %13, align 1
  %18 = icmp sgt i32 %11, 0
  br i1 %18, label %10, label %19

19:                                               ; preds = %10, %16, %9
  %20 = phi i32 [ %5, %9 ], [ %11, %10 ], [ -1, %16 ]
  %21 = icmp slt i32 %20, 0
  br i1 %21, label %22, label %28

22:                                               ; preds = %19
  %23 = getelementptr inbounds i8, ptr %0, i64 1
  %24 = sext i32 %1 to i64
  %25 = add nsw i64 %24, -1
  tail call void @llvm.memmove.p0.p0.i64(ptr align 1 %23, ptr align 1 %0, i64 %25, i1 false)
  store i8 49, ptr %0, align 1
  %26 = load i32, ptr %2, align 4
  %27 = add nsw i32 %26, 1
  store i32 %27, ptr %2, align 4
  br label %62

28:                                               ; preds = %19
  %29 = zext i32 %20 to i64
  %30 = getelementptr inbounds i8, ptr %0, i64 %29
  %31 = load i8, ptr %30, align 1
  %32 = add i8 %31, 1
  store i8 %32, ptr %30, align 1
  br label %62

33:                                               ; preds = %8, %39
  %34 = phi i32 [ %40, %39 ], [ %5, %8 ]
  %35 = zext i32 %34 to i64
  %36 = getelementptr inbounds i8, ptr %0, i64 %35
  %37 = load i8, ptr %36, align 1
  %38 = icmp eq i8 %37, 48
  br i1 %38, label %39, label %42

39:                                               ; preds = %33
  %40 = add nsw i32 %34, -1
  store i8 57, ptr %36, align 1
  %41 = icmp sgt i32 %34, 0
  br i1 %41, label %33, label %42

42:                                               ; preds = %33, %39, %8
  %43 = phi i32 [ %5, %8 ], [ %34, %33 ], [ -1, %39 ]
  %44 = icmp slt i32 %43, 0
  br i1 %44, label %62, label %45

45:                                               ; preds = %42
  %46 = zext i32 %43 to i64
  %47 = getelementptr inbounds i8, ptr %0, i64 %46
  %48 = load i8, ptr %47, align 1
  %49 = add i8 %48, -1
  store i8 %49, ptr %47, align 1
  %50 = load i8, ptr %0, align 1
  %51 = icmp ne i8 %50, 48
  %52 = icmp eq i32 %1, 1
  %53 = or i1 %51, %52
  br i1 %53, label %62, label %54

54:                                               ; preds = %45
  %55 = getelementptr inbounds i8, ptr %0, i64 1
  %56 = sext i32 %1 to i64
  %57 = add nsw i64 %56, -1
  tail call void @llvm.memmove.p0.p0.i64(ptr align 1 %0, ptr align 1 %55, i64 %57, i1 false)
  %58 = sext i32 %5 to i64
  %59 = getelementptr inbounds i8, ptr %0, i64 %58
  store i8 57, ptr %59, align 1
  %60 = load i32, ptr %2, align 4
  %61 = add nsw i32 %60, -1
  store i32 %61, ptr %2, align 4
  br label %62

62:                                               ; preds = %28, %22, %54, %45, %42
  ret void
}

define internal fastcc void @ae.build_e(ptr %0, ptr %1, i32 %2, i32 %3) unnamed_addr {
  %5 = load i8, ptr %1, align 1
  store i8 %5, ptr %0, align 1
  %6 = icmp sgt i32 %2, 1
  br i1 %6, label %7, label %22

7:                                                ; preds = %4
  %8 = getelementptr inbounds i8, ptr %0, i64 1
  store i8 46, ptr %8, align 1
  %9 = add nuw i32 %2, 1
  %10 = zext i32 %9 to i64
  br label %13

11:                                               ; preds = %13
  %12 = and i64 %18, 4294967295
  br label %22

13:                                               ; preds = %7, %13
  %14 = phi i64 [ 2, %7 ], [ %18, %13 ]
  %15 = phi i64 [ 1, %7 ], [ %20, %13 ]
  %16 = getelementptr inbounds i8, ptr %1, i64 %15
  %17 = load i8, ptr %16, align 1
  %18 = add nuw nsw i64 %14, 1
  %19 = getelementptr inbounds i8, ptr %0, i64 %14
  store i8 %17, ptr %19, align 1
  %20 = add nuw nsw i64 %15, 1
  %21 = icmp eq i64 %18, %10
  br i1 %21, label %11, label %13

22:                                               ; preds = %11, %4
  %23 = phi i64 [ %12, %11 ], [ 1, %4 ]
  %24 = getelementptr inbounds i8, ptr %0, i64 %23
  %25 = tail call i32 (ptr, i64, ptr, ...) @snprintf(ptr %24, i64 16, ptr @.aert.str.18, i32 %3)
  ret void
}

define internal void @ae.print(ptr %0) {
  %2 = getelementptr inbounds %ae.Str, ptr %0, i64 0, i32 1
  %3 = load i64, ptr %0, align 8
  %4 = load ptr, ptr @stdout, align 8
  %5 = tail call i64 @fwrite(ptr %2, i64 1, i64 %3, ptr %4)
  ret void
}

declare i64 @fwrite(ptr, i64, i64, ptr)

define internal void @ae.println(ptr %0) {
  %2 = getelementptr inbounds %ae.Str, ptr %0, i64 0, i32 1
  %3 = load i64, ptr %0, align 8
  %4 = load ptr, ptr @stdout, align 8
  %5 = tail call i64 @fwrite(ptr %2, i64 1, i64 %3, ptr %4)
  %6 = load ptr, ptr @stdout, align 8
  %7 = tail call i32 @fputc(i32 10, ptr %6)
  ret void
}

define internal void @ae.print_i32(i32 %0) {
  %2 = tail call i32 (ptr, ...) @printf(ptr @.aert.str.12, i32 %0)
  ret void
}

declare i32 @printf(ptr, ...)

define internal void @ae.print_i64(i64 %0) {
  %2 = tail call i32 (ptr, ...) @printf(ptr @.aert.str.13, i64 %0)
  ret void
}

define internal void @ae.print_f64(double %0) {
  %2 = alloca [408 x i8], align 16
  call void @llvm.lifetime.start.p0(i64 408, ptr %2)
  %3 = call i32 @ae.fmt_f64(double %0, ptr %2)
  %4 = sext i32 %3 to i64
  %5 = getelementptr inbounds [408 x i8], ptr %2, i64 0, i64 %4
  store i8 10, ptr %5, align 1
  %6 = add nsw i64 %4, 1
  %7 = load ptr, ptr @stdout, align 8
  %8 = call i64 @fwrite(ptr %2, i64 1, i64 %6, ptr %7)
  call void @llvm.lifetime.end.p0(i64 408, ptr %2)
  ret void
}

define internal void @ae.print_bool(i1 %0) {
  %2 = select i1 %0, ptr @.aert.str.14, ptr @.aert.str.15
  %3 = load ptr, ptr @stdout, align 8
  %4 = tail call i32 @fputs(ptr %2, ptr %3)
  ret void
}

define internal void @ae.print_char(i32 %0) {
  %2 = alloca [8 x i8], align 1
  call void @llvm.lifetime.start.p0(i64 8, ptr %2)
  %3 = icmp ult i32 %0, 128
  br i1 %3, label %4, label %6

4:                                                ; preds = %1
  %5 = trunc i32 %0 to i8
  store i8 %5, ptr %2, align 1
  br label %48

6:                                                ; preds = %1
  %7 = icmp ult i32 %0, 2048
  br i1 %7, label %8, label %16

8:                                                ; preds = %6
  %9 = lshr i32 %0, 6
  %10 = trunc i32 %9 to i8
  %11 = or i8 %10, -64
  store i8 %11, ptr %2, align 1
  %12 = trunc i32 %0 to i8
  %13 = and i8 %12, 63
  %14 = or i8 %13, -128
  %15 = getelementptr inbounds i8, ptr %2, i64 1
  store i8 %14, ptr %15, align 1
  br label %48

16:                                               ; preds = %6
  %17 = icmp ult i32 %0, 65536
  %18 = getelementptr inbounds i8, ptr %2, i64 1
  br i1 %17, label %19, label %31

19:                                               ; preds = %16
  %20 = lshr i32 %0, 12
  %21 = trunc i32 %20 to i8
  %22 = or i8 %21, -32
  store i8 %22, ptr %2, align 1
  %23 = lshr i32 %0, 6
  %24 = trunc i32 %23 to i8
  %25 = and i8 %24, 63
  %26 = or i8 %25, -128
  store i8 %26, ptr %18, align 1
  %27 = trunc i32 %0 to i8
  %28 = and i8 %27, 63
  %29 = or i8 %28, -128
  %30 = getelementptr inbounds i8, ptr %2, i64 2
  store i8 %29, ptr %30, align 1
  br label %48

31:                                               ; preds = %16
  %32 = lshr i32 %0, 18
  %33 = trunc i32 %32 to i8
  %34 = or i8 %33, -16
  store i8 %34, ptr %2, align 1
  %35 = lshr i32 %0, 12
  %36 = trunc i32 %35 to i8
  %37 = and i8 %36, 63
  %38 = or i8 %37, -128
  store i8 %38, ptr %18, align 1
  %39 = lshr i32 %0, 6
  %40 = trunc i32 %39 to i8
  %41 = and i8 %40, 63
  %42 = or i8 %41, -128
  %43 = getelementptr inbounds i8, ptr %2, i64 2
  store i8 %42, ptr %43, align 1
  %44 = trunc i32 %0 to i8
  %45 = and i8 %44, 63
  %46 = or i8 %45, -128
  %47 = getelementptr inbounds i8, ptr %2, i64 3
  store i8 %46, ptr %47, align 1
  br label %48

48:                                               ; preds = %4, %8, %19, %31
  %49 = phi i64 [ 1, %4 ], [ 2, %8 ], [ 3, %19 ], [ 4, %31 ]
  %50 = getelementptr inbounds [8 x i8], ptr %2, i64 0, i64 %49
  store i8 10, ptr %50, align 1
  %51 = add nuw nsw i64 %49, 1
  %52 = load ptr, ptr @stdout, align 8
  %53 = call i64 @fwrite(ptr %2, i64 1, i64 %51, ptr %52)
  call void @llvm.lifetime.end.p0(i64 8, ptr %2)
  ret void
}

define internal ptr @ae.to_string(i32 %0) {
  %2 = alloca [32 x i8], align 16
  call void @llvm.lifetime.start.p0(i64 32, ptr %2)
  %3 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %2, i64 32, ptr @.aert.str.16, i32 %0)
  %4 = sext i32 %3 to i64
  %5 = add nsw i64 %4, 9
  %6 = tail call ptr @malloc(i64 %5)
  %7 = icmp eq ptr %6, null
  br i1 %7, label %8, label %9

8:                                                ; preds = %1
  tail call void @ae.rt_error(ptr @.aert.str.5)
  unreachable

9:                                                ; preds = %1
  store i64 %4, ptr %6, align 8
  %10 = getelementptr inbounds %ae.Str, ptr %6, i64 0, i32 1, i64 %4
  store i8 0, ptr %10, align 1
  %11 = getelementptr inbounds %ae.Str, ptr %6, i64 0, i32 1
  call void @llvm.memcpy.p0.p0.i64(ptr align 8 %11, ptr align 16 %2, i64 %4, i1 false)
  call void @llvm.lifetime.end.p0(i64 32, ptr %2)
  ret ptr %6
}

define internal ptr @ae.i64_to_string(i64 %0) {
  %2 = alloca [32 x i8], align 16
  call void @llvm.lifetime.start.p0(i64 32, ptr %2)
  %3 = call i32 (ptr, i64, ptr, ...) @snprintf(ptr %2, i64 32, ptr @.aert.str.17, i64 %0)
  %4 = sext i32 %3 to i64
  %5 = add nsw i64 %4, 9
  %6 = tail call ptr @malloc(i64 %5)
  %7 = icmp eq ptr %6, null
  br i1 %7, label %8, label %9

8:                                                ; preds = %1
  tail call void @ae.rt_error(ptr @.aert.str.5)
  unreachable

9:                                                ; preds = %1
  store i64 %4, ptr %6, align 8
  %10 = getelementptr inbounds %ae.Str, ptr %6, i64 0, i32 1, i64 %4
  store i8 0, ptr %10, align 1
  %11 = getelementptr inbounds %ae.Str, ptr %6, i64 0, i32 1
  call void @llvm.memcpy.p0.p0.i64(ptr align 8 %11, ptr align 16 %2, i64 %4, i1 false)
  call void @llvm.lifetime.end.p0(i64 32, ptr %2)
  ret ptr %6
}

define internal ptr @ae.f64_to_string(double %0) {
  %2 = alloca [408 x i8], align 16
  call void @llvm.lifetime.start.p0(i64 408, ptr %2)
  %3 = call i32 @ae.fmt_f64(double %0, ptr %2)
  %4 = sext i32 %3 to i64
  %5 = add nsw i64 %4, 9
  %6 = tail call ptr @malloc(i64 %5)
  %7 = icmp eq ptr %6, null
  br i1 %7, label %8, label %9

8:                                                ; preds = %1
  tail call void @ae.rt_error(ptr @.aert.str.5)
  unreachable

9:                                                ; preds = %1
  store i64 %4, ptr %6, align 8
  %10 = getelementptr inbounds %ae.Str, ptr %6, i64 0, i32 1, i64 %4
  store i8 0, ptr %10, align 1
  %11 = getelementptr inbounds %ae.Str, ptr %6, i64 0, i32 1
  call void @llvm.memcpy.p0.p0.i64(ptr align 8 %11, ptr align 16 %2, i64 %4, i1 false)
  call void @llvm.lifetime.end.p0(i64 408, ptr %2)
  ret ptr %6
}

define internal ptr @ae.char_to_string(i32 %0) {
  %2 = alloca i64, align 8
  call void @llvm.lifetime.start.p0(i64 8, ptr %2)
  %3 = icmp ult i32 %0, 128
  br i1 %3, label %4, label %6

4:                                                ; preds = %1
  %5 = trunc i32 %0 to i8
  store i8 %5, ptr %2, align 8
  br label %49

6:                                                ; preds = %1
  %7 = icmp ult i32 %0, 2048
  br i1 %7, label %8, label %16

8:                                                ; preds = %6
  %9 = lshr i32 %0, 6
  %10 = trunc i32 %9 to i8
  %11 = or i8 %10, -64
  store i8 %11, ptr %2, align 8
  %12 = trunc i32 %0 to i8
  %13 = and i8 %12, 63
  %14 = or i8 %13, -128
  %15 = getelementptr inbounds i8, ptr %2, i64 1
  store i8 %14, ptr %15, align 1
  br label %49

16:                                               ; preds = %6
  %17 = icmp ult i32 %0, 65536
  br i1 %17, label %18, label %31

18:                                               ; preds = %16
  %19 = lshr i32 %0, 12
  %20 = trunc i32 %19 to i8
  %21 = or i8 %20, -32
  store i8 %21, ptr %2, align 8
  %22 = lshr i32 %0, 6
  %23 = trunc i32 %22 to i8
  %24 = and i8 %23, 63
  %25 = or i8 %24, -128
  %26 = getelementptr inbounds i8, ptr %2, i64 1
  store i8 %25, ptr %26, align 1
  %27 = trunc i32 %0 to i8
  %28 = and i8 %27, 63
  %29 = or i8 %28, -128
  %30 = getelementptr inbounds i8, ptr %2, i64 2
  store i8 %29, ptr %30, align 2
  br label %49

31:                                               ; preds = %16
  %32 = lshr i32 %0, 18
  %33 = trunc i32 %32 to i8
  %34 = or i8 %33, -16
  store i8 %34, ptr %2, align 8
  %35 = lshr i32 %0, 12
  %36 = trunc i32 %35 to i8
  %37 = and i8 %36, 63
  %38 = or i8 %37, -128
  %39 = getelementptr inbounds i8, ptr %2, i64 1
  store i8 %38, ptr %39, align 1
  %40 = lshr i32 %0, 6
  %41 = trunc i32 %40 to i8
  %42 = and i8 %41, 63
  %43 = or i8 %42, -128
  %44 = getelementptr inbounds i8, ptr %2, i64 2
  store i8 %43, ptr %44, align 2
  %45 = trunc i32 %0 to i8
  %46 = and i8 %45, 63
  %47 = or i8 %46, -128
  %48 = getelementptr inbounds i8, ptr %2, i64 3
  store i8 %47, ptr %48, align 1
  br label %49

49:                                               ; preds = %4, %8, %18, %31
  %50 = phi i64 [ 1, %4 ], [ 2, %8 ], [ 3, %18 ], [ 4, %31 ]
  %51 = add nuw nsw i64 %50, 9
  %52 = tail call ptr @malloc(i64 %51)
  %53 = icmp eq ptr %52, null
  br i1 %53, label %54, label %55

54:                                               ; preds = %49
  tail call void @ae.rt_error(ptr @.aert.str.5)
  unreachable

55:                                               ; preds = %49
  store i64 %50, ptr %52, align 8
  %56 = getelementptr inbounds %ae.Str, ptr %52, i64 0, i32 1, i64 %50
  store i8 0, ptr %56, align 1
  %57 = getelementptr inbounds %ae.Str, ptr %52, i64 0, i32 1
  call void @llvm.memcpy.p0.p0.i64(ptr align 8 %57, ptr align 8 %2, i64 %50, i1 false)
  call void @llvm.lifetime.end.p0(i64 8, ptr %2)
  ret ptr %52
}

define internal i32 @ae.abs(i32 %0) {
  %2 = tail call i32 @llvm.abs.i32(i32 %0, i1 false)
  ret i32 %2
}

define internal i32 @ae.min(i32 %0, i32 %1) {
  %3 = tail call i32 @llvm.smin.i32(i32 %0, i32 %1)
  ret i32 %3
}

define internal i32 @ae.max(i32 %0, i32 %1) {
  %3 = tail call i32 @llvm.smax.i32(i32 %0, i32 %1)
  ret i32 %3
}

define internal i32 @ae.clamp(i32 %0, i32 %1, i32 %2) {
  %4 = tail call i32 @llvm.smin.i32(i32 %2, i32 %0)
  %5 = tail call i32 @llvm.smax.i32(i32 %1, i32 %4)
  ret i32 %5
}

define internal double @ae.sqrt(double %0) {
  %2 = tail call double @llvm.sqrt.f64(double %0)
  ret double %2
}

declare double @llvm.sqrt.f64(double)

define internal double @ae.floor(double %0) {
  %2 = tail call double @llvm.floor.f64(double %0)
  ret double %2
}

declare double @llvm.floor.f64(double)

define internal double @ae.ceil(double %0) {
  %2 = tail call double @llvm.ceil.f64(double %0)
  ret double %2
}

declare double @llvm.ceil.f64(double)

define internal i32 @ae.pow_i32(i32 %0, i32 %1) {
  %3 = icmp slt i32 %1, 0
  br i1 %3, label %17, label %4

4:                                                ; preds = %2
  %5 = icmp eq i32 %1, 0
  br i1 %5, label %17, label %6

6:                                                ; preds = %4, %6
  %7 = phi i32 [ %15, %6 ], [ %1, %4 ]
  %8 = phi i32 [ %14, %6 ], [ %0, %4 ]
  %9 = phi i32 [ %13, %6 ], [ 1, %4 ]
  %10 = and i32 %7, 1
  %11 = icmp eq i32 %10, 0
  %12 = select i1 %11, i32 1, i32 %8
  %13 = mul i32 %12, %9
  %14 = mul i32 %8, %8
  %15 = lshr i32 %7, 1
  %16 = icmp ult i32 %7, 2
  br i1 %16, label %17, label %6

17:                                               ; preds = %6, %4, %2
  %18 = phi i32 [ 0, %2 ], [ 1, %4 ], [ %13, %6 ]
  ret i32 %18
}

declare i64 @strtol(ptr, ptr, i32)

declare void @llvm.memmove.p0.p0.i64(ptr, ptr, i64, i1)

declare i32 @bcmp(ptr, ptr, i64)

declare i64 @llvm.smin.i64(i64, i64)

declare i32 @llvm.abs.i32(i32, i1)

declare i32 @llvm.smin.i32(i32, i32)

declare i32 @llvm.smax.i32(i32, i32)

declare void @llvm.memset.p0.i64(ptr, i8, i64, i1)
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::{compile_source, CompileOptions};

    fn llvm_of(name: &str, src: &str, opt_level: u8) -> String {
        let c = compile_source(
            name,
            src,
            &CompileOptions {
                opt_level,
                color: false,
            },
        );
        assert!(c.ok(), "{name} failed to compile");
        c.llvm.clone().unwrap_or_default()
    }

    fn example_sources() -> Vec<(String, String)> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/examples");
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .expect("examples directory")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map_or(false, |x| x == "ae"))
            .collect();
        files.sort();
        files
            .into_iter()
            .map(|p| {
                let src = std::fs::read_to_string(&p).expect("read example");
                (p.display().to_string(), src)
            })
            .collect()
    }

    /// Feed `ir` to `llvm-as`; returns its stderr on failure.
    fn assemble(tool: &str, ir: &str) -> Result<(), String> {
        use std::io::Write;
        let mut child = Command::new(tool)
            .arg("--disable-output")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(ir.as_bytes()).map_err(|e| e.to_string())?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }

    #[test]
    fn emits_define_main() {
        let src = include_str!("../../examples/hello.ae");
        let ll = llvm_of("hello.ae", src, 0);
        assert!(ll.contains("define i32 @main()"));
        assert!(ll.contains("@ae.user.main"));
        assert!(ll.contains("@ae.println"));
        assert!(ll.contains("c\"hello, aether\\00\""));
        assert!(!ll.contains("UNSUPPORTED"));
    }

    #[test]
    fn registers_use_alloca_load_store() {
        let src = "fn main() -> i32 { let mut x = 1; x = x + 2; return x; }";
        let ll = llvm_of("t.ae", src, 0);
        assert!(ll.contains("entry:"));
        assert!(ll.contains("= alloca i32"));
        assert!(ll.contains("br label %bb0"));
        assert!(ll.contains("= load i32, ptr %r"));
        assert!(ll.contains("store i32"));
        assert!(ll.contains("add i32"));
    }

    #[test]
    fn aggregates_are_inline_values() {
        let src = "struct P { x: i32, y: f64 }
            fn bump(p: P) -> P { let mut q = p; q.x = q.x + 1; return q; }
            fn main() -> i32 {
                let p = P { x: 1, y: 2.5 };
                let a: [i32; 3] = [1, 2, 3];
                let b = a[1] + bump(p).x;
                let s = \"hi\";
                if s == \"hi\" { print_f64(p.y); }
                return b;
            }";
        let ll = llvm_of("t.ae", src, 0);
        assert!(ll.contains("%\"T.P\" = type { i32, double }"));
        assert!(ll.contains("alloca %\"T.P\""));
        assert!(ll.contains("alloca [3 x i32]"));
        assert!(ll.contains("@llvm.memcpy.p0.p0.i64"));
        assert!(ll.contains("define void @bump(ptr %ret, ptr %p0)"));
        assert!(ll.contains("@ae.index_oob"));
        assert!(ll.contains("@ae.str_eq"));
        assert!(ll.contains("<{ i64 2, [3 x i8] c\"hi\\00\" }>"));
    }

    #[test]
    fn enum_slots_with_mixed_types_are_byte_buffers() {
        let e = Type::Enum {
            name: "E".into(),
            variants: vec![
                ("A".into(), vec![Type::I32, Type::F64]),
                ("B".into(), vec![Type::Bool, Type::String]),
                ("C".into(), vec![]),
            ],
        };
        assert_eq!(enum_slots(&e), vec!["i32", "[1 x i32]", "[1 x i64]"]);
        assert_eq!(layout(&e), (16, 8));
        let t = Type::Tuple(vec![Type::Bool, Type::I64, Type::I32]);
        assert_eq!(layout(&t), (24, 8));
    }

    #[test]
    fn user_functions_never_clash_with_the_runtime() {
        let src = "fn malloc(x: i32) -> i32 { return x; }
            fn abs(x: i32) -> i32 { return 7; }
            fn main() -> i32 { print_i32(abs(malloc(3))); return 0; }";
        let ll = llvm_of("t.ae", src, 0);
        assert!(ll.contains("define i32 @malloc.ae(i32 %p0)"));
        assert!(ll.contains("define i32 @abs(i32 %p0)"));
        assert!(ll.contains("call i32 @abs("));
    }

    /// When `llvm-as` is installed, every example must assemble at -O0 and -O2.
    /// CI has no LLVM, so the test passes trivially there.
    #[test]
    fn examples_assemble_with_llvm_as() {
        let Some(tool) = find_tool("llvm-as").or_else(|| find_tool("llvm-as-18")) else {
            return;
        };
        for (name, src) in example_sources() {
            for level in [0u8, 2u8] {
                let ll = llvm_of(&name, &src, level);
                if let Err(e) = assemble(&tool, &ll) {
                    panic!("llvm-as rejected {name} at -O{level}:\n{e}\n{ll}");
                }
            }
        }
    }
}
