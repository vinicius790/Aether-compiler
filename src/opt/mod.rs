//! Optimization pipeline over Aether IR.
//!
//! Each pass is a function `IrModule -> (IrModule, PassStats)`. The driver
//! records per-pass instruction counts so `aether optimize --stats` can show
//! before/after differences.

pub mod inline;
pub mod liveness;

use crate::ast::{BinOp, UnOp};
use crate::ir::{ConstValue, Inst, IrFunction, IrModule, Reg, Terminator};
use crate::ty::Type;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct PassStats {
    pub name: String,
    pub insts_before: usize,
    pub insts_after: usize,
}

impl PassStats {
    pub fn delta(&self) -> i64 {
        self.insts_after as i64 - self.insts_before as i64
    }
}

#[derive(Debug, Clone, Default)]
pub struct OptReport {
    pub passes: Vec<PassStats>,
    pub insts_before: usize,
    pub insts_after: usize,
}

impl OptReport {
    pub fn summary(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "IR instructions: {} → {} ({:+})\n",
            self.insts_before,
            self.insts_after,
            self.insts_after as i64 - self.insts_before as i64
        ));
        for p in &self.passes {
            s.push_str(&format!(
                "  {:<24} {:>5} → {:>5}  ({:+})\n",
                p.name,
                p.insts_before,
                p.insts_after,
                p.delta()
            ));
        }
        s
    }
}

fn count_insts(m: &IrModule) -> usize {
    m.functions
        .iter()
        .map(|f| f.blocks.iter().map(|b| b.insts.len() + 1).sum::<usize>())
        .sum()
}

pub fn optimize(module: IrModule, level: u8) -> (IrModule, OptReport) {
    let mut module = module;
    let insts_before = count_insts(&module);
    let mut report = OptReport {
        insts_before,
        ..Default::default()
    };
    if level == 0 {
        report.insts_after = insts_before;
        return (module, report);
    }

    let passes: Vec<(&str, fn(&mut IrModule))> = if level >= 2 {
        vec![
            ("const-fold", pass_const_fold),
            ("algebraic", pass_algebraic),
            ("local-cse", pass_local_cse),
            ("inline", crate::opt::inline::pass_inline),
            ("copy-prop", pass_copy_prop),
            ("const-prop", pass_const_prop),
            ("cf-simplify", pass_cf_simplify),
            ("dce", pass_dce),
            ("const-fold", pass_const_fold),
            ("dce", pass_dce),
        ]
    } else {
        vec![
            ("const-fold", pass_const_fold),
            ("copy-prop", pass_copy_prop),
            ("dce", pass_dce),
        ]
    };

    for (name, pass) in passes {
        let before = count_insts(&module);
        pass(&mut module);
        let after = count_insts(&module);
        report.passes.push(PassStats {
            name: name.to_string(),
            insts_before: before,
            insts_after: after,
        });
    }
    report.insts_after = count_insts(&module);
    (module, report)
}

