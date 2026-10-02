//! Textual LLVM IR emitter.
//!
//! We emit LLVM 15+ compatible IR (opaque pointers) and shell out to
//! `opt`/`lli` when those tools are present. Linking against libLLVM is
//! deliberately avoided so the compiler stays portable.
//!
//! Aether IR is not SSA: a virtual register may be assigned any number of
//! times. We therefore emit the classic mem2reg-ready form: every register
//! gets one `alloca` in the `entry` block, every use is a `load` into a fresh
//! SSA temporary and every definition is a `store`. `opt -O2` (or just
//! `-passes=mem2reg`) turns this back into proper SSA.
//!
//! Type mapping: `i32`/`i64` map 1:1, `f64` -> `double`, `bool` -> `i1`,
//! `char` -> `i32` (Unicode code point), `string` -> `ptr` to a NUL-terminated
//! private constant, `unit` has no slot. Arrays and structs live in their own
//! `alloca` where the IR allocates them; the register slot only holds the
//! pointer.
//!
//! Known differences from the VM:
//! * Aggregates have reference semantics here: a `Move` of an array or struct
//!   copies the pointer, so two locals may alias where the VM would copy.
//!   Element/field *stores* of aggregates do copy the contents.
//! * Indexing emits a plain `getelementptr` with no bounds check.
//! * Aggregate `==`/`!=` is a `memcmp` of the storage, so uninitialized
//!   padding bytes in a struct can make two equal values compare unequal.
//! * Returning an aggregate copies it into `malloc`ed memory that is never
//!   freed.
//! * `print_f64` uses `%g`, so float formatting differs slightly from the
//!   VM's Rust `Display`.
//! * String concatenation (`string + string`) is not supported; the emitted
//!   program aborts at that point.
//! * `AllocArray`/`AllocStruct` emit the `alloca` where the IR allocates, so an
//!   allocation inside a loop grows the stack frame on every iteration.

use crate::ast::{BinOp, UnOp};
use crate::ir::{ConstValue, Inst, IrFunction, IrModule, Reg, Terminator};
use crate::ty::Type;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::{Command, Stdio};

/// Natives provided by the runtime, as `(name, param types, return type)`.
fn native_sig(name: &str) -> Option<(Vec<Type>, Type)> {
    Some(match name {
        "print" | "println" => (vec![Type::String], Type::Unit),
        "print_i32" => (vec![Type::I32], Type::Unit),
        "print_i64" => (vec![Type::I64], Type::Unit),
        "print_f64" => (vec![Type::F64], Type::Unit),
        "print_bool" | "assert" => (vec![Type::Bool], Type::Unit),
        "len" => (vec![Type::String], Type::I32),
        _ => return None,
    })
}

fn is_builtin(name: &str) -> bool {
    native_sig(name).is_some()
}

/// C symbols the prelude declares; a user function with one of these names is
/// renamed so the module does not define a symbol twice.
const RESERVED: &[&str] = &["printf", "puts", "strcmp", "strlen", "memcmp", "malloc", "abort"];

/// Module-wide emission state shared by all functions.
struct ModuleCtx<'a> {
    module: &'a IrModule,
    /// Interned string literals, in order of first appearance.
    strings: Vec<String>,
    /// `declare` lines for callees the module knows nothing about.
    extern_decls: BTreeMap<String, String>,
}

