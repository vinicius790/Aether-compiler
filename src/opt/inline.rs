//! Leaf inlining: a callee with one block, no calls and a small body
//! is copied into the caller. Register names are shifted so they do
//! not collide. Recursion and `main` are never inlined.

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

fn remap_reg(r: Reg, base: u32, params: &[(u32, u32)]) -> Reg {
    for (from, to) in params {
        if r.0 == *from {
            return Reg(*to);
        }
    }
    Reg(r.0 + base)
}

fn remap_inst(inst: &Inst, base: u32, params: &[(u32, u32)]) -> Inst {
    let m = |r: Reg| remap_reg(r, base, params);
    match inst.clone() {
        Inst::LoadConst { dest, value } => Inst::LoadConst {
            dest: m(dest),
            value,
        },
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
        other => other,
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
                match &inst {
                    Inst::Call {
                        dest,
                        func,
                        args,
                    } => {
                        if let Some(cal) = leaves.iter().find(|c| c.name == *func) {
                            let base = f.reg_count + extra;
                            extra += cal.reg_count;
                            let params: Vec<(u32, u32)> = cal
                                .params
                                .iter()
                                .zip(args.iter())
                                .map(|((_, _, pr), a)| (pr.0, a.0))
                                .collect();
                            for ci in &cal.blocks[0].insts {
                                out.push(remap_inst(ci, base, &params));
                            }
                            if let (Some(d), Terminator::Return { value: Some(r) }) =
                                (*dest, &cal.blocks[0].term)
                            {
                                out.push(Inst::Move {
                                    dest: d,
                                    src: remap_reg(*r, base, &params),
                                });
                            }
                        } else {
                            out.push(inst);
                        }
                    }
                    _ => out.push(inst),
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

    #[test]
    fn inlines_tiny_add() {
        let src = r#"
            fn add1(x: i32) -> i32 { return x + 1; }
            fn main() -> i32 { return add1(3); }
        "#;
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, _) = analyze(&prog);
        let ir = emit_ir(&hir.unwrap());
        let (opt, _) = optimize(ir, 2);
        let text = crate::ir::dump_ir(&opt);
        // after inline + fold the main body should not need the helper at runtime
        let bc = crate::backend::assemble(&opt);
        let (v, _, _) = crate::vm::execute_captured(&bc).expect("vm");
        assert_eq!(v, crate::vm::Value::I32(4), "{text}");
    }
}