fn fold_bin(op: BinOp, ty: &Type, lhs: &ConstValue, rhs: &ConstValue) -> Option<ConstValue> {
    match (op, lhs, rhs) {
        (BinOp::Add, ConstValue::I32(a), ConstValue::I32(b)) => {
            Some(ConstValue::I32(a.wrapping_add(*b)))
        }
        (BinOp::Sub, ConstValue::I32(a), ConstValue::I32(b)) => {
            Some(ConstValue::I32(a.wrapping_sub(*b)))
        }
        (BinOp::Mul, ConstValue::I32(a), ConstValue::I32(b)) => {
            Some(ConstValue::I32(a.wrapping_mul(*b)))
        }
        (BinOp::Div, ConstValue::I32(a), ConstValue::I32(b)) if *b != 0 => Some(ConstValue::I32(a / b)),
        (BinOp::Rem, ConstValue::I32(a), ConstValue::I32(b)) if *b != 0 => Some(ConstValue::I32(a % b)),
        (BinOp::Add, ConstValue::I64(a), ConstValue::I64(b)) => {
            Some(ConstValue::I64(a.wrapping_add(*b)))
        }
        (BinOp::Sub, ConstValue::I64(a), ConstValue::I64(b)) => {
            Some(ConstValue::I64(a.wrapping_sub(*b)))
        }
        (BinOp::Mul, ConstValue::I64(a), ConstValue::I64(b)) => {
            Some(ConstValue::I64(a.wrapping_mul(*b)))
        }
        (BinOp::Div, ConstValue::I64(a), ConstValue::I64(b)) if *b != 0 => Some(ConstValue::I64(a / b)),
        (BinOp::Add, ConstValue::F64(a), ConstValue::F64(b)) => Some(ConstValue::F64(a + b)),
        (BinOp::Sub, ConstValue::F64(a), ConstValue::F64(b)) => Some(ConstValue::F64(a - b)),
        (BinOp::Mul, ConstValue::F64(a), ConstValue::F64(b)) => Some(ConstValue::F64(a * b)),
        (BinOp::Div, ConstValue::F64(a), ConstValue::F64(b)) if *b != 0.0 => {
            Some(ConstValue::F64(a / b))
        }
        (BinOp::Eq, ConstValue::I32(a), ConstValue::I32(b)) => Some(ConstValue::Bool(a == b)),
        (BinOp::Ne, ConstValue::I32(a), ConstValue::I32(b)) => Some(ConstValue::Bool(a != b)),
        (BinOp::Lt, ConstValue::I32(a), ConstValue::I32(b)) => Some(ConstValue::Bool(a < b)),
        (BinOp::Le, ConstValue::I32(a), ConstValue::I32(b)) => Some(ConstValue::Bool(a <= b)),
        (BinOp::Gt, ConstValue::I32(a), ConstValue::I32(b)) => Some(ConstValue::Bool(a > b)),
        (BinOp::Ge, ConstValue::I32(a), ConstValue::I32(b)) => Some(ConstValue::Bool(a >= b)),
        (BinOp::Eq, ConstValue::Bool(a), ConstValue::Bool(b)) => Some(ConstValue::Bool(a == b)),
        (BinOp::And, ConstValue::Bool(a), ConstValue::Bool(b)) => Some(ConstValue::Bool(*a && *b)),
        (BinOp::Or, ConstValue::Bool(a), ConstValue::Bool(b)) => Some(ConstValue::Bool(*a || *b)),
        (BinOp::Add, ConstValue::String(a), ConstValue::String(b)) => {
            Some(ConstValue::String(format!("{a}{b}")))
        }
        _ => {
            let _ = ty;
            None
        }
    }
}

fn fold_un(op: UnOp, src: &ConstValue) -> Option<ConstValue> {
    match (op, src) {
        (UnOp::Neg, ConstValue::I32(v)) => Some(ConstValue::I32(v.wrapping_neg())),
        (UnOp::Neg, ConstValue::I64(v)) => Some(ConstValue::I64(v.wrapping_neg())),
        (UnOp::Neg, ConstValue::F64(v)) => Some(ConstValue::F64(-v)),
        (UnOp::Not, ConstValue::Bool(v)) => Some(ConstValue::Bool(!v)),
        _ => None,
    }
}

pub fn pass_const_fold(module: &mut IrModule) {
    for f in &mut module.functions {
        fold_function(f);
    }
}