pub fn emit_llvm_ir(module: &IrModule) -> String {
    let mut cx = ModuleCtx {
        module,
        strings: Vec::new(),
        extern_decls: BTreeMap::new(),
    };

    let mut body = String::new();
    for f in &module.functions {
        if is_builtin(&f.name) {
            continue;
        }
        if f.is_extern {
            emit_declare(&mut body, f);
            continue;
        }
        emit_function(&mut body, f, &mut cx);
    }

    let mut out = String::new();
    out.push_str("; ModuleID = 'aether'\n");
    out.push_str("source_filename = \"aether\"\n");
    out.push_str("target datalayout = \"e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128\"\n");
    out.push_str("target triple = \"x86_64-pc-linux-gnu\"\n\n");

    for (name, ty) in &module.structs {
        if let Type::Struct { fields, .. } = ty {
            let parts: Vec<String> = fields.iter().map(|(_, t)| llvm_ty(t)).collect();
            let _ = writeln!(out, "{} = type {{ {} }}", struct_name(name), parts.join(", "));
        }
    }
    if !module.structs.is_empty() {
        out.push('\n');
    }

    out.push_str("declare i32 @printf(ptr, ...)\n");
    out.push_str("declare i32 @puts(ptr)\n");
    out.push_str("declare i32 @strcmp(ptr, ptr)\n");
    out.push_str("declare i32 @memcmp(ptr, ptr, i64)\n");
    out.push_str("declare i64 @strlen(ptr)\n");
    out.push_str("declare ptr @malloc(i64)\n");
    out.push_str("declare void @abort() noreturn\n");
    for decl in cx.extern_decls.values() {
        out.push_str(decl);
        out.push('\n');
    }
    out.push('\n');

    global_str(&mut out, "@.fmt.s", "%s");
    global_str(&mut out, "@.fmt.sn", "%s\n");
    global_str(&mut out, "@.fmt.d", "%d\n");
    global_str(&mut out, "@.fmt.lld", "%lld\n");
    global_str(&mut out, "@.fmt.g", "%g\n");
    global_str(&mut out, "@.str.true", "true");
    global_str(&mut out, "@.str.false", "false");
    for (i, s) in cx.strings.iter().enumerate() {
        global_str(&mut out, &format!("@.str.{i}"), s);
    }
    out.push('\n');

    out.push_str(&body);
    out.push_str(NATIVE_WRAPPERS);
    out
}

/// Runtime natives, defined as thin wrappers over libc.
const NATIVE_WRAPPERS: &str = r#"define void @print(ptr %s) {
entry:
  %t0 = call i32 (ptr, ...) @printf(ptr @.fmt.s, ptr %s)
  ret void
}

define void @println(ptr %s) {
entry:
  %t0 = call i32 (ptr, ...) @printf(ptr @.fmt.sn, ptr %s)
  ret void
}

define void @print_i32(i32 %v) {
entry:
  %t0 = call i32 (ptr, ...) @printf(ptr @.fmt.d, i32 %v)
  ret void
}

define void @print_i64(i64 %v) {
entry:
  %t0 = call i32 (ptr, ...) @printf(ptr @.fmt.lld, i64 %v)
  ret void
}

define void @print_f64(double %v) {
entry:
  %t0 = call i32 (ptr, ...) @printf(ptr @.fmt.g, double %v)
  ret void
}

define void @print_bool(i1 %v) {
entry:
  %t0 = select i1 %v, ptr @.str.true, ptr @.str.false
  %t1 = call i32 @puts(ptr %t0)
  ret void
}

define i32 @len(ptr %s) {
entry:
  %t0 = call i64 @strlen(ptr %s)
  %t1 = trunc i64 %t0 to i32
  ret i32 %t1
}

define void @assert(i1 %c) {
entry:
  br i1 %c, label %ok, label %fail
ok:
  ret void
fail:
  call void @abort()
  unreachable
}
"#;

/// `@name = private unnamed_addr constant [N x i8] c"..."` with `N` computed
/// from the actual byte length (plus the NUL terminator).
fn global_str(out: &mut String, name: &str, s: &str) {
    let _ = writeln!(
        out,
        "{name} = private unnamed_addr constant [{n} x i8] c\"{bytes}\"",
        n = s.len() + 1,
        bytes = llvm_string_bytes(s)
    );
}

