//! Direct generation of Aether IR — the local analogue of Rustlantis.
//!
//! Rustlantis does not emit Rust source: it builds a MIR CFG so it never
//! has to please the borrow checker. Here the bottleneck is smaller, but
//! the same idea applies: skip lexer/parser/sema and feed the optimizer
//! and the VM a well-formed `IrModule`.
//!
//! Generation invariants
//! ---------------------
//! * every function has an entry block and every block a terminator;
//! * every register use is `< reg_count`;
//! * arithmetic is `i32`, compares produce `bool`;
//! * `for`-style loops are a counted CFG with a compile-time bound ≤ 4;
//! * *decoy blocks* hang off a `branch` on a constant `true`, so `-O0`
//!   never takes them and `-O2` should fold them. They exist to tempt
//!   the optimizer with extra CFG shape.
//!
//! What this is not
//! ----------------
//! Not LLVM MIR, not rustc MIR. It is Aether's own three-address IR.
//! A later greybox target (`#[derive(Arbitrary)]` on `IrModule`) would
//! sit on the same types.

use super::rng::FuzzRng;
use crate::ast::BinOp;
use crate::backend::assemble;
use crate::ir::{BasicBlock, BlockId, ConstValue, Inst, IrFunction, IrModule, Reg, Terminator};
use crate::opt::optimize;
use crate::span::Span;
use crate::ty::Type;
use crate::vm::{execute_captured, Value};

#[derive(Clone, Copy)]
enum RTy {
    I32,
    Bool,
}

pub fn gen_ir(rng: &mut FuzzRng) -> IrModule {
    let n_helpers = rng.int(0, 1) as usize;
    let mut functions = Vec::new();
    for i in 0..n_helpers {
        functions.push(gen_helper(rng, i));
    }
    functions.push(gen_main(rng, n_helpers));
    IrModule {
        functions,
        structs: Vec::new(),
    }
}

struct FunBuilder {
    name: String,
    params: Vec<(String, Type, Reg)>,
    blocks: Vec<BasicBlock>,
    next_reg: u32,
    tys: Vec<RTy>,
}

impl FunBuilder {
    fn new(name: &str) -> Self {
        FunBuilder {
            name: name.into(),
            params: Vec::new(),
            blocks: vec![BasicBlock {
                id: BlockId(0),
                insts: Vec::new(),
                term: Terminator::Unreachable,
            }],
            next_reg: 0,
            tys: Vec::new(),
        }
    }

    fn alloc(&mut self, ty: RTy) -> Reg {
        let r = Reg(self.next_reg);
        self.next_reg += 1;
        self.tys.push(ty);
        r
    }

    fn add_param(&mut self, name: &str) -> Reg {
        let r = self.alloc(RTy::I32);
        self.params.push((name.into(), Type::I32, r));
        r
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len() as u32);
        self.blocks.push(BasicBlock {
            id,
            insts: Vec::new(),
            term: Terminator::Unreachable,
        });
        id
    }

    fn emit(&mut self, bb: BlockId, inst: Inst) {
        self.blocks[bb.0 as usize].insts.push(inst);
    }

    fn term(&mut self, bb: BlockId, t: Terminator) {
        self.blocks[bb.0 as usize].term = t;
    }

    fn const_i32(&mut self, bb: BlockId, v: i32) -> Reg {
        let d = self.alloc(RTy::I32);
        self.emit(
            bb,
            Inst::LoadConst {
                dest: d,
                value: ConstValue::I32(v),
            },
        );
        d
    }

    fn const_bool(&mut self, bb: BlockId, v: bool) -> Reg {
        let d = self.alloc(RTy::Bool);
        self.emit(
            bb,
            Inst::LoadConst {
                dest: d,
                value: ConstValue::Bool(v),
            },
        );
        d
    }

    fn bin(&mut self, bb: BlockId, op: BinOp, ty: Type, out: RTy, lhs: Reg, rhs: Reg) -> Reg {
        let d = self.alloc(out);
        self.emit(
            bb,
            Inst::Bin {
                dest: d,
                op,
                ty,
                lhs,
                rhs,
            },
        );
        d
    }

    fn finish(self) -> IrFunction {
        IrFunction {
            name: self.name,
            params: self.params,
            return_ty: Type::I32,
            blocks: self.blocks,
            reg_count: self.next_reg,
            is_extern: false,
            span: Span::DUMMY,
        }
    }
}

fn gen_helper(rng: &mut FuzzRng, idx: usize) -> IrFunction {
    let mut b = FunBuilder::new(&format!("h{idx}"));
    let p0 = b.add_param("p0");
    let entry = BlockId(0);
    let k = b.const_i32(entry, rng.int(1, 5));
    let op = if rng.bool() { BinOp::Add } else { BinOp::Mul };
    let r = b.bin(entry, op, Type::I32, RTy::I32, p0, k);
    b.term(entry, Terminator::Return { value: Some(r) });
    b.finish()
}