fn fold_function(f: &mut IrFunction) {
    for bb in &mut f.blocks {
        let mut consts: HashMap<u32, ConstValue> = HashMap::new();
        for inst in &mut bb.insts {
            match inst {
                Inst::LoadConst { dest, value } => {
                    consts.insert(dest.0, value.clone());
                }
                Inst::Move { dest, src } => {
                    if let Some(v) = consts.get(&src.0).cloned() {
                        consts.insert(dest.0, v.clone());
                        *inst = Inst::LoadConst { dest: *dest, value: v };
                    } else {
                        consts.remove(&dest.0);
                    }
                }
                Inst::Bin {
                    dest,
                    op,
                    ty,
                    lhs,
                    rhs,
                } => {
                    if let (Some(l), Some(r)) = (consts.get(&lhs.0), consts.get(&rhs.0)) {
                        if let Some(v) = fold_bin(*op, ty, l, r) {
                            consts.insert(dest.0, v.clone());
                            *inst = Inst::LoadConst { dest: *dest, value: v };
                            continue;
                        }
                    }
                    consts.remove(&dest.0);
                }
                Inst::Un { dest, op, src, .. } => {
                    if let Some(v) = consts.get(&src.0) {
                        if let Some(folded) = fold_un(*op, v) {
                            consts.insert(dest.0, folded.clone());
                            *inst = Inst::LoadConst {
                                dest: *dest,
                                value: folded,
                            };
                            continue;
                        }
                    }
                    consts.remove(&dest.0);
                }
                other => {
                    if let Some(d) = other.dest_reg() {
                        consts.remove(&d.0);
                    }
                }
            }
        }
        if let Terminator::Branch { cond, then_bb, else_bb } = bb.term.clone() {
            if let Some(ConstValue::Bool(v)) = consts.get(&cond.0) {
                bb.term = Terminator::Jump {
                    target: if *v { then_bb } else { else_bb },
                };
            }
        }
    }
}

pub fn pass_const_prop(module: &mut IrModule) {
    // const-fold already propagates within a function in one sweep; a second
    // sweep catches chains created by earlier algebraic rewrites.
    pass_const_fold(module);
}

pub fn pass_algebraic(module: &mut IrModule) {
    for f in &mut module.functions {
        for bb in &mut f.blocks {
            let mut consts: HashMap<u32, ConstValue> = HashMap::new();
            for inst in &mut bb.insts {
                if let Inst::LoadConst { dest, value } = inst {
                    consts.insert(dest.0, value.clone());
                } else if let Inst::Move { dest, src } = inst {
                    if let Some(v) = consts.get(&src.0).cloned() {
                        consts.insert(dest.0, v);
                    } else {
                        consts.remove(&dest.0);
                    }
                } else if let Some(d) = inst.dest_reg() {
                    if !matches!(inst, Inst::Bin { .. }) {
                        consts.remove(&d.0);
                    }
                }
                if let Inst::Bin {
                    dest,
                    op,
                    ty,
                    lhs,
                    rhs,
                } = inst.clone()
                {
                    let lconst = consts.get(&lhs.0);
                    let rconst = consts.get(&rhs.0);
                    // x + 0 / x * 1 / x * 0 / x - 0
                    let rewritten = match (op, lconst, rconst) {
                        // Opaque identities: same register, no const required.
                        (BinOp::Sub, _, _) if lhs.0 == rhs.0 => Some(Inst::LoadConst {
                            dest,
                            value: ConstValue::I32(0),
                        }),
                        (BinOp::Eq, _, _) if lhs.0 == rhs.0 && ty == Type::I32 => {
                            Some(Inst::LoadConst {
                                dest,
                                value: ConstValue::Bool(true),
                            })
                        }
                        (BinOp::Ne, _, _) if lhs.0 == rhs.0 && ty == Type::I32 => {
                            Some(Inst::LoadConst {
                                dest,
                                value: ConstValue::Bool(false),
                            })
                        }
                        (BinOp::Lt, _, _) | (BinOp::Gt, _, _) if lhs.0 == rhs.0 && ty == Type::I32 => {
                            Some(Inst::LoadConst {
                                dest,
                                value: ConstValue::Bool(false),
                            })
                        }
                        (BinOp::Le, _, _) | (BinOp::Ge, _, _) if lhs.0 == rhs.0 && ty == Type::I32 => {
                            Some(Inst::LoadConst {
                                dest,
                                value: ConstValue::Bool(true),
                            })
                        }
                        (BinOp::Add, _, Some(ConstValue::I32(0))) => Some(Inst::Move { dest, src: lhs }),
                        (BinOp::Add, Some(ConstValue::I32(0)), _) => Some(Inst::Move { dest, src: rhs }),
                        (BinOp::Sub, _, Some(ConstValue::I32(0))) => Some(Inst::Move { dest, src: lhs }),
                        (BinOp::Mul, _, Some(ConstValue::I32(1))) => Some(Inst::Move { dest, src: lhs }),
                        (BinOp::Mul, Some(ConstValue::I32(1)), _) => Some(Inst::Move { dest, src: rhs }),
                        (BinOp::Mul, _, Some(ConstValue::I32(0)))
                        | (BinOp::Mul, Some(ConstValue::I32(0)), _) => Some(Inst::LoadConst {
                            dest,
                            value: ConstValue::I32(0),
                        }),
                        (BinOp::Div, _, Some(ConstValue::I32(1))) => Some(Inst::Move { dest, src: lhs }),
                        (BinOp::And, _, Some(ConstValue::Bool(true))) => Some(Inst::Move { dest, src: lhs }),
                        (BinOp::And, Some(ConstValue::Bool(true)), _) => Some(Inst::Move { dest, src: rhs }),
                        (BinOp::And, _, Some(ConstValue::Bool(false)))
                        | (BinOp::And, Some(ConstValue::Bool(false)), _) => Some(Inst::LoadConst {
                            dest,
                            value: ConstValue::Bool(false),
                        }),
                        (BinOp::Or, _, Some(ConstValue::Bool(false))) => Some(Inst::Move { dest, src: lhs }),
                        (BinOp::Or, Some(ConstValue::Bool(false)), _) => Some(Inst::Move { dest, src: rhs }),
                        (BinOp::Or, _, Some(ConstValue::Bool(true)))
                        | (BinOp::Or, Some(ConstValue::Bool(true)), _) => Some(Inst::LoadConst {
                            dest,
                            value: ConstValue::Bool(true),
                        }),
                        (BinOp::Mul, _, Some(ConstValue::I32(2))) if ty == Type::I32 => {
                            Some(Inst::Bin {
                                dest,
                                op: BinOp::Add,
                                ty,
                                lhs,
                                rhs: lhs,
                            })
                        }
                        _ => None,
                    };
                    if let Some(r) = rewritten {
                        if let Inst::LoadConst { value, .. } = &r {
                            consts.insert(dest.0, value.clone());
                        } else {
                            consts.remove(&dest.0);
                        }
                        *inst = r;
                    } else {
                        consts.remove(&dest.0);
                    }
                }
            }
        }
    }
}