fn emit_declare(out: &mut String, f: &IrFunction) {
    let params: Vec<String> = f.params.iter().map(|(_, t, _)| slot_ty(t)).collect();
    let _ = writeln!(
        out,
        "declare {} @{}({})\n",
        llvm_ret(&f.return_ty),
        fn_symbol(&f.name),
        params.join(", ")
    );
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

    /// LLVM type of the register's stack slot. Untyped registers default to
    /// `i32` so references to them still assemble.
    fn slot(&self, r: Reg) -> String {
        match self.ty(r) {
            Some(t) => slot_ty(t),
            None => "i32".into(),
        }
    }

    fn is_unit(&self, r: Reg) -> bool {
        matches!(self.ty(r), Some(Type::Unit))
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
    // block order (loops), so iterate to a fixpoint.
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
                    | Inst::Nop => continue,
                };
                let slot = &mut info.types[dest.0 as usize];
                if slot.is_none() && ty.is_some() {
                    *slot = ty;
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

fn callee_params(cx: &ModuleCtx, name: &str) -> Option<Vec<Type>> {
    if let Some(f) = cx.module.function(name) {
        return Some(f.params.iter().map(|(_, t, _)| t.clone()).collect());
    }
    native_sig(name).map(|(params, _)| params)
}

// ---------------------------------------------------------------------------
// Function emission
// ---------------------------------------------------------------------------

/// Per-function emission state.
struct FnCtx<'a, 'm> {
    out: String,
    regs: RegInfo,
    next_tmp: u32,
    cx: &'a mut ModuleCtx<'m>,
}

impl FnCtx<'_, '_> {
    fn tmp(&mut self) -> String {
        let t = format!("%t{}", self.next_tmp);
        self.next_tmp += 1;
        t
    }

    fn line(&mut self, s: &str) {
        self.out.push_str("  ");
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// Load register `r` into a fresh temporary; returns the temporary.
    fn load(&mut self, r: Reg) -> String {
        let t = self.tmp();
        let ty = self.regs.slot(r);
        self.line(&format!("{t} = load {ty}, ptr %r{}", r.0));
        t
    }

    /// Store `value` (already typed as the slot type) into register `r`.
    fn store(&mut self, r: Reg, value: &str) {
        if self.regs.is_unit(r) {
            return;
        }
        let ty = self.regs.slot(r);
        self.line(&format!("store {ty} {value}, ptr %r{}", r.0));
    }

    fn intern(&mut self, s: &str) -> usize {
        if let Some(i) = self.cx.strings.iter().position(|x| x == s) {
            i
        } else {
            self.cx.strings.push(s.to_string());
            self.cx.strings.len() - 1
        }
    }

    /// Pointer to element `index` of the array held in `base`.
    fn index_ptr(&mut self, base: Reg, index: Reg, elem: &Type) -> String {
        let b = self.load(base);
        let i = self.load(index);
        let p = self.tmp();
        match self.regs.ty(base).cloned() {
            Some(arr @ Type::Array { .. }) => {
                let aty = llvm_ty(&arr);
                self.line(&format!("{p} = getelementptr {aty}, ptr {b}, i32 0, i32 {i}"));
            }
            // Base type unknown: index as a flat run of elements.
            _ => {
                let ety = llvm_ty(elem);
                self.line(&format!("{p} = getelementptr {ety}, ptr {b}, i32 {i}"));
            }
        }
        p
    }

    /// Pointer to field `index` of the struct held in `base`.
    fn field_ptr(&mut self, base: Reg, index: usize, field_ty: &Type) -> String {
        let b = self.load(base);
        let p = self.tmp();
        match self.regs.ty(base).cloned() {
            Some(st @ Type::Struct { .. }) => {
                let sty = llvm_ty(&st);
                self.line(&format!("{p} = getelementptr {sty}, ptr {b}, i32 0, i32 {index}"));
            }
            // Base type unknown: treat the storage as consecutive fields.
            _ => {
                let fty = llvm_ty(field_ty);
                self.line(&format!("{p} = getelementptr {fty}, ptr {b}, i32 {index}"));
            }
        }
        p
    }

    /// Store the value in register `value` (of type `ty`) through pointer `p`.
    /// Aggregates are copied by value; scalars are stored directly.
    fn store_through(&mut self, p: &str, value: Reg, ty: &Type) {
        let v = self.load(value);
        if is_aggregate(ty) {
            let aty = llvm_ty(ty);
            let tmp = self.tmp();
            self.line(&format!("{tmp} = load {aty}, ptr {v}"));
            self.line(&format!("store {aty} {tmp}, ptr {p}"));
        } else {
            self.line(&format!("store {} {v}, ptr {p}", llvm_ty(ty)));
        }
    }

    /// Load the value of type `ty` behind pointer `p` into register `dest`.
    /// For aggregates the register receives the pointer itself.
    fn load_through(&mut self, p: &str, dest: Reg, ty: &Type) {
        if is_aggregate(ty) {
            self.store(dest, p);
        } else {
            let tmp = self.tmp();
            self.line(&format!("{tmp} = load {}, ptr {p}", llvm_ty(ty)));
            self.store(dest, &tmp);
        }
    }
}

fn emit_function(out: &mut String, f: &IrFunction, cx: &mut ModuleCtx<'_>) {
    let regs = infer_regs(f, cx);
    let mut fx = FnCtx {
        out: String::new(),
        regs,
        next_tmp: 0,
        cx,
    };

    let params: Vec<String> = f
        .params
        .iter()
        .enumerate()
        .map(|(i, (_, t, _))| format!("{} %p{i}", slot_ty(t)))
        .collect();
    let _ = writeln!(
        fx.out,
        "define {} @{}({}) {{",
        llvm_ret(&f.return_ty),
        fn_symbol(&f.name),
        params.join(", ")
    );

    // entry: one alloca per live register, parameters spilled, jump to bb0.
    fx.out.push_str("entry:\n");
    for i in 0..fx.regs.types.len() {
        let r = Reg(i as u32);
        if !fx.regs.referenced[i] || fx.regs.is_unit(r) {
            continue;
        }
        let ty = fx.regs.slot(r);
        fx.line(&format!("%r{i} = alloca {ty}"));
    }
    for (i, (_, _, r)) in f.params.iter().enumerate() {
        let p = format!("%p{i}");
        fx.store(*r, &p);
    }
    match f.blocks.first() {
        Some(bb) => fx.line(&format!("br label %bb{}", bb.id.0)),
        None => fx.line(&default_return(&f.return_ty)),
    }

    for bb in &f.blocks {
        let _ = writeln!(fx.out, "bb{}:", bb.id.0);
        for inst in &bb.insts {
            emit_inst(&mut fx, inst);
        }
        emit_term(&mut fx, &bb.term, &f.return_ty);
    }
    fx.out.push_str("}\n\n");
    out.push_str(&fx.out);
}

fn emit_inst(fx: &mut FnCtx, inst: &Inst) {
    if let Some(d) = inst.dest_reg() {
        if fx.regs.is_unit(d) {
            return;
        }
    }
    match inst {
        Inst::LoadConst { dest, value } => match value {
            ConstValue::I32(v) => fx.store(*dest, &v.to_string()),
            ConstValue::I64(v) => fx.store(*dest, &v.to_string()),
            ConstValue::F64(v) => fx.store(*dest, &llvm_f64(*v)),
            ConstValue::Bool(v) => fx.store(*dest, if *v { "true" } else { "false" }),
            ConstValue::String(s) => {
                let idx = fx.intern(s);
                fx.store(*dest, &format!("@.str.{idx}"));
            }
            ConstValue::Char(c) => fx.store(*dest, &(*c as u32).to_string()),
            ConstValue::Unit => {}
        },
        Inst::Move { dest, src } => {
            // Aggregates copy the pointer (see module docs).
            let v = fx.load(*src);
            fx.store(*dest, &v);
        }
        Inst::Bin {
            dest,
            op,
            ty,
            lhs,
            rhs,
        } => emit_bin(fx, *dest, *op, ty, *lhs, *rhs),
        Inst::Un { dest, op, ty, src } => {
            let v = fx.load(*src);
            let t = fx.tmp();
            match (op, ty) {
                (UnOp::Neg, Type::F64) => fx.line(&format!("{t} = fneg double {v}")),
                (UnOp::Neg, _) => fx.line(&format!("{t} = sub {} 0, {v}", llvm_ty(ty))),
                (UnOp::Not, Type::Bool) => fx.line(&format!("{t} = xor i1 {v}, true")),
                (UnOp::Not, _) => fx.line(&format!("{t} = xor {} {v}, -1", llvm_ty(ty))),
            }
            fx.store(*dest, &t);
        }
        Inst::Call { dest, func, args } => emit_call(fx, *dest, func, args),
        Inst::Cast { dest, src, from, to } => {
            let v = fx.load(*src);
            let r = emit_cast(fx, &v, from, to);
            fx.store(*dest, &r);
        }
        Inst::IndexLoad {
            dest,
            base,
            index,
            elem,
        } => {
            let p = fx.index_ptr(*base, *index, elem);
            fx.load_through(&p, *dest, elem);
        }
        Inst::IndexStore {
            base,
            index,
            value,
            elem,
        } => {
            let p = fx.index_ptr(*base, *index, elem);
            fx.store_through(&p, *value, elem);
        }
        Inst::FieldLoad {
            dest,
            base,
            index,
            ty,
        } => {
            let p = fx.field_ptr(*base, *index, ty);
            fx.load_through(&p, *dest, ty);
        }
        Inst::FieldStore {
            base,
            index,
            value,
            ty,
        } => {
            let p = fx.field_ptr(*base, *index, ty);
            fx.store_through(&p, *value, ty);
        }
        Inst::AllocArray { dest, elem, len } => {
            let t = fx.tmp();
            fx.line(&format!("{t} = alloca [{len} x {}]", llvm_ty(elem)));
            fx.store(*dest, &t);
        }
        Inst::AllocStruct { dest, ty } => {
            let t = fx.tmp();
            fx.line(&format!("{t} = alloca {}", llvm_ty(ty)));
            fx.store(*dest, &t);
        }
        Inst::Nop => {}
    }
}

fn emit_bin(fx: &mut FnCtx, dest: Reg, op: BinOp, ty: &Type, lhs: Reg, rhs: Reg) {
    let a = fx.load(lhs);
    let b = fx.load(rhs);
    let t = fx.tmp();
    match ty {
        Type::String => {
            if op == BinOp::Add {
                fx.line("; UNSUPPORTED: string concatenation");
                fx.line("call void @abort()");
                return;
            }
            // Ordering/equality via strcmp against zero.
            let c = fx.tmp();
            fx.line(&format!("{c} = call i32 @strcmp(ptr {a}, ptr {b})"));
            fx.line(&format!("{t} = icmp {} i32 {c}, 0", icmp_pred(op)));
        }
        Type::Array { .. } | Type::Struct { .. } => {
            // Content comparison of the backing storage.
            let aty = llvm_ty(ty);
            let size = format!("ptrtoint (ptr getelementptr ({aty}, ptr null, i32 1) to i64)");
            let c = fx.tmp();
            fx.line(&format!("{c} = call i32 @memcmp(ptr {a}, ptr {b}, i64 {size})"));
            fx.line(&format!("{t} = icmp {} i32 {c}, 0", icmp_pred(op)));
        }
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
                // Not typeable on floats; reinterpret as integer bit ops.
                BinOp::And | BinOp::Or => {
                    let ai = fx.tmp();
                    let bi = fx.tmp();
                    let ri = fx.tmp();
                    let opc = if op == BinOp::And { "and" } else { "or" };
                    fx.line(&format!("{ai} = bitcast double {a} to i64"));
                    fx.line(&format!("{bi} = bitcast double {b} to i64"));
                    fx.line(&format!("{ri} = {opc} i64 {ai}, {bi}"));
                    fx.line(&format!("{t} = bitcast i64 {ri} to double"));
                    fx.store(dest, &t);
                    return;
                }
            };
            fx.line(&format!("{t} = {opcode} double {a}, {b}"));
        }
        _ => {
            // i32, i64, char (i32), bool (i1) and anything untyped.
            let lty = llvm_ty(ty);
            let opcode = match op {
                BinOp::Add => "add".to_string(),
                BinOp::Sub => "sub".to_string(),
                BinOp::Mul => "mul".to_string(),
                BinOp::Div => "sdiv".to_string(),
                BinOp::Rem => "srem".to_string(),
                BinOp::And => "and".to_string(),
                BinOp::Or => "or".to_string(),
                cmp => format!("icmp {}", icmp_pred(cmp)),
            };
            fx.line(&format!("{t} = {opcode} {lty} {a}, {b}"));
        }
    }
    fx.store(dest, &t);
}

fn icmp_pred(op: BinOp) -> &'static str {
    match op {
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => "slt",
        BinOp::Le => "sle",
        BinOp::Gt => "sgt",
        BinOp::Ge => "sge",
        _ => "ne",
    }
}

