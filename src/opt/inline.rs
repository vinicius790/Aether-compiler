//! Leaf inlining: a callee with one block, no calls and a small body is
//! copied into the caller. Callee registers are shifted past the caller's
//! `reg_count` so they cannot collide; arguments are copied into the shifted
//! parameter registers, so a callee that reassigns a parameter never writes
//! into the caller's argument register. Recursion and `main` are never inlined.
//!
//! Runs after `cf-simplify`, which removes the dead block lowering leaves
//! after every `return`; before that pass no function is a single block.

use crate::ir::{Inst, IrFunction, IrModule, Reg, Terminator};

const MAX_INSTS: usize = 12;

pub fn is_inlineable(f: &IrFunction) -> bool {
    if f.name == "main" || f.is_extern || f.blocks.len() != 1 {
        return false;
    }
    let bb = &f.blocks[0];
    if bb.insts.len() > MAX_INSTS {
        return false;
    }
    if bb.insts.iter().any(|i| matches!(i, Inst::Call { .. })) {
        return false;
    }
    matches!(bb.term, Terminator::Return { .. })
}

/// Shift every register of `inst` by `base`.
fn remap_inst(inst: &Inst, base: u32) -> Inst {
    let m = |r: Reg| Reg(r.0 + base);
    match inst.clone() {
        Inst::LoadConst { dest, value } => Inst::LoadConst { dest: m(dest), value },
        Inst::Move { dest, src } => Inst::Move {
            dest: m(dest),
            src: m(src),
        },
        Inst::Bin {
            dest,
            op,
            ty,
            lhs,
            rhs,
        } => Inst::Bin {
            dest: m(dest),
            op,
            ty,
            lhs: m(lhs),
            rhs: m(rhs),
        },
        Inst::Un { dest, op, ty, src } => Inst::Un {
            dest: m(dest),
            op,
            ty,
            src: m(src),
        },
        Inst::Call { dest, func, args } => Inst::Call {
            dest: dest.map(m),
            func,
            args: args.into_iter().map(m).collect(),
        },
        Inst::Cast {
            dest,
            src,
            from,
            to,
        } => Inst::Cast {
            dest: m(dest),
            src: m(src),
            from,
            to,
        },
        Inst::IndexLoad {
            dest,
            base: b,
            index,
            elem,
        } => Inst::IndexLoad {
            dest: m(dest),
            base: m(b),
            index: m(index),
            elem,
        },
        Inst::IndexStore {
            base: b,
            index,
            value,
            elem,
        } => Inst::IndexStore {
            base: m(b),
            index: m(index),
            value: m(value),
            elem,
        },
        Inst::FieldLoad {
            dest,
            base: b,
            index,
            ty,
        } => Inst::FieldLoad {
            dest: m(dest),
            base: m(b),
            index,
            ty,
        },
        Inst::FieldStore {
            base: b,
            index,
            value,
            ty,
        } => Inst::FieldStore {
            base: m(b),
            index,
            value: m(value),
            ty,
        },
        Inst::AllocArray { dest, elem, len } => Inst::AllocArray {
            dest: m(dest),
            elem,
            len,
        },
        Inst::AllocStruct { dest, ty } => Inst::AllocStruct { dest: m(dest), ty },
        Inst::Nop => Inst::Nop,
    }
}

pub fn pass_inline(module: &mut IrModule) {
    let leaves: Vec<IrFunction> = module
        .functions
        .iter()
        .filter(|f| is_inlineable(f))
        .cloned()
        .collect();
    if leaves.is_empty() {
        return;
    }
    for f in &mut module.functions {
        let mut extra = 0u32;
        for bb in &mut f.blocks {
            let mut out = Vec::new();
            for inst in bb.insts.drain(..) {
                let Inst::Call { dest, func, args } = &inst else {
                    out.push(inst);
                    continue;
                };
                let Some(cal) = leaves.iter().find(|c| c.name == *func && c.name != f.name) else {
                    out.push(inst);
                    continue;
                };
                let base = f.reg_count + extra;
                extra += cal.reg_count;
                for ((_, _, pr), a) in cal.params.iter().zip(args.iter()) {
                    out.push(Inst::Move {
                        dest: Reg(pr.0 + base),
                        src: *a,
                    });
                }
                for ci in &cal.blocks[0].insts {
                    out.push(remap_inst(ci, base));
                }
                if let (Some(d), Terminator::Return { value: Some(r) }) = (*dest, &cal.blocks[0].term) {
                    out.push(Inst::Move {
                        dest: d,
                        src: Reg(r.0 + base),
                    });
                }
            }
            bb.insts = out;
        }
        f.reg_count += extra;
    }
}

#[cfg(test)]
mod tests {
    use crate::ir::emit_ir;
    use crate::lexer::tokenize;
    use crate::opt::optimize;
    use crate::parser::parse;
    use crate::sema::analyze;
    use crate::span::FileId;

    fn run(src: &str) -> (crate::vm::Value, String) {
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, _) = analyze(&prog);
        let ir = emit_ir(&hir.unwrap());
        let (opt, _) = optimize(ir, 2);
        let text = crate::ir::dump_ir(&opt);
        let bc = crate::backend::assemble(&opt);
        let (v, _, _) = crate::vm::execute_captured(&bc).expect("vm");
        (v, text)
    }

    #[test]
    fn inlines_tiny_add() {
        let src = r#"
            fn add1(x: i32) -> i32 { return x + 1; }
            fn main() -> i32 { return add1(3); }
        "#;
        let (v, text) = run(src);
        assert_eq!(v, crate::vm::Value::I32(4), "{text}");
        let main = text.split("fn main").nth(1).unwrap();
        assert!(!main.contains("call add1"), "call should be inlined:\n{text}");
    }

    #[test]
    fn inlined_local_does_not_clobber_caller() {
        let src = r#"
            fn bump(x: i32) -> i32 { let mut y = x; y = y + 1; return y; }
            fn main() -> i32 { let a = 5; let b = bump(a); return a * 10 + b; }
        "#;
        let (v, text) = run(src);
        assert_eq!(v, crate::vm::Value::I32(56), "{text}");
    }
}