fn gen_main(rng: &mut FuzzRng, helpers: usize) -> IrFunction {
    let mut b = FunBuilder::new("main");
    let mut bb = BlockId(0);
    let mut acc = b.const_i32(bb, 0);

    let n_chunks = rng.int(1, 3) as usize;
    for _ in 0..n_chunks {
        match rng.int(0, 4) {
            0 => {
                // acc = acc ⊕ k
                let k = b.const_i32(bb, rng.int(0, 7));
                let op = *rng.choose(&[BinOp::Add, BinOp::Sub, BinOp::Mul]);
                acc = b.bin(bb, op, Type::I32, RTy::I32, acc, k);
            }
            1 => {
                bb = insert_decoy(&mut b, rng, bb, acc);
                // Live-path increment, *after* the join, so both edges
                // leave `acc` defined (non-SSA).
                let k = b.const_i32(bb, 1);
                acc = b.bin(bb, BinOp::Add, Type::I32, RTy::I32, acc, k);
            }
            2 => {
                // Counted loop with a compile-time bound. The accumulator
                // register is updated *in place* inside the body so both
                // the taken and the not-taken edge leave `acc` defined
                // (non-SSA: a new dest in the body would be undefined
                // when n == 0 and -O2 deletes that block).
                let n = rng.int(0, 3);
                let header = b.new_block();
                let body = b.new_block();
                let after = b.new_block();
                b.term(bb, Terminator::Jump { target: header });
                let i = b.const_i32(header, 0);
                let lim = b.const_i32(header, n);
                let cmp = b.bin(header, BinOp::Lt, Type::I32, RTy::Bool, i, lim);
                b.term(
                    header,
                    Terminator::Branch {
                        cond: cmp,
                        then_bb: body,
                        else_bb: after,
                    },
                );
                let one = b.const_i32(body, 1);
                b.emit(
                    body,
                    Inst::Bin {
                        dest: acc,
                        op: BinOp::Add,
                        ty: Type::I32,
                        lhs: acc,
                        rhs: one,
                    },
                );
                b.term(body, Terminator::Jump { target: after });
                bb = after;
            }
            3 if helpers > 0 => {
                let arg = b.const_i32(bb, rng.int(0, 5));
                let dest = b.alloc(RTy::I32);
                let idx = rng.int(0, helpers.saturating_sub(1) as i32);
                b.emit(
                    bb,
                    Inst::Call {
                        dest: Some(dest),
                        func: format!("h{idx}"),
                        args: vec![arg],
                    },
                );
                acc = b.bin(bb, BinOp::Add, Type::I32, RTy::I32, acc, dest);
            }
            _ => {
                let v = b.const_i32(bb, rng.int(0, 9));
                b.emit(
                    bb,
                    Inst::Call {
                        dest: None,
                        func: "print_i32".into(),
                        args: vec![v],
                    },
                );
            }
        }
    }

    b.term(bb, Terminator::Return { value: Some(acc) });
    b.finish()
}

/// Decoy catalogue (Rustlantis-inspired).
///
/// Every variant joins at `real`. The dead side never writes `acc`.
/// Predicates are either a literal bool (trivial for const-fold) or an
/// *opaque* identity (`acc-acc==0`, `acc*0==0`) that a weak fold may miss.
fn insert_decoy(b: &mut FunBuilder, rng: &mut FuzzRng, bb: BlockId, acc: Reg) -> BlockId {
    let real = b.new_block();
    match rng.int(0, 4) {
        0 => {
            let cond = b.const_bool(bb, true);
            let decoy = b.new_block();
            b.term(
                bb,
                Terminator::Branch {
                    cond,
                    then_bb: real,
                    else_bb: decoy,
                },
            );
            fill_decoy(b, rng, decoy, real);
        }
        1 => {
            let cond = b.const_bool(bb, false);
            let decoy = b.new_block();
            b.term(
                bb,
                Terminator::Branch {
                    cond,
                    then_bb: decoy,
                    else_bb: real,
                },
            );
            fill_decoy(b, rng, decoy, real);
        }
        2 => {
            // (acc - acc) == 0  — always true, not a literal.
            let diff = b.bin(bb, BinOp::Sub, Type::I32, RTy::I32, acc, acc);
            let z = b.const_i32(bb, 0);
            let cond = b.bin(bb, BinOp::Eq, Type::I32, RTy::Bool, diff, z);
            let decoy = b.new_block();
            b.term(
                bb,
                Terminator::Branch {
                    cond,
                    then_bb: real,
                    else_bb: decoy,
                },
            );
            fill_decoy(b, rng, decoy, real);
        }
        3 => {
            // (acc * 0) == 0
            let z = b.const_i32(bb, 0);
            let prod = b.bin(bb, BinOp::Mul, Type::I32, RTy::I32, acc, z);
            let cond = b.bin(bb, BinOp::Eq, Type::I32, RTy::Bool, prod, z);
            let decoy = b.new_block();
            b.term(
                bb,
                Terminator::Branch {
                    cond,
                    then_bb: real,
                    else_bb: decoy,
                },
            );
            fill_decoy(b, rng, decoy, real);
        }
        _ => {
            let cond = b.const_bool(bb, true);
            let d1 = b.new_block();
            let d2 = b.new_block();
            b.term(
                bb,
                Terminator::Branch {
                    cond,
                    then_bb: real,
                    else_bb: d1,
                },
            );
            fill_decoy(b, rng, d1, d2);
            fill_decoy(b, rng, d2, real);
        }
    }
    real
}

