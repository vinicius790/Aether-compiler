//! Textual LLVM IR emitter.
//!
//! We emit LLVM 15+ compatible IR and shell out to `opt`/`llc`/`clang`/`lli`
//! when those tools are present. Linking against libLLVM is deliberately
//! avoided so the compiler stays portable.

use crate::ast::{BinOp, UnOp};
use crate::ir::{ConstValue, Inst, IrFunction, IrModule, Terminator};
use crate::ty::Type;
use std::fmt::Write as _;
use std::process::{Command, Stdio};

pub fn emit_llvm_ir(module: &IrModule) -> String {
    let mut out = String::new();
    out.push_str("; ModuleID = 'aether'\n");
    out.push_str("source_filename = \"aether\"\n");
    out.push_str("target datalayout = \"e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128\"\n");
    out.push_str("target triple = \"x86_64-pc-linux-gnu\"\n\n");
    out.push_str("declare i32 @printf(ptr, ...)\n");
    out.push_str("declare i32 @puts(ptr)\n");
    out.push_str("@.nl = private unnamed_addr constant [2 x i8] c\"\\0A\\00\"\n");
    out.push_str("@.fmt_i32 = private unnamed_addr constant [4 x i8] c\"%d\\0A\\00\"\n");
    out.push_str("@.fmt_i64 = private unnamed_addr constant [6 x i8] c\"%lld\\0A\\00\"\n");
    out.push_str("@.fmt_f64 = private unnamed_addr constant [5 x i8] c\"%f\\0A\\00\"\n");
    out.push_str("@.fmt_str = private unnamed_addr constant [4 x i8] c\"%s\\0A\\00\"\n\n");

    for (name, ty) in &module.structs {
        if let Type::Struct { fields, .. } = ty {
            let parts: Vec<String> = fields.iter().map(|(_, t)| llvm_ty(t)).collect();
            let _ = writeln!(out, "%struct.{} = type {{ {} }}", name, parts.join(", "));
        }
    }
    out.push('\n');

    // string constants collected during emission
    let mut strings: Vec<String> = Vec::new();

    for f in &module.functions {
        if is_builtin(&f.name) {
            continue;
        }
        emit_function(&mut out, f, &mut strings);
    }

    // prepend string globals — we appended functions first; rebuild with strings on top is
    // simpler by emitting strings at the end (LLVM allows it).
    if !strings.is_empty() {
        let mut prelude = String::new();
        for (i, s) in strings.iter().enumerate() {
            let bytes = llvm_string_bytes(s);
            let _ = writeln!(
                prelude,
                "@.str.{i} = private unnamed_addr constant [{n} x i8] c\"{bytes}\"",
                n = s.len() + 1
            );
        }
        out.push('\n');
        out.push_str(&prelude);
    }

    // builtin wrappers
    out.push_str(
        r#"
define void @print_i32(i32 %v) {
  call i32 (ptr, ...) @printf(ptr @.fmt_i32, i32 %v)
  ret void
}
define void @print_i64(i64 %v) {
  call i32 (ptr, ...) @printf(ptr @.fmt_i64, i64 %v)
  ret void
}
define void @print_f64(double %v) {
  call i32 (ptr, ...) @printf(ptr @.fmt_f64, double %v)
  ret void
}
define void @print_bool(i1 %v) {
  %z = zext i1 %v to i32
  call i32 (ptr, ...) @printf(ptr @.fmt_i32, i32 %z)
  ret void
}
define void @println(ptr %s) {
  call i32 (ptr, ...) @printf(ptr @.fmt_str, ptr %s)
  ret void
}
define void @print(ptr %s) {
  call i32 (ptr, ...) @printf(ptr @.fmt_str, ptr %s)
  ret void
}
"#,
    );
    out
}

fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "print" | "println" | "print_i32" | "print_i64" | "print_f64" | "print_bool" | "len" | "assert"
    )
}

fn emit_function(out: &mut String, f: &IrFunction, strings: &mut Vec<String>) {
    let ret = llvm_ret(&f.return_ty);
    let params: Vec<String> = f
        .params
        .iter()
        .map(|(n, t, r)| format!("{} %r{} ; {n}", llvm_ty(t), r.0))
        .collect();
    let _ = writeln!(
        out,
        "define {ret} @{name}({params}) {{",
        name = sanitize(&f.name),
        params = params.join(", ")
    );
    for bb in &f.blocks {
        let _ = writeln!(out, "bb{}:", bb.id.0);
        for inst in &bb.insts {
            emit_inst(out, inst, strings);
        }
        emit_term(out, &bb.term, &f.return_ty);
    }
    out.push_str("}\n\n");
}