/// Local common-subexpression elimination (CS143 lecture 14:
/// value numbering inside a basic block).
pub fn pass_local_cse(module: &mut IrModule) {
    for f in &mut module.functions {
        for bb in &mut f.blocks {
            let mut seen: HashMap<(u8, u32, u32), u32> = HashMap::new();
            for inst in &mut bb.insts {
                if let Inst::Bin {
                    dest,
                    op,
                    lhs,
                    rhs,
                    ..
                } = inst.clone()
                {
                    let key = (op as u8, lhs.0, rhs.0);
                    if let Some(&prev) = seen.get(&key) {
                        *inst = Inst::Move {
                            dest,
                            src: Reg(prev),
                        };
                        seen.retain(|(_, a, b), d| {
                            *a != dest.0 && *b != dest.0 && *d != dest.0
                        });
                        continue;
                    }
                    seen.retain(|(_, a, b), d| *a != dest.0 && *b != dest.0 && *d != dest.0);
                    seen.insert(key, dest.0);
                } else if let Some(d) = inst.dest_reg() {
                    seen.retain(|(_, a, b), prev| *a != d.0 && *b != d.0 && *prev != d.0);
                }
            }
        }
    }
}

pub fn pass_copy_prop(module: &mut IrModule) {
    for f in &mut module.functions {
        for bb in &mut f.blocks {
        let mut alias: HashMap<u32, u32> = HashMap::new();
            for inst in &mut bb.insts {
                // resolve uses
                rewrite_uses(inst, &alias);
                if let Inst::Move { dest, src } = inst {
                    if dest.0 != src.0 {
                        alias.retain(|_, v| *v != dest.0);
                        alias.insert(dest.0, resolve(&alias, src.0));
                    }
                } else if let Some(d) = inst.dest_reg() {
                    alias.retain(|_, v| *v != d.0);
                    alias.remove(&d.0);
                }
            }
            match &mut bb.term {
                Terminator::Branch { cond, .. } => {
                    cond.0 = resolve(&alias, cond.0);
                }
                Terminator::Return { value: Some(r) } => {
                    r.0 = resolve(&alias, r.0);
                }
                _ => {}
            }
        }
    }
}

