//! Optimization pipeline over Aether IR.
//!
//! Each pass is a function `IrModule -> (IrModule, PassStats)`. The driver
//! records per-pass instruction counts so `aether optimize --stats` can show
//! before/after differences.

pub mod inline;
pub mod liveness;
pub mod regalloc;

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

    // cf-simplify runs before inline: lowering leaves a dead block after every
    // `return`, and the inliner only accepts single-block leaves.
    let passes: Vec<(&str, fn(&mut IrModule))> = if level >= 2 {
        vec![
            ("const-fold", pass_const_fold),
            ("algebraic", pass_algebraic),
            ("cf-simplify", pass_cf_simplify),
            ("inline", crate::opt::inline::pass_inline),
            ("local-cse", pass_local_cse),
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

    // Run the sequence to a fixpoint: a later pass often exposes work for an
    // earlier one (inlining feeds folding feeds DCE). Bounded so a pathological
    // module cannot spin; in practice two or three rounds suffice.
    const MAX_ROUNDS: usize = 4;
    for round in 0..MAX_ROUNDS {
        let round_before = count_insts(&module);
        for (name, pass) in &passes {
            let before = count_insts(&module);
            pass(&mut module);
            let after = count_insts(&module);
            if round == 0 || before != after {
                report.passes.push(PassStats {
                    name: name.to_string(),
                    insts_before: before,
                    insts_after: after,
                });
            }
        }
        if level >= 2 {
            let before = count_insts(&module);
            pass_dead_functions(&mut module);
            let after = count_insts(&module);
            if round == 0 || before != after {
                report.passes.push(PassStats {
                    name: "dead-fn".to_string(),
                    insts_before: before,
                    insts_after: after,
                });
            }
        }
        if count_insts(&module) == round_before {
            break;
        }
    }
    if level >= 2 {
        // Register compaction runs once, after the fixpoint: it only renumbers
        // registers, so no earlier pass can benefit from it.
        let regs_before = regalloc::reg_total(&module);
        let before = count_insts(&module);
        regalloc::pass_regalloc(&mut module);
        report.passes.push(PassStats {
            name: format!("regalloc (regs {}→{})", regs_before, regalloc::reg_total(&module)),
            insts_before: before,
            insts_after: count_insts(&module),
        });
    }
    report.insts_after = count_insts(&module);
    (module, report)
}

/// Removes functions that `main` can never reach (typically leaves that were
/// inlined everywhere). Extern declarations are kept: they are bound by the host.
pub fn pass_dead_functions(module: &mut IrModule) {
    let mut live: HashSet<String> = HashSet::new();
    let mut stack = vec!["main".to_string()];
    while let Some(name) = stack.pop() {
        if !live.insert(name.clone()) {
            continue;
        }
        if let Some(f) = module.function(&name) {
            for bb in &f.blocks {
                for inst in &bb.insts {
                    if let Inst::Call { func, .. } = inst {
                        if !live.contains(func) {
                            stack.push(func.clone());
                        }
                    }
                }
            }
        }
    }
    module
        .functions
        .retain(|f| f.is_extern || live.contains(&f.name));
}

/// Compare two constants of the same type the way the VM would.
fn fold_cmp<T: PartialOrd>(op: BinOp, a: T, b: T) -> Option<ConstValue> {
    Some(ConstValue::Bool(match op {
        BinOp::Eq => a == b,
        BinOp::Ne => a != b,
        BinOp::Lt => a < b,
        BinOp::Le => a <= b,
        BinOp::Gt => a > b,
        BinOp::Ge => a >= b,
        _ => return None,
    }))
}

/// Integer arithmetic wraps, exactly like the VM (`i32::MIN / -1` included).
/// Division by zero is left to the VM, which reports it at runtime.
/// Shift amounts are masked to the bit width (`b & 31` for `i32`, `b & 63`
/// for `i64`), so shifting by the width or by a negative amount never traps;
/// `>>` is arithmetic (sign-filling). The VM must do the same.
fn fold_bin(op: BinOp, _ty: &Type, lhs: &ConstValue, rhs: &ConstValue) -> Option<ConstValue> {
    use ConstValue::*;
    match (op, lhs, rhs) {
        (BinOp::Add, I32(a), I32(b)) => Some(I32(a.wrapping_add(*b))),
        (BinOp::Sub, I32(a), I32(b)) => Some(I32(a.wrapping_sub(*b))),
        (BinOp::Mul, I32(a), I32(b)) => Some(I32(a.wrapping_mul(*b))),
        (BinOp::Div, I32(a), I32(b)) if *b != 0 => Some(I32(a.wrapping_div(*b))),
        (BinOp::Rem, I32(a), I32(b)) if *b != 0 => Some(I32(a.wrapping_rem(*b))),
        (BinOp::BitAnd, I32(a), I32(b)) => Some(I32(a & b)),
        (BinOp::BitOr, I32(a), I32(b)) => Some(I32(a | b)),
        (BinOp::BitXor, I32(a), I32(b)) => Some(I32(a ^ b)),
        (BinOp::Shl, I32(a), I32(b)) => Some(I32(a.wrapping_shl((*b & 31) as u32))),
        (BinOp::Shr, I32(a), I32(b)) => Some(I32(a.wrapping_shr((*b & 31) as u32))),
        (BinOp::Add, I64(a), I64(b)) => Some(I64(a.wrapping_add(*b))),
        (BinOp::Sub, I64(a), I64(b)) => Some(I64(a.wrapping_sub(*b))),
        (BinOp::Mul, I64(a), I64(b)) => Some(I64(a.wrapping_mul(*b))),
        (BinOp::Div, I64(a), I64(b)) if *b != 0 => Some(I64(a.wrapping_div(*b))),
        (BinOp::Rem, I64(a), I64(b)) if *b != 0 => Some(I64(a.wrapping_rem(*b))),
        (BinOp::BitAnd, I64(a), I64(b)) => Some(I64(a & b)),
        (BinOp::BitOr, I64(a), I64(b)) => Some(I64(a | b)),
        (BinOp::BitXor, I64(a), I64(b)) => Some(I64(a ^ b)),
        (BinOp::Shl, I64(a), I64(b)) => Some(I64(a.wrapping_shl((*b & 63) as u32))),
        (BinOp::Shr, I64(a), I64(b)) => Some(I64(a.wrapping_shr((*b & 63) as u32))),
        (BinOp::Add, F64(a), F64(b)) => Some(F64(a + b)),
        (BinOp::Sub, F64(a), F64(b)) => Some(F64(a - b)),
        (BinOp::Mul, F64(a), F64(b)) => Some(F64(a * b)),
        (BinOp::Div, F64(a), F64(b)) => Some(F64(a / b)),
        (BinOp::And, Bool(a), Bool(b)) => Some(Bool(*a && *b)),
        (BinOp::Or, Bool(a), Bool(b)) => Some(Bool(*a || *b)),
        (BinOp::Add, String(a), String(b)) => Some(String(format!("{a}{b}"))),
        (_, I32(a), I32(b)) => fold_cmp(op, a, b),
        (_, I64(a), I64(b)) => fold_cmp(op, a, b),
        (_, F64(a), F64(b)) => fold_cmp(op, a, b),
        (_, Char(a), Char(b)) => fold_cmp(op, a, b),
        (BinOp::Eq, Bool(a), Bool(b)) => Some(Bool(a == b)),
        (BinOp::Ne, Bool(a), Bool(b)) => Some(Bool(a != b)),
        (BinOp::Eq, String(a), String(b)) => Some(Bool(a == b)),
        (BinOp::Ne, String(a), String(b)) => Some(Bool(a != b)),
        _ => None,
    }
}

fn fold_un(op: UnOp, src: &ConstValue) -> Option<ConstValue> {
    match (op, src) {
        (UnOp::Neg, ConstValue::I32(v)) => Some(ConstValue::I32(v.wrapping_neg())),
        (UnOp::Neg, ConstValue::I64(v)) => Some(ConstValue::I64(v.wrapping_neg())),
        (UnOp::Neg, ConstValue::F64(v)) => Some(ConstValue::F64(-v)),
        (UnOp::Not, ConstValue::Bool(v)) => Some(ConstValue::Bool(!v)),
        // `!` on integers is bitwise not
        (UnOp::Not, ConstValue::I32(v)) => Some(ConstValue::I32(!v)),
        (UnOp::Not, ConstValue::I64(v)) => Some(ConstValue::I64(!v)),
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
                    // Integer identities only: for f64 `x - x`, `x * 0` and
                    // `x == x` are not constants when `x` is NaN or infinite.
                    let is_int = ty.is_integer();
                    let same = lhs.0 == rhs.0;
                    let zero = int_const(&ty, 0);
                    let rewritten = match (op, lconst, rconst) {
                        // Opaque identities: same register, no const required.
                        (BinOp::Sub, _, _) if same && is_int => {
                            Some(Inst::LoadConst { dest, value: zero })
                        }
                        (BinOp::Eq, _, _) | (BinOp::Le, _, _) | (BinOp::Ge, _, _)
                            if same && is_int =>
                        {
                            Some(Inst::LoadConst {
                                dest,
                                value: ConstValue::Bool(true),
                            })
                        }
                        (BinOp::Ne, _, _) | (BinOp::Lt, _, _) | (BinOp::Gt, _, _)
                            if same && is_int =>
                        {
                            Some(Inst::LoadConst {
                                dest,
                                value: ConstValue::Bool(false),
                            })
                        }
                        // x ^ x → 0; x & x → x; x | x → x
                        (BinOp::BitXor, _, _) if same && is_int => {
                            Some(Inst::LoadConst { dest, value: zero })
                        }
                        (BinOp::BitAnd, _, _) | (BinOp::BitOr, _, _) if same && is_int => {
                            Some(Inst::Move { dest, src: lhs })
                        }
                        // x & 0 → 0; x | 0 → x; x ^ 0 → x; x << 0 → x; x >> 0 → x
                        (BinOp::BitAnd, _, Some(c)) | (BinOp::BitAnd, Some(c), _)
                            if is_int && is_int_const(c, 0) =>
                        {
                            Some(Inst::LoadConst { dest, value: zero })
                        }
                        (BinOp::BitOr, _, Some(c)) | (BinOp::BitXor, _, Some(c))
                            if is_int && is_int_const(c, 0) =>
                        {
                            Some(Inst::Move { dest, src: lhs })
                        }
                        (BinOp::BitOr, Some(c), _) | (BinOp::BitXor, Some(c), _)
                            if is_int && is_int_const(c, 0) =>
                        {
                            Some(Inst::Move { dest, src: rhs })
                        }
                        (BinOp::Shl, _, Some(c)) | (BinOp::Shr, _, Some(c))
                            if is_int && is_int_const(c, 0) =>
                        {
                            Some(Inst::Move { dest, src: lhs })
                        }
                        (BinOp::Add, _, Some(c)) if is_int && is_int_const(c, 0) => {
                            Some(Inst::Move { dest, src: lhs })
                        }
                        (BinOp::Add, Some(c), _) if is_int && is_int_const(c, 0) => {
                            Some(Inst::Move { dest, src: rhs })
                        }
                        (BinOp::Sub, _, Some(c)) if is_int && is_int_const(c, 0) => {
                            Some(Inst::Move { dest, src: lhs })
                        }
                        (BinOp::Mul, _, Some(c)) if is_int && is_int_const(c, 1) => {
                            Some(Inst::Move { dest, src: lhs })
                        }
                        (BinOp::Mul, Some(c), _) if is_int && is_int_const(c, 1) => {
                            Some(Inst::Move { dest, src: rhs })
                        }
                        (BinOp::Mul, _, Some(c)) | (BinOp::Mul, Some(c), _)
                            if is_int && is_int_const(c, 0) =>
                        {
                            Some(Inst::LoadConst { dest, value: zero })
                        }
                        (BinOp::Div, _, Some(c)) if is_int && is_int_const(c, 1) => {
                            Some(Inst::Move { dest, src: lhs })
                        }
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
                        (BinOp::Mul, _, Some(c)) if is_int && is_int_const(c, 2) => {
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

fn int_const(ty: &Type, v: i64) -> ConstValue {
    if *ty == Type::I64 {
        ConstValue::I64(v)
    } else {
        ConstValue::I32(v as i32)
    }
}

fn is_int_const(c: &ConstValue, v: i64) -> bool {
    match c {
        ConstValue::I32(x) => i64::from(*x) == v,
        ConstValue::I64(x) => *x == v,
        _ => false,
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

/// Replaces uses of a `Move` destination by its source within a block.
///
/// Aggregates have value semantics (`Move` copies the array/struct), so a
/// store through a register must neither be redirected to the register it
/// was copied from, nor leave aliases alive on either side of the copy.
pub fn pass_copy_prop(module: &mut IrModule) {
    for f in &mut module.functions {
        for bb in &mut f.blocks {
            let mut alias: HashMap<u32, u32> = HashMap::new();
            for inst in &mut bb.insts {
                // resolve uses
                rewrite_uses(inst, &alias);
                if let Inst::IndexStore { base, .. } | Inst::FieldStore { base, .. } = inst {
                    let b = base.0;
                    alias.retain(|k, v| *k != b && *v != b);
                    continue;
                }
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
        // the base of a store is mutated in place: never redirect it
        Inst::IndexStore { index, value, .. } => {
            map(index);
            map(value);
        }
        Inst::FieldLoad { base, .. } => map(base),
        Inst::FieldStore { value, .. } => map(value),
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
                // Besides calls and stores, an instruction that can trap at
                // run time is observable even when its result is unused:
                // integer `/` and `%` (division by zero) and indexing
                // (bounds). Removing them would make -O1/-O2 succeed where
                // -O0 reports the error.
                let effect = matches!(
                    inst,
                    Inst::Call { .. }
                        | Inst::IndexStore { .. }
                        | Inst::FieldStore { .. }
                        | Inst::IndexLoad { .. }
                        | Inst::Yield
                ) || matches!(
                    inst,
                    Inst::Bin { op: BinOp::Div | BinOp::Rem, ty, .. } if ty.is_integer()
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
        let bc = crate::backend::assemble(&opt).expect("assemble");
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
    fn folds_bitwise_and_shift_constants() {
        for (expr, want) in [
            ("0xFF & 0b1010", "10_i32"),
            ("0xF0 | 0x0F", "255_i32"),
            ("0xFF ^ 0x0F", "240_i32"),
            ("1 << 4", "16_i32"),
            ("-16 >> 2", "-4_i32"),
            ("!0", "-1_i32"),
            ("1 << 32", "1_i32"),  // amount masked to 5 bits
            ("1 << -1", "-2147483648_i32"),  // -1 & 31 == 31
            ("1 << 31", "-2147483648_i32"),
            ("(0 - 1) >> 31", "-1_i32"),
        ] {
            let ir = compile_ir(&format!("fn main() -> i32 {{ return {expr}; }}"));
            let (opt, _) = optimize(ir, 2);
            let text = dump_ir(&opt);
            assert!(text.contains(&format!("const {want}")), "{expr}:\n{text}");
        }
        for (expr, want) in [
            ("1 << 40", "1099511627776_i64"),
            ("1 << 64", "1_i64"),
            ("!0", "-1_i64"),
            ("0x7FFF_FFFF_FFFF_FFFF & 0xFF", "255_i64"),
        ] {
            let ir = compile_ir(&format!(
                "fn main() -> i32 {{ let x: i64 = {expr}; print_i64(x); return 0; }}"
            ));
            let (opt, _) = optimize(ir, 2);
            let text = dump_ir(&opt);
            assert!(text.contains(&format!("const {want}")), "{expr}:\n{text}");
        }
    }

    #[test]
    fn algebraic_bitwise_identities() {
        // `x` comes from an extern call, so it is neither constant nor
        // inlinable: the Bin must disappear through the identity alone.
        for (body, op_text) in [
            ("x & 0", "&.i32"),
            ("0 & x", "&.i32"),
            ("x | 0", "|.i32"),
            ("0 | x", "|.i32"),
            ("x ^ 0", "^.i32"),
            ("x ^ x", "^.i32"),
            ("x & x", "&.i32"),
            ("x | x", "|.i32"),
            ("x << 0", "<<.i32"),
            ("x >> 0", ">>.i32"),
        ] {
            let ir = compile_ir(&format!(
                "extern fn opaque() -> i32;\nfn main() -> i32 {{ let x = opaque(); return {body}; }}"
            ));
            let (opt, _) = optimize(ir, 2);
            let text = dump_ir(&opt);
            assert!(!text.contains(op_text), "{body}:\n{text}");
        }
        // not an identity: `x & 1`, `x ^ 1`, `x << 1` stay
        let ir = compile_ir(
            "extern fn opaque() -> i32;\nfn main() -> i32 { let x = opaque(); return (x & 1) + (x ^ 1) + (x << 1); }",
        );
        let (opt, _) = optimize(ir, 2);
        let text = dump_ir(&opt);
        assert!(
            text.contains("&.i32") && text.contains("^.i32") && text.contains("<<.i32"),
            "{text}"
        );
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
        let bc = crate::backend::assemble(&opt).expect("assemble");
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
        let bc = crate::backend::assemble(&opt).expect("assemble");
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
        let bc = crate::backend::assemble(&opt).expect("assemble");
        let (v, _, _) = crate::vm::execute_captured(&bc).expect("vm");
        assert_eq!(v, crate::vm::Value::I32(2));
    }

    #[test]
    fn inlined_leaf_is_removed_and_fixpoint_folds_through() {
        let src = r#"
            fn add1(x: i32) -> i32 { return x + 1; }
            fn twice(x: i32) -> i32 { return add1(add1(x)); }
            fn main() -> i32 { return twice(40); }
        "#;
        let ir = compile_ir(src);
        let (opt, report) = optimize(ir, 2);
        let text = dump_ir(&opt);
        assert!(!text.contains("fn add1"), "add1 should be dead after inlining\n{text}");
        assert!(!text.contains("fn twice"), "twice should be dead after inlining\n{text}");
        assert!(
            text.contains("const 42_i32"),
            "nested inlining should fold to 42\n{text}\n{}",
            report.summary()
        );
    }

    #[test]
    fn keeps_functions_main_reaches() {
        let src = r#"
            fn loud(n: i32) -> i32 { print_i32(n); return n; }
            fn fib(n: i32) -> i32 { if n < 2 { return n; } return fib(n - 1) + fib(n - 2); }
            fn main() -> i32 { return loud(fib(5)); }
        "#;
        let ir = compile_ir(src);
        let (opt, _) = optimize(ir, 2);
        assert!(opt.function("fib").is_some());
        assert!(opt.function("loud").is_some() || dump_ir(&opt).contains("print_i32"));
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
        let bc = crate::backend::assemble(&opted).expect("assemble");
        let (v, _, _) = crate::vm::execute_captured(&bc).expect("vm");
        assert_eq!(v, crate::vm::Value::I32(-16));
    }
}