fn emit_call(fx: &mut FnCtx, dest: Option<Reg>, func: &str, args: &[Reg]) {
    let params = callee_params(fx.cx, func).filter(|p| p.len() == args.len());
    let ret = callee_ret(fx.cx, func);

    let mut argv = Vec::with_capacity(args.len());
    for (i, r) in args.iter().enumerate() {
        let v = fx.load(*r);
        let ty = match &params {
            Some(p) => slot_ty(&p[i]),
            None => fx.regs.slot(*r),
        };
        argv.push(format!("{ty} {v}"));
    }

    let ret_ty = match &ret {
        Some(t) => llvm_ret(t),
        // Unknown callee: declare it from what we see at the call site.
        None => {
            let guessed = match dest {
                Some(d) => fx.regs.slot(d),
                None => "void".into(),
            };
            let ptys: Vec<String> = argv
                .iter()
                .map(|a| a.split(' ').next().unwrap_or("i32").to_string())
                .collect();
            let decl = format!("declare {guessed} @{}({})", fn_symbol(func), ptys.join(", "));
            fx.cx.extern_decls.entry(func.to_string()).or_insert(decl);
            guessed
        }
    };

    let callee = format!("@{}({})", fn_symbol(func), argv.join(", "));
    match dest {
        Some(d) if ret_ty != "void" && !fx.regs.is_unit(d) => {
            let t = fx.tmp();
            fx.line(&format!("{t} = call {ret_ty} {callee}"));
            fx.store(d, &t);
        }
        _ => fx.line(&format!("call {ret_ty} {callee}")),
    }
}

