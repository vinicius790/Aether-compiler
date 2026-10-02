//! Register compaction for the non-SSA IR.
//!
//! Lowering never reuses a virtual register and the inliner shifts every
//! callee register past the caller's `reg_count`, so after inlining a
//! function may declare thousands of registers while only a few are live at
//! any point. The VM allocates `vec![Value::Unit; nregs]` per call frame, so
//! every spare register costs time and memory on every call.
//!
//! This pass renumbers registers per function to a minimal count:
//!
//! * live ranges are built at **block granularity**: a register is live in a
//!   block if it is live-in or live-out there, or defined or used anywhere in
//!   it (terminator included). Two registers interfere whenever both are live
//!   in the same block. This is coarse (nothing is reused inside a block) but
//!   trivially safe for non-SSA reassignments and for aggregates with value
//!   semantics (an `IndexStore`/`FieldStore` base counts as a use);
//! * registers are greedy-colored in order of first appearance; parameters
//!   keep their fixed registers and nothing else may take them;
//! * registers that nothing mentions any more (DCE'd temporaries, shifted
//!   inliner copies) are dropped.
//!
//! The block-level sets are seeded from `liveness::analyze_function` and then
//! re-iterated to a fixpoint with a transfer function that removes the
//! destination *before* adding the uses (so `r = call f(r)` keeps `r` live
//! across the call). If anything looks wrong (a register past `reg_count`,
//! duplicate block ids, no convergence) the function is left untouched.

use crate::ir::{BasicBlock, Inst, IrFunction, IrModule, Reg, Terminator};
use crate::ty::Type;
use crate::opt::liveness::{self, LiveSet};
use std::collections::{HashMap, HashSet};

/// Sum of `reg_count` over every function in the module.
pub fn reg_total(m: &IrModule) -> u32 {
    m.functions.iter().map(|f| f.reg_count).sum()
}

pub fn pass_regalloc(module: &mut IrModule) {
    // Return types of every callee, so a call destination gets a type class.
    let mut rets: HashMap<String, Type> = module
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.return_ty.clone()))
        .collect();
    for n in ["print", "println", "print_i32", "print_i64", "print_f64", "print_bool", "assert"] {
        rets.entry(n.to_string()).or_insert(Type::Unit);
    }
    rets.entry("len".to_string()).or_insert(Type::I32);
    for f in &mut module.functions {
        if let Some((map, count)) = plan(f, &rets) {
            apply(f, &map, count);
        }
    }
}

/// Type of each register, inferred from its definitions (`None` when unknown
/// or inconsistent). Registers only share a color with registers of the same
/// type: the VM would not care, but the LLVM emitter gives every register one
/// typed `alloca`.
fn reg_types(f: &IrFunction, rets: &HashMap<String, Type>) -> Vec<Option<Type>> {
    let n = f.reg_count as usize;
    let mut ty: Vec<Option<Type>> = vec![None; n];
    let mut conflict = vec![false; n];
    for (_, t, r) in &f.params {
        if (r.0 as usize) < n {
            ty[r.0 as usize] = Some(t.clone());
        }
    }
    let set = |ty: &mut Vec<Option<Type>>, conflict: &mut Vec<bool>, r: Reg, t: Option<Type>| {
        let i = r.0 as usize;
        if i >= n || conflict[i] {
            return false;
        }
        match (&ty[i], t) {
            (_, None) => false,
            (None, Some(t)) => {
                ty[i] = Some(t);
                true
            }
            (Some(old), Some(t)) if *old == t => false,
            (Some(_), Some(_)) => {
                conflict[i] = true;
                ty[i] = None;
                false
            }
        }
    };
    // Moves copy the type of their source; iterate until nothing changes.
    for _ in 0..8 {
        let mut changed = false;
        for bb in &f.blocks {
            for inst in &bb.insts {
                let (dest, t) = match inst {
                    Inst::LoadConst { dest, value } => (*dest, Some(value.ty())),
                    Inst::Move { dest, src } => (*dest, ty.get(src.0 as usize).cloned().flatten()),
                    Inst::Bin { dest, op, ty: t, .. } => {
                        (*dest, Some(if op.is_cmp() { Type::Bool } else { t.clone() }))
                    }
                    Inst::Un { dest, ty: t, .. } => (*dest, Some(t.clone())),
                    Inst::Cast { dest, to, .. } => (*dest, Some(to.clone())),
                    Inst::IndexLoad { dest, elem, .. } => (*dest, Some(elem.clone())),
                    Inst::FieldLoad { dest, ty: t, .. } => (*dest, Some(t.clone())),
                    Inst::AllocArray { dest, elem, len } => (
                        *dest,
                        Some(Type::Array {
                            elem: Box::new(elem.clone()),
                            len: *len,
                        }),
                    ),
                    Inst::AllocStruct { dest, ty: t } => (*dest, Some(t.clone())),
                    Inst::Call {
                        dest: Some(dest),
                        func,
                        ..
                    } => (*dest, rets.get(func).cloned()),
                    _ => continue,
                };
                changed |= set(&mut ty, &mut conflict, dest, t);
            }
        }
        if !changed {
            break;
        }
    }
    ty
}