fn resolve(alias: &HashMap<u32, u32>, r: u32) -> u32 {
    let mut cur = r;
    let mut guard = 0;
    while let Some(&n) = alias.get(&cur) {
        if n == cur || guard > 64 {
            break;
        }
        cur = n;
        guard += 1;
    }
    cur
}

fn rewrite_uses(inst: &mut Inst, alias: &HashMap<u32, u32>) {
    let map = |r: &mut Reg| r.0 = resolve(alias, r.0);
    match inst {
        Inst::Move { src, .. } | Inst::Un { src, .. } | Inst::Cast { src, .. } => map(src),
        Inst::Bin { lhs, rhs, .. } => {
            map(lhs);
            map(rhs);
        }
        Inst::Call { args, .. } => {
            for a in args {
                map(a);
            }
        }
        Inst::IndexLoad { base, index, .. } => {
            map(base);
            map(index);
        }
        Inst::IndexStore {
            base, index, value, ..
        } => {
            map(base);
            map(index);
            map(value);
        }
        Inst::FieldLoad { base, .. } => map(base),
        Inst::FieldStore { base, value, .. } => {
            map(base);
            map(value);
        }
        _ => {}
    }
}

pub fn pass_cf_simplify(module: &mut IrModule) {
    for f in &mut module.functions {
        // remove empty jump chains: bb with no insts and Jump(t) → retarget
        let mut redirect: HashMap<u32, u32> = HashMap::new();
        for bb in &f.blocks {
            if bb.insts.is_empty() {
                if let Terminator::Jump { target } = bb.term {
                    if target != bb.id {
                        redirect.insert(bb.id.0, target.0);
                    }
                }
            }
        }
        let resolve_bb = |id: crate::ir::BlockId| {
            let mut cur = id.0;
            for _ in 0..16 {
                if let Some(&n) = redirect.get(&cur) {
                    cur = n;
                } else {
                    break;
                }
            }
            crate::ir::BlockId(cur)
        };
        for bb in &mut f.blocks {
            match &mut bb.term {
                Terminator::Jump { target } => *target = resolve_bb(*target),
                Terminator::Branch {
                    then_bb, else_bb, ..
                } => {
                    *then_bb = resolve_bb(*then_bb);
                    *else_bb = resolve_bb(*else_bb);
                }
                _ => {}
            }
        }
        // drop unreachable blocks
        let mut live = HashSet::new();
        if let Some(first) = f.blocks.first() {
            let mut stack = vec![first.id];
            while let Some(id) = stack.pop() {
                if !live.insert(id.0) {
                    continue;
                }
                if let Some(bb) = f.blocks.iter().find(|b| b.id == id) {
                    match bb.term {
                        Terminator::Jump { target } => stack.push(target),
                        Terminator::Branch {
                            then_bb, else_bb, ..
                        } => {
                            stack.push(then_bb);
                            stack.push(else_bb);
                        }
                        _ => {}
                    }
                }
            }
        }
        f.blocks.retain(|b| live.contains(&b.id.0));
    }
}