/// Convert `v` from `from` to `to`, returning the resulting SSA value. Every
/// combination of scalar types yields a valid instruction (or a plain copy).
fn emit_cast(fx: &mut FnCtx, v: &str, from: &Type, to: &Type) -> String {
    let fty = slot_ty(from);
    let tty = slot_ty(to);
    if fty == tty {
        return v.to_string();
    }
    let t = fx.tmp();
    let opcode = match (fty.as_str(), tty.as_str()) {
        ("i1", "double") => "uitofp",
        ("i1", _) => "zext",
        (_, "i1") if fty == "double" => {
            fx.line(&format!("{t} = fcmp une double {v}, 0.0"));
            return t;
        }
        (_, "i1") => {
            fx.line(&format!("{t} = icmp ne {fty} {v}, 0"));
            return t;
        }
        ("double", _) => "fptosi",
        (_, "double") => "sitofp",
        ("i32", "i64") => "sext",
        ("i64", "i32") => "trunc",
        ("ptr", _) => "ptrtoint",
        (_, "ptr") => "inttoptr",
        _ => "bitcast",
    };
    fx.line(&format!("{t} = {opcode} {fty} {v} to {tty}"));
    t
}

fn emit_term(fx: &mut FnCtx, term: &Terminator, ret_ty: &Type) {
    match term {
        Terminator::Jump { target } => fx.line(&format!("br label %bb{}", target.0)),
        Terminator::Branch {
            cond,
            then_bb,
            else_bb,
        } => {
            let c = fx.load(*cond);
            let slot = fx.regs.slot(*cond);
            let c = if slot == "i1" {
                c
            } else {
                // Defensive: a non-bool condition is tested against zero.
                let t = fx.tmp();
                if slot == "double" {
                    fx.line(&format!("{t} = fcmp une double {c}, 0.0"));
                } else if slot == "ptr" {
                    fx.line(&format!("{t} = icmp ne ptr {c}, null"));
                } else {
                    fx.line(&format!("{t} = icmp ne {slot} {c}, 0"));
                }
                t
            };
            fx.line(&format!(
                "br i1 {c}, label %bb{}, label %bb{}",
                then_bb.0, else_bb.0
            ));
        }
        Terminator::Return { value } => match (ret_ty, value) {
            (Type::Unit | Type::Error, _) => fx.line("ret void"),
            (_, Some(r)) if !fx.regs.is_unit(*r) => {
                let t = fx.tmp();
                let rty = slot_ty(ret_ty);
                fx.line(&format!("{t} = load {rty}, ptr %r{}", r.0));
                if is_aggregate(ret_ty) {
                    // The callee's alloca dies with the frame; hand back a heap copy.
                    let aty = llvm_ty(ret_ty);
                    let size = format!("ptrtoint (ptr getelementptr ({aty}, ptr null, i32 1) to i64)");
                    let heap = fx.tmp();
                    let val = fx.tmp();
                    fx.line(&format!("{heap} = call ptr @malloc(i64 {size})"));
                    fx.line(&format!("{val} = load {aty}, ptr {t}"));
                    fx.line(&format!("store {aty} {val}, ptr {heap}"));
                    fx.line(&format!("ret ptr {heap}"));
                } else {
                    fx.line(&format!("ret {rty} {t}"));
                }
            }
            // Dead block left behind by lowering (`return` with no value in a
            // non-unit function): nothing meaningful can be returned.
            _ => fx.line("unreachable"),
        },
        Terminator::Unreachable => fx.line("unreachable"),
    }
}