fn term_reg(t: &Terminator) -> Option<Reg> {
    match t {
        Terminator::Branch { cond, .. } => Some(*cond),
        Terminator::Return { value } => *value,
        Terminator::Jump { .. } | Terminator::Unreachable => None,
    }
}

fn block_out(bb: &BasicBlock, live_in: &HashMap<u32, LiveSet>) -> LiveSet {
    let mut out = LiveSet::new();
    match &bb.term {
        Terminator::Jump { target } => {
            if let Some(s) = live_in.get(&target.0) {
                out.extend(s.iter().copied());
            }
        }
        Terminator::Branch {
            cond,
            then_bb,
            else_bb,
        } => {
            out.insert(cond.0);
            if let Some(s) = live_in.get(&then_bb.0) {
                out.extend(s.iter().copied());
            }
            if let Some(s) = live_in.get(&else_bb.0) {
                out.extend(s.iter().copied());
            }
        }
        Terminator::Return { value: Some(r) } => {
            out.insert(r.0);
        }
        Terminator::Return { value: None } | Terminator::Unreachable => {}
    }
    out
}

/// Exact backward transfer: kill the destination, then gen the uses.
fn block_in(bb: &BasicBlock, out: &LiveSet) -> LiveSet {
    let mut live = out.clone();
    for inst in bb.insts.iter().rev() {
        if let Some(d) = inst.dest_reg() {
            live.remove(&d.0);
        }
        for u in inst.uses() {
            live.insert(u.0);
        }
    }
    live
}

/// Seeds from the shared liveness analysis and iterates to a fixpoint of the
/// exact transfer. Returns `None` if the iteration does not settle.
fn refined_liveness(f: &IrFunction) -> Option<liveness::Liveness> {
    let mut lv = liveness::analyze_function(f);
    let limit = 2 * f.blocks.len() + 16;
    let mut iters = 0usize;
    loop {
        let mut changed = false;
        for bb in f.blocks.iter().rev() {
            let out = block_out(bb, &lv.live_in);
            let inn = block_in(bb, &out);
            if lv.live_out.get(&bb.id.0) != Some(&out) {
                lv.live_out.insert(bb.id.0, out);
                changed = true;
            }
            if lv.live_in.get(&bb.id.0) != Some(&inn) {
                lv.live_in.insert(bb.id.0, inn);
                changed = true;
            }
        }
        if !changed {
            return Some(lv);
        }
        iters += 1;
        if iters > limit {
            return None;
        }
    }
}

fn touch(r: Reg, mentioned: &mut [bool], order: &mut Vec<u32>) -> bool {
    let i = r.0 as usize;
    if i >= mentioned.len() {
        return false;
    }
    if !mentioned[i] {
        mentioned[i] = true;
        order.push(r.0);
    }
    true
}