fn emit_inst(out: &mut String, inst: &Inst, strings: &mut Vec<String>) {
    match inst {
        Inst::LoadConst { dest, value } => match value {
            ConstValue::I32(v) => {
                let _ = writeln!(out, "  %r{} = add i32 0, {v}", dest.0);
            }
            ConstValue::I64(v) => {
                let _ = writeln!(out, "  %r{} = add i64 0, {v}", dest.0);
            }
            ConstValue::F64(v) => {
                let _ = writeln!(out, "  %r{} = fadd double 0.0, {v}", dest.0);
            }
            ConstValue::Bool(v) => {
                let bit = if *v { 1 } else { 0 };
                let _ = writeln!(out, "  %r{} = add i1 0, {bit}", dest.0);
            }
            ConstValue::String(s) => {
                let idx = intern(strings, s);
                let n = s.len() + 1;
                let _ = writeln!(
                    out,
                    "  %r{} = getelementptr inbounds [{n} x i8], ptr @.str.{idx}, i32 0, i32 0",
                    dest.0
                );
            }
            ConstValue::Char(c) => {
                let _ = writeln!(out, "  %r{} = add i8 0, {}", dest.0, *c as u8);
            }
            ConstValue::Unit => {
                let _ = writeln!(out, "  ; unit -> %r{}", dest.0);
            }
        },
        Inst::Move { dest, src } => {
            // LLVM SSA cannot reassign; we emit a dummy orbit. For non-SSA IR we
            // use a stack slot convention: treat registers as values and accept
            // that some programs need mem2reg. Emit `add x, 0` as a copy.
            let _ = writeln!(out, "  %r{} = add i32 %r{}, 0", dest.0, src.0);
        }
        Inst::Bin {
            dest,
            op,
            ty,
            lhs,
            rhs,
        } => {
            let lty = llvm_ty(ty);
            let opcode = match (op, ty) {
                (BinOp::Add, Type::F64) => "fadd",
                (BinOp::Sub, Type::F64) => "fsub",
                (BinOp::Mul, Type::F64) => "fmul",
                (BinOp::Div, Type::F64) => "fdiv",
                (BinOp::Add, _) => "add",
                (BinOp::Sub, _) => "sub",
                (BinOp::Mul, _) => "mul",
                (BinOp::Div, _) => "sdiv",
                (BinOp::Rem, _) => "srem",
                (BinOp::Eq, Type::F64) => "fcmp oeq",
                (BinOp::Ne, Type::F64) => "fcmp one",
                (BinOp::Lt, Type::F64) => "fcmp olt",
                (BinOp::Le, Type::F64) => "fcmp ole",
                (BinOp::Gt, Type::F64) => "fcmp ogt",
                (BinOp::Ge, Type::F64) => "fcmp oge",
                (BinOp::Eq, _) => "icmp eq",
                (BinOp::Ne, _) => "icmp ne",
                (BinOp::Lt, _) => "icmp slt",
                (BinOp::Le, _) => "icmp sle",
                (BinOp::Gt, _) => "icmp sgt",
                (BinOp::Ge, _) => "icmp sge",
                (BinOp::And, _) => "and",
                (BinOp::Or, _) => "or",
            };
            let _ = writeln!(
                out,
                "  %r{} = {opcode} {lty} %r{}, %r{}",
                dest.0, lhs.0, rhs.0
            );
        }
        Inst::Un { dest, op, ty, src } => match op {
            UnOp::Neg if *ty == Type::F64 => {
                let _ = writeln!(out, "  %r{} = fneg double %r{}", dest.0, src.0);
            }
            UnOp::Neg => {
                let _ = writeln!(out, "  %r{} = sub {} 0, %r{}", dest.0, llvm_ty(ty), src.0);
            }
            UnOp::Not => {
                let _ = writeln!(out, "  %r{} = xor i1 %r{}, true", dest.0, src.0);
            }
        },
        Inst::Call { dest, func, args } => {
            let argv: Vec<String> = args.iter().map(|r| format!("i32 %r{}", r.0)).collect();
            if let Some(d) = dest {
                let _ = writeln!(
                    out,
                    "  %r{} = call i32 @{}({})",
                    d.0,
                    sanitize(func),
                    argv.join(", ")
                );
            } else {
                let _ = writeln!(
                    out,
                    "  call void @{}({})",
                    sanitize(func),
                    argv.join(", ")
                );
            }
        }
        Inst::Cast { dest, src, from, to } => {
            let op = match (from, to) {
                (Type::I32, Type::I64) => "sext",
                (Type::I64, Type::I32) => "trunc",
                (Type::I32, Type::F64) => "sitofp",
                (Type::F64, Type::I32) => "fptosi",
                (Type::Bool, Type::I32) => "zext",
                _ => "bitcast",
            };
            let _ = writeln!(
                out,
                "  %r{} = {op} {} %r{} to {}",
                dest.0,
                llvm_ty(from),
                src.0,
                llvm_ty(to)
            );
        }
        Inst::AllocArray { dest, elem, len } => {
            let _ = writeln!(
                out,
                "  %r{} = alloca [{} x {}], align 8",
                dest.0,
                len,
                llvm_ty(elem)
            );
        }
        Inst::AllocStruct { dest, ty } => {
            let _ = writeln!(out, "  %r{} = alloca {}, align 8", dest.0, llvm_ty(ty));
        }
        Inst::IndexLoad {
            dest, base, index, elem, ..
        } => {
            let _ = writeln!(
                out,
                "  %r{}_ptr = getelementptr i32, ptr %r{}, i32 %r{}\n  %r{} = load {}, ptr %r{}_ptr",
                dest.0,
                base.0,
                index.0,
                dest.0,
                llvm_ty(elem),
                dest.0
            );
        }
        Inst::IndexStore {
            base, index, value, elem, ..
        } => {
            let _ = writeln!(
                out,
                "  %t{}_{}_ptr = getelementptr i32, ptr %r{}, i32 %r{}\n  store {} %r{}, ptr %t{}_{}_ptr",
                base.0,
                value.0,
                base.0,
                index.0,
                llvm_ty(elem),
                value.0,
                base.0,
                value.0
            );
        }
        Inst::FieldLoad {
            dest, base, index, ty, ..
        } => {
            let _ = writeln!(
                out,
                "  %r{}_fptr = getelementptr i8, ptr %r{}, i32 {}\n  %r{} = load {}, ptr %r{}_fptr",
                dest.0, base.0, index, dest.0, llvm_ty(ty), dest.0
            );
        }
        Inst::FieldStore {
            base, index, value, ty, ..
        } => {
            let _ = writeln!(
                out,
                "  %t{}_{}_fptr = getelementptr i8, ptr %r{}, i32 {}\n  store {} %r{}, ptr %t{}_{}_fptr",
                base.0, value.0, base.0, index, llvm_ty(ty), value.0, base.0, value.0
            );
        }
        Inst::Nop => {}
    }
}