/// Terminator for a body-less function: return a zero value of the right type.
fn default_return(ret_ty: &Type) -> String {
    match ret_ty {
        Type::Unit | Type::Error => "ret void".into(),
        Type::F64 => "ret double 0.0".into(),
        Type::Bool => "ret i1 false".into(),
        Type::String | Type::Array { .. } | Type::Struct { .. } | Type::Fn { .. } => {
            "ret ptr null".into()
        }
        other => format!("ret {} 0", llvm_ty(other)),
    }
}

// ---------------------------------------------------------------------------
// Types and names
// ---------------------------------------------------------------------------

/// LLVM type of a value of type `ty` *as stored in memory* (array elements,
/// struct fields, alloca contents). Aggregates are spelled out in full.
fn llvm_ty(ty: &Type) -> String {
    match ty {
        Type::Unit | Type::Error => "void".into(),
        Type::Bool => "i1".into(),
        Type::I32 | Type::Char => "i32".into(),
        Type::I64 => "i64".into(),
        Type::F64 => "double".into(),
        Type::String | Type::Fn { .. } => "ptr".into(),
        Type::Array { elem, len } => format!("[{len} x {}]", llvm_ty(elem)),
        Type::Struct { name, .. } => struct_name(name),
    }
}

/// LLVM type of a register slot / parameter / return value holding `ty`.
/// Aggregates are passed around by pointer.
fn slot_ty(ty: &Type) -> String {
    if is_aggregate(ty) {
        "ptr".into()
    } else {
        llvm_ty(ty)
    }
}