fn bit_set(words: &mut [u64], bit: usize) {
    words[bit / 64] |= 1u64 << (bit % 64);
}


/// Computes `(old -> new)` and the new register count, or `None` to leave the
/// function untouched.
/// First color that is free in every block of the register and holds either
/// nothing yet or exactly the register's type.
fn pick_color(
    forbidden: &[u64],
    color_type: &[Option<Type>],
    ty: &Option<Type>,
    limit: usize,
) -> Option<usize> {
    (0..limit)
        .filter(|&c| forbidden[c / 64] & (1u64 << (c % 64)) == 0)
        .find(|&c| match (&color_type[c], ty) {
            (None, _) => true,
            (Some(held), Some(t)) => held == t,
            (Some(_), None) => false,
        })
}

fn plan(f: &IrFunction, rets: &HashMap<String, Type>) -> Option<(Vec<u32>, u32)> {
    if f.is_extern || f.blocks.is_empty() {
        return None;
    }
    let n = f.reg_count as usize;
    let types = reg_types(f, rets);
    let mut ids = HashSet::new();
    if !f.blocks.iter().all(|b| ids.insert(b.id.0)) {
        return None;
    }

    // First-appearance order; parameters first. Any register >= reg_count
    // aborts the pass for this function.
    let mut mentioned = vec![false; n];
    let mut order: Vec<u32> = Vec::new();
    let mut is_param = vec![false; n];
    for (_, _, r) in &f.params {
        if !touch(*r, &mut mentioned, &mut order) {
            return None;
        }
        let i = r.0 as usize;
        if is_param[i] {
            return None; // duplicate parameter register
        }
        is_param[i] = true;
    }
    for bb in &f.blocks {
        for inst in &bb.insts {
            for u in inst.uses() {
                if !touch(u, &mut mentioned, &mut order) {
                    return None;
                }
            }
            if let Some(d) = inst.dest_reg() {
                if !touch(d, &mut mentioned, &mut order) {
                    return None;
                }
            }
        }
        if let Some(r) = term_reg(&bb.term) {
            if !touch(r, &mut mentioned, &mut order) {
                return None;
            }
        }
    }
    if order.is_empty() {
        return None;
    }

    let lv = refined_liveness(f)?;

    // blocks_of[r] = indices of the blocks where r is live (block granularity).
    let mut blocks_of: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (bi, bb) in f.blocks.iter().enumerate() {
        let mut set: HashSet<u32> = HashSet::new();
        if let Some(s) = lv.live_in.get(&bb.id.0) {
            set.extend(s.iter().copied());
        }
        if let Some(s) = lv.live_out.get(&bb.id.0) {
            set.extend(s.iter().copied());
        }
        for inst in &bb.insts {
            for u in inst.uses() {
                set.insert(u.0);
            }
            if let Some(d) = inst.dest_reg() {
                set.insert(d.0);
            }
        }
        if let Some(r) = term_reg(&bb.term) {
            set.insert(r.0);
        }
        for r in set {
            let i = r as usize;
            if i >= n || !mentioned[i] {
                return None;
            }
            blocks_of[i].push(bi as u32);
        }
    }

    // Greedy coloring. `used[b]` is the set of colors already given to
    // registers live in block b; a register's forbidden set is the union over
    // its blocks plus every parameter register.
    let words = (n + 63) / 64;
    let mut reserved = vec![0u64; words];
    for (i, p) in is_param.iter().enumerate() {
        if *p {
            bit_set(&mut reserved, i);
        }
    }
    let mut used: Vec<Vec<u64>> = vec![vec![0u64; words]; f.blocks.len()];
    let mut map = vec![u32::MAX; n];
    let mut forbidden = vec![0u64; words];
    let mut max_color = 0usize;
    // Type held by each color; a register of unknown type gets a color of its
    // own (`Type::Error` marks it so nothing else matches it).
    let mut color_type: Vec<Option<Type>> = vec![None; n];
    for &r in &order {
        let i = r as usize;
        let color = if is_param[i] {
            i
        } else {
            forbidden.copy_from_slice(&reserved);
            for &b in &blocks_of[i] {
                for (w, word) in used[b as usize].iter().enumerate() {
                    forbidden[w] |= *word;
                }
            }
            pick_color(&forbidden, &color_type, &types[i], n)?
        };
        color_type[color] = Some(types[i].clone().unwrap_or(Type::Error));
        map[i] = color as u32;
        max_color = max_color.max(color);
        for &b in &blocks_of[i] {
            bit_set(&mut used[b as usize], color);
        }
    }

    let count = (max_color + 1).max(f.params.len()) as u32;
    if count > f.reg_count {
        return None;
    }
    Some((map, count))
}