pub fn pass_dce(module: &mut IrModule) {
    for f in &mut module.functions {
        let lv = liveness::analyze_function(f);
        for bb in &mut f.blocks {
            let mut used = lv.live_out.get(&bb.id.0).cloned().unwrap_or_default();
            match &bb.term {
                Terminator::Branch { cond, .. } => {
                    used.insert(cond.0);
                }
                Terminator::Return { value: Some(r) } => {
                    used.insert(r.0);
                }
                _ => {}
            }
            let mut keep = vec![false; bb.insts.len()];
            for i in (0..bb.insts.len()).rev() {
                let inst = &bb.insts[i];
                let effect = matches!(
                    inst,
                    Inst::Call { .. } | Inst::IndexStore { .. } | Inst::FieldStore { .. }
                );
                let dest_live = inst
                    .dest_reg()
                    .map(|d| used.contains(&d.0))
                    .unwrap_or(false);
                if effect || dest_live {
                    keep[i] = true;
                    if let Some(d) = inst.dest_reg() {
                        used.remove(&d.0);
                    }
                    for u in inst.uses() {
                        used.insert(u.0);
                    }
                }
            }
            let old = std::mem::take(&mut bb.insts);
            bb.insts = old
                .into_iter()
                .enumerate()
                .filter(|(i, inst)| keep[*i] && !matches!(inst, Inst::Nop))
                .map(|(_, inst)| inst)
                .collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{emit_ir, dump_ir};
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::sema::analyze;
    use crate::span::FileId;

    fn compile_ir(src: &str) -> IrModule {
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, _) = analyze(&prog);
        emit_ir(&hir.unwrap())
    }

    #[test]
    fn folds_and_false() {
        let ir = compile_ir("fn main() -> i32 { if false && true { return 1; } return 0; }");
        let (opt, _) = optimize(ir, 2);
        let bc = crate::backend::assemble(&opt);
        let (v, _, _) = crate::vm::execute_captured(&bc).expect("vm");
        assert_eq!(v, crate::vm::Value::I32(0));
    }

    #[test]
    fn folds_1_plus_2() {
        let ir = compile_ir("fn main() -> i32 { return 1 + 2; }");
        let (opt, report) = optimize(ir, 2);
        let text = dump_ir(&opt);
        assert!(text.contains("const 3_i32"), "{text}\n{}", report.summary());
        assert!(!text.contains("add.i32"), "{text}");
    }

    #[test]
    fn dce_removes_dead_let() {
        let ir = compile_ir("fn main() -> i32 { let x = 1 + 2; return 0; }");
        let before = count_insts(&ir);
        let (opt, _) = optimize(ir, 2);
        let after = count_insts(&opt);
        assert!(after <= before);
    }

    #[test]
    fn local_cse_reuses_same_binop() {
        let src = r#"
            fn h(p0: i32) -> i32 {
                let x = p0 + 1;
                let y = p0 + 1;
                return x + y;
            }
            fn main() -> i32 { return h(3); }
        "#;
        let ir = compile_ir(src);
        let (opt, _) = optimize(ir, 2);
        let bc = crate::backend::assemble(&opt);
        let (v, _, _) = crate::vm::execute_captured(&bc).expect("vm");
        assert_eq!(v, crate::vm::Value::I32(8));
    }

    #[test]
    fn folds_opaque_x_minus_x() {
        let src = r#"
            fn main() -> i32 {
                let mut acc = 7;
                if (acc - acc) == 0 { return 1; }
                return 0;
            }
        "#;
        let ir = compile_ir(src);
        let (opt, _) = optimize(ir, 2);
        let text = dump_ir(&opt);
        assert!(
            !text.contains("br %"),
            "opaque (x-x)==0 should become a jump\n{text}"
        );
        let bc = crate::backend::assemble(&opt);
        let (v, _, _) = crate::vm::execute_captured(&bc).expect("vm");
        assert_eq!(v, crate::vm::Value::I32(1));
    }

    #[test]
    fn folds_opaque_x_times_zero() {
        let src = r#"
            fn main() -> i32 {
                let mut acc = 7;
                if (acc * 0) == 0 { return 2; }
                return 9;
            }
        "#;
        let ir = compile_ir(src);
        let (opt, _) = optimize(ir, 2);
        let bc = crate::backend::assemble(&opt);
        let (v, _, _) = crate::vm::execute_captured(&bc).expect("vm");
        assert_eq!(v, crate::vm::Value::I32(2));
    }

    #[test]
    fn does_not_fold_across_reassignment() {
        let src = r#"
            fn h0(p0: i32) -> i32 { return (0 - 10) - (p0 / 5); }
            fn main() -> i32 {
                let mut acc = 0;
                acc = h0(2);
                acc = acc + 2;
                acc = acc + acc;
                return acc;
            }
        "#;
        let ir = compile_ir(src);
        let (opted, _) = optimize(ir, 2);
        let bc = crate::backend::assemble(&opted);
        let (v, _, _) = crate::vm::execute_captured(&bc).expect("vm");
        assert_eq!(v, crate::vm::Value::I32(-16));
    }
}