fn llvm_ret(ty: &Type) -> String {
    match ty {
        Type::Unit | Type::Error => "void".into(),
        _ => slot_ty(ty),
    }
}

fn is_aggregate(ty: &Type) -> bool {
    matches!(ty, Type::Array { .. } | Type::Struct { .. })
}

fn struct_name(name: &str) -> String {
    format!("%struct.{}", sanitize(name))
}

/// Exact IEEE-754 bit pattern; LLVM accepts this for any double, including
/// values whose shortest decimal form would not round-trip.
fn llvm_f64(v: f64) -> String {
    format!("0x{:016X}", v.to_bits())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

/// Symbol for a user function: sanitized, and kept clear of the libc symbols
/// the prelude declares.
fn fn_symbol(name: &str) -> String {
    let s = sanitize(name);
    if RESERVED.contains(&s.as_str()) {
        format!("{s}.ae")
    } else {
        s
    }
}

fn llvm_string_bytes(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'\\' => out.push_str("\\5C"),
            b'"' => out.push_str("\\22"),
            0..=31 | 127..=255 => out.push_str(&format!("\\{b:02X}")),
            _ => out.push(b as char),
        }
    }
    out.push_str("\\00");
    out
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
        assert!(ll.contains("define i32 @main"));
        assert!(ll.contains("@println"));
        assert!(ll.contains("c\"hello, aether\\00\""));
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
    fn aggregates_and_strings() {
        let src = "struct P { x: i32, y: f64 }
            fn main() -> i32 {
                let p = P { x: 1, y: 2.5 };
                let a: [i32; 3] = [1, 2, 3];
                let b = a[1] + p.x;
                let s = \"hi\";
                if s == \"hi\" { print_f64(p.y); }
                return b;
            }";
        let ll = llvm_of("t.ae", src, 0);
        assert!(ll.contains("%struct.P = type { i32, double }"));
        assert!(ll.contains("alloca %struct.P"));
        assert!(ll.contains("alloca [3 x i32]"));
        assert!(ll.contains("getelementptr [3 x i32], ptr"));
        assert!(ll.contains("getelementptr %struct.P, ptr"));
        assert!(ll.contains("@strcmp"));
        assert!(ll.contains("[3 x i8] c\"hi\\00\""));
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