fn apply(f: &mut IrFunction, map: &[u32], count: u32) {
    let m = |r: &mut Reg| r.0 = map[r.0 as usize];
    for p in &mut f.params {
        m(&mut p.2);
    }
    for bb in &mut f.blocks {
        for inst in &mut bb.insts {
            match inst {
                Inst::LoadConst { dest, .. }
                | Inst::AllocArray { dest, .. }
                | Inst::AllocStruct { dest, .. } => m(dest),
                Inst::Move { dest, src }
                | Inst::Un { dest, src, .. }
                | Inst::Cast { dest, src, .. } => {
                    m(dest);
                    m(src);
                }
                Inst::Bin { dest, lhs, rhs, .. } => {
                    m(dest);
                    m(lhs);
                    m(rhs);
                }
                Inst::Call { dest, args, .. } => {
                    if let Some(d) = dest {
                        m(d);
                    }
                    for a in args {
                        m(a);
                    }
                }
                Inst::IndexLoad {
                    dest, base, index, ..
                } => {
                    m(dest);
                    m(base);
                    m(index);
                }
                Inst::IndexStore {
                    base, index, value, ..
                } => {
                    m(base);
                    m(index);
                    m(value);
                }
                Inst::FieldLoad { dest, base, .. } => {
                    m(dest);
                    m(base);
                }
                Inst::FieldStore { base, value, .. } => {
                    m(base);
                    m(value);
                }
                Inst::Yield | Inst::Nop => {}
            }
        }
        match &mut bb.term {
            Terminator::Branch { cond, .. } => m(cond),
            Terminator::Return { value: Some(r) } => m(r),
            Terminator::Jump { .. } | Terminator::Return { value: None } | Terminator::Unreachable => {}
        }
    }
    f.reg_count = count;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinOp;
    use crate::ir::{emit_ir, verify::verify_module, BlockId, ConstValue};
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::sema::analyze;
    use crate::span::FileId;
    use crate::ty::Type;
    use crate::vm::Value;

    fn compile_ir(src: &str) -> IrModule {
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, _) = analyze(&prog);
        emit_ir(&hir.unwrap())
    }

    fn run(m: &IrModule) -> (Value, String) {
        let bc = crate::backend::assemble(m).expect("assemble");
        let (v, out, _) = crate::vm::execute_captured(&bc).expect("vm run");
        (v, out)
    }

    fn func<'a>(m: &'a IrModule, name: &str) -> &'a IrFunction {
        m.function(name).expect("function present")
    }

    const FIB_LOOP: &str = r#"