fn emit_term(out: &mut String, term: &Terminator, ret_ty: &Type) {
    match term {
        Terminator::Jump { target } => {
            let _ = writeln!(out, "  br label %bb{}", target.0);
        }
        Terminator::Branch {
            cond,
            then_bb,
            else_bb,
        } => {
            let _ = writeln!(
                out,
                "  br i1 %r{}, label %bb{}, label %bb{}",
                cond.0, then_bb.0, else_bb.0
            );
        }
        Terminator::Return { value } => match (ret_ty, value) {
            (Type::Unit, _) => {
                let _ = writeln!(out, "  ret void");
            }
            (_, Some(r)) => {
                let _ = writeln!(out, "  ret {} %r{}", llvm_ty(ret_ty), r.0);
            }
            (Type::I32, None) => {
                let _ = writeln!(out, "  ret i32 0");
            }
            _ => {
                let _ = writeln!(out, "  ret void");
            }
        },
        Terminator::Unreachable => {
            let _ = writeln!(out, "  unreachable");
        }
    }
}

fn llvm_ty(ty: &Type) -> String {
    match ty {
        Type::Unit | Type::Error => "void".into(),
        Type::Bool => "i1".into(),
        Type::I32 => "i32".into(),
        Type::I64 => "i64".into(),
        Type::F64 => "double".into(),
        Type::String => "ptr".into(),
        Type::Char => "i8".into(),
        Type::Array { elem, len } => format!("[{len} x {}]", llvm_ty(elem)),
        Type::Struct { name, .. } => format!("%struct.{name}"),
        Type::Fn { .. } => "ptr".into(),
    }
}

fn llvm_ret(ty: &Type) -> String {
    if *ty == Type::Unit {
        "void".into()
    } else {
        llvm_ty(ty)
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

fn intern(strings: &mut Vec<String>, s: &str) -> usize {
    if let Some(i) = strings.iter().position(|x| x == s) {
        i
    } else {
        strings.push(s.to_string());
        strings.len() - 1
    }
}

fn llvm_string_bytes(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\22"),
            0..=31 | 127..=255 => out.push_str(&format!("\\{b:02X}")),
            _ => out.push(b as char),
        }
    }
    out.push_str("\\00");
    out
}

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
    use crate::ir::emit_ir;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::sema::analyze;
    use crate::span::FileId;

    #[test]
    fn emits_define_main() {
        let src = "fn main() -> i32 { return 1 + 2; }";
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, _) = analyze(&prog);
        let ir = emit_ir(&hir.unwrap());
        let ll = emit_llvm_ir(&ir);
        assert!(ll.contains("define i32 @main"));
        assert!(ll.contains("add i32"));
    }
}