fn fill_decoy(b: &mut FunBuilder, rng: &mut FuzzRng, decoy: BlockId, join: BlockId) {
    let junk = b.const_i32(decoy, rng.int(50, 99));
    let _ = b.bin(decoy, BinOp::Add, Type::I32, RTy::I32, junk, junk);
    b.term(decoy, Terminator::Jump { target: join });
}

/// How many branches look like decoys (const bool or compare-to-zero).
pub fn decoy_stats(m: &IrModule) -> (usize, usize) {
    let mut branches = 0;
    let mut decoyish = 0;
    for f in &m.functions {
        for bb in &f.blocks {
            if let Terminator::Branch { .. } = bb.term {
                branches += 1;
                let looks = bb.insts.iter().any(|i| match i {
                    Inst::LoadConst {
                        value: ConstValue::Bool(_),
                        ..
                    } => true,
                    Inst::Bin {
                        op: BinOp::Eq | BinOp::Ne,
                        ..
                    } => true,
                    _ => false,
                });
                if looks {
                    decoyish += 1;
                }
            }
        }
    }
    (branches, decoyish)
}

/// Structural well-formedness of a generated (or optimized) module.
pub fn check_ir(m: &IrModule) -> Result<(), String> {
    crate::ir::verify::verify_module(m)
}

pub fn eval_ir(module: IrModule, opt_level: u8) -> Result<(Value, String), String> {
    let (ir, _) = optimize(module, opt_level);
    check_ir(&ir).map_err(|e| format!("opt O{opt_level} broke IR: {e}"))?;
    let bc = assemble(&ir);
    execute_captured(&bc).map(|(v, out, _)| (v, out)).map_err(|e| e.to_string())
}

/// Aspect-preserving IR mutation: rewrite an instruction without
/// invalidating register types or CFG edges.
pub fn mutate_ir(rng: &mut FuzzRng, module: &IrModule) -> IrModule {
    let mut out = module.clone();
    // Only touch `main` so helper signatures stay a stable aspect.
    let Some(f) = out.functions.iter_mut().find(|f| f.name == "main") else {
        return out;
    };
    // 1. Tweak an i32 immediate.
    for bb in &mut f.blocks {
        for inst in &mut bb.insts {
            if let Inst::LoadConst {
                value: ConstValue::I32(n),
                ..
            } = inst
            {
                if rng.int(0, 3) == 0 {
                    *n = rng.int(0, 7);
                }
            }
        }
    }
    // 2. Swap + / - / * on i32 bins (type aspect stays i32→i32).
    for bb in &mut f.blocks {
        for inst in &mut bb.insts {
            if let Inst::Bin {
                op,
                ty: Type::I32,
                ..
            } = inst
            {
                if matches!(*op, BinOp::Add | BinOp::Sub | BinOp::Mul) && rng.int(0, 2) == 0 {
                    *op = *rng.choose(&[BinOp::Add, BinOp::Sub, BinOp::Mul]);
                }
            }
        }
    }
    // 3. Insert `x + 0` after a LoadConst i32 (value aspect, extra work for DCE).
    if rng.bool() {
        if let Some(bb) = f.blocks.first_mut() {
            if let Some(Inst::LoadConst {
                dest,
                value: ConstValue::I32(_),
            }) = bb.insts.first().cloned()
            {
                let zero = Reg(f.reg_count);
                f.reg_count += 1;
                let sum = Reg(f.reg_count);
                f.reg_count += 1;
                bb.insts.insert(
                    1,
                    Inst::LoadConst {
                        dest: zero,
                        value: ConstValue::I32(0),
                    },
                );
                bb.insts.insert(
                    2,
                    Inst::Bin {
                        dest: sum,
                        op: BinOp::Add,
                        ty: Type::I32,
                        lhs: dest,
                        rhs: zero,
                    },
                );
                // leave `sum` unused — DCE food, value of dest unchanged
                let _ = sum;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzz::rng::FuzzRng;

    #[test]
    fn generated_ir_is_well_formed() {
        let mut rng = FuzzRng::new(11);
        for _ in 0..20 {
            let m = gen_ir(&mut rng);
            check_ir(&m).expect("generated IR");
            check_ir(&optimize(m, 2).0).expect("optimized IR");
        }
    }

    #[test]
    fn generated_ir_runs() {
        let mut rng = FuzzRng::new(13);
        let m = gen_ir(&mut rng);
        let _ = eval_ir(m, 0).expect("run O0");
    }

    #[test]
    fn some_modules_contain_decoy_branches() {
        let mut rng = FuzzRng::new(101);
        let mut seen = 0;
        for _ in 0..40 {
            let m = gen_ir(&mut rng);
            let (br, decoy) = decoy_stats(&m);
            seen += decoy;
            let _ = br;
            check_ir(&m).unwrap();
        }
        assert!(seen > 0, "expected at least one decoy-like branch");
    }
}