fn fib(n: i32) -> i32 {
    if n < 2 {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}

fn main() -> i32 {
    let mut acc = 0;
    let mut i = 0;
    while i < 10 {
        acc = acc + fib(i);
        i = i + 1;
    }
    for j in 0..5 {
        acc = acc + j;
    }
    print_i32(acc);
    return acc;
}
"#;

    #[test]
    fn fib_and_loop_match_o0_after_o2() {
        let ir0 = compile_ir(FIB_LOOP);
        let (ir2, report) = crate::opt::optimize(ir0.clone(), 2);
        assert!(verify_module(&ir2).is_ok(), "{:?}", verify_module(&ir2));
        assert!(
            report.passes.iter().any(|p| p.name.starts_with("regalloc (regs ")),
            "regalloc stats missing: {:?}",
            report.passes.iter().map(|p| p.name.clone()).collect::<Vec<_>>()
        );
        assert!(reg_total(&ir2) <= reg_total(&ir0));
        assert_eq!(run(&ir0), run(&ir2));
        assert_eq!(run(&ir2).0, Value::I32(98));

        // Regalloc alone on the unoptimized IR is also semantics-preserving.
        let mut ir_ra = ir0.clone();
        pass_regalloc(&mut ir_ra);
        assert!(verify_module(&ir_ra).is_ok());
        assert_eq!(run(&ir0), run(&ir_ra));
    }

    #[test]
    fn three_hundred_temporaries_compact() {
        let mut src = String::from("fn main() -> i32 {\n");
        for n in 0..300 {
            src.push_str(&format!("    let v{n} = {n} + 1;\n"));
        }
        src.push_str("    let mut s = 0;\n");
        for n in 0..300 {
            src.push_str(&format!("    s = s + v{n};\n"));
        }
        src.push_str("    return s;\n}\n");
        let ir0 = compile_ir(&src);
        assert!(func(&ir0, "main").reg_count >= 300);
        let (ir2, _) = crate::opt::optimize(ir0, 2);
        let main = func(&ir2, "main");
        assert!(main.reg_count < 300, "reg_count = {}", main.reg_count);
        assert_eq!(run(&ir2).0, Value::I32(45150));
    }

    #[test]
    fn block_local_temporaries_share_registers() {
        // Each `if` body is its own block with private temporaries; regalloc
        // alone (no other pass) must fold them onto a handful of registers.
        // 20 bodies keep the unoptimized function under the bytecode's 255
        // register limit (registers are encoded as u8), so -O0 is a valid oracle.
        let mut src = String::from("fn main() -> i32 {\n    let mut s = 0;\n");
        for n in 0..20 {
            src.push_str(&format!(
                "    if s < 1000000 {{ let a{n} = {n} * 2; let b{n} = a{n} + 3; s = s + b{n}; }}\n"
            ));
        }
        src.push_str("    return s;\n}\n");
        let ir0 = compile_ir(&src);
        let before = func(&ir0, "main").reg_count;
        assert!(before < 256, "test program must fit the -O0 VM: {before} regs");
        let mut ir = ir0.clone();
        pass_regalloc(&mut ir);
        let after = func(&ir, "main").reg_count;
        assert!(verify_module(&ir).is_ok());
        assert!(after * 4 < before, "reg_count {before} -> {after}");
        assert_eq!(run(&ir0), run(&ir));
        assert_eq!(run(&ir).0, Value::I32(440));
    }

    #[test]
    fn params_keep_their_indices() {
        let src = r#"
fn add3(a: i32, b: i32, c: i32) -> i32 {
    let t = a + b;
    let u = t + c;
    if u > 100 {
        let big = u * 2;
        return big;
    }
    return u;
}
fn main() -> i32 { return add3(1, 2, 3) + add3(100, 2, 3); }
"#;
        let ir0 = compile_ir(src);
        let mut ir = ir0.clone();
        pass_regalloc(&mut ir);
        let f = func(&ir, "add3");
        for (i, p) in f.params.iter().enumerate() {
            assert_eq!(p.2, Reg(i as u32), "param {i} moved");
        }
        // No other value may land on a parameter register.
        for bb in &f.blocks {
            for inst in &bb.insts {
                if let Some(d) = inst.dest_reg() {
                    assert!(d.0 >= f.params.len() as u32, "{inst:?} clobbers a parameter");
                }
            }
        }
        assert!(f.reg_count < func(&ir0, "add3").reg_count);
        assert_eq!(run(&ir0), run(&ir));
        assert_eq!(run(&ir).0, Value::I32(6 + 210));

        let (ir2, _) = crate::opt::optimize(ir0.clone(), 2);
        assert_eq!(run(&ir2).0, Value::I32(216));
        for f in &ir2.functions {
            for (i, p) in f.params.iter().enumerate() {
                assert_eq!(p.2, Reg(i as u32));
            }
        }
    }

    #[test]
    fn loop_header_reassignment_read_after_loop() {
        let src = r#"
fn main() -> i32 {
    let mut s = 0;
    let mut i = 0;
    while i < 10 {
        s = s + i;
        i = i + 1;
    }
    let t = s * 2;
    return s + t;
}
"#;
        let ir0 = compile_ir(src);
        let mut ir_ra = ir0.clone();
        pass_regalloc(&mut ir_ra);
        assert!(verify_module(&ir_ra).is_ok());
        assert_eq!(run(&ir_ra).0, Value::I32(135));
        let (ir2, _) = crate::opt::optimize(ir0.clone(), 2);
        assert_eq!(run(&ir0), run(&ir2));
        assert_eq!(run(&ir2).0, Value::I32(135));
    }

    #[test]
    fn call_redefining_its_own_argument_stays_live() {
        // bb0: %1 = 5           ; jmp bb1
        // bb1: %2 = 7; %3 = %2+%2 ; jmp bb2
        // bb2: %1 = id(%1); %4 = %1 + %3 ; ret %4
        // %1 must not share a register with %2: it is read in bb2 before the
        // call rewrites it, so it is live across bb1.
        let mut m = compile_ir("fn id(x: i32) -> i32 { return x; }\nfn main() -> i32 { return 0; }");
        {
            let main = m.functions.iter_mut().find(|f| f.name == "main").unwrap();
            main.blocks = vec![
                BasicBlock {
                    id: BlockId(0),
                    insts: vec![Inst::LoadConst {
                        dest: Reg(1),
                        value: ConstValue::I32(5),
                    }],
                    term: Terminator::Jump { target: BlockId(1) },
                },
                BasicBlock {
                    id: BlockId(1),
                    insts: vec![
                        Inst::LoadConst {
                            dest: Reg(2),
                            value: ConstValue::I32(7),
                        },
                        Inst::Bin {
                            dest: Reg(3),
                            op: BinOp::Add,
                            ty: Type::I32,
                            lhs: Reg(2),
                            rhs: Reg(2),
                        },
                    ],
                    term: Terminator::Jump { target: BlockId(2) },
                },
                BasicBlock {
                    id: BlockId(2),
                    insts: vec![
                        Inst::Call {
                            dest: Some(Reg(1)),
                            func: "id".to_string(),
                            args: vec![Reg(1)],
                        },
                        Inst::Bin {
                            dest: Reg(4),
                            op: BinOp::Add,
                            ty: Type::I32,
                            lhs: Reg(1),
                            rhs: Reg(3),
                        },
                    ],
                    term: Terminator::Return { value: Some(Reg(4)) },
                },
            ];
            main.reg_count = 5;
        }
        assert_eq!(run(&m).0, Value::I32(19));
        let mut ra = m.clone();
        pass_regalloc(&mut ra);
        assert!(verify_module(&ra).is_ok());
        let main = func(&ra, "main");
        assert!(main.reg_count <= 4, "reg_count = {}", main.reg_count);
        assert_eq!(run(&ra).0, Value::I32(19));
    }

    #[test]
    fn out_of_range_register_leaves_function_untouched() {
        let mut m = compile_ir("fn main() -> i32 { let a = 1; let b = a + 2; return b; }");
        let main = m.functions.iter_mut().find(|f| f.name == "main").unwrap();
        main.blocks[0].insts.push(Inst::LoadConst {
            dest: Reg(main.reg_count + 10),
            value: ConstValue::I32(0),
        });
        let before = m.clone();
        pass_regalloc(&mut m);
        assert_eq!(m, before);
    }
}
