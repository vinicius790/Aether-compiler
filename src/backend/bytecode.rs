//! Register bytecode for the Aether VM.
//!
//! Instruction format is a tagged enum stored in a `Vec<Op>`. A compact
//! binary encoding is provided for dump/load and for `disassemble`.

use crate::ast::{BinOp, UnOp};
use crate::ir::{ConstValue, Inst, IrFunction, IrModule, Terminator};
use crate::ty::Type;
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Immediate {
    I32(i32),
    I64(i64),
    F64(u64), // bits
    Bool(bool),
    Str(u32), // constant pool index
    Char(u32),
    Unit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    LoadImm { dest: u8, imm: Immediate },
    LoadStr { dest: u8, idx: u32 },
    Move { dest: u8, src: u8 },
    AddI32 { dest: u8, lhs: u8, rhs: u8 },
    SubI32 { dest: u8, lhs: u8, rhs: u8 },
    MulI32 { dest: u8, lhs: u8, rhs: u8 },
    DivI32 { dest: u8, lhs: u8, rhs: u8 },
    RemI32 { dest: u8, lhs: u8, rhs: u8 },
    NegI32 { dest: u8, src: u8 },
    AddI64 { dest: u8, lhs: u8, rhs: u8 },
    SubI64 { dest: u8, lhs: u8, rhs: u8 },
    MulI64 { dest: u8, lhs: u8, rhs: u8 },
    DivI64 { dest: u8, lhs: u8, rhs: u8 },
    AddF64 { dest: u8, lhs: u8, rhs: u8 },
    SubF64 { dest: u8, lhs: u8, rhs: u8 },
    MulF64 { dest: u8, lhs: u8, rhs: u8 },
    DivF64 { dest: u8, lhs: u8, rhs: u8 },
    NegF64 { dest: u8, src: u8 },
    CmpEqI32 { dest: u8, lhs: u8, rhs: u8 },
    CmpNeI32 { dest: u8, lhs: u8, rhs: u8 },
    CmpLtI32 { dest: u8, lhs: u8, rhs: u8 },
    CmpLeI32 { dest: u8, lhs: u8, rhs: u8 },
    CmpGtI32 { dest: u8, lhs: u8, rhs: u8 },
    CmpGeI32 { dest: u8, lhs: u8, rhs: u8 },
    CmpEqI64 { dest: u8, lhs: u8, rhs: u8 },
    CmpLtI64 { dest: u8, lhs: u8, rhs: u8 },
    CmpEqF64 { dest: u8, lhs: u8, rhs: u8 },
    CmpLtF64 { dest: u8, lhs: u8, rhs: u8 },
    CmpEqBool { dest: u8, lhs: u8, rhs: u8 },
    AndBool { dest: u8, lhs: u8, rhs: u8 },
    OrBool { dest: u8, lhs: u8, rhs: u8 },
    NotBool { dest: u8, src: u8 },
    Jump { target: u32 },
    JumpIf { cond: u8, target: u32 },
    JumpIfNot { cond: u8, target: u32 },
    Call { func: u32, dest: Option<u8>, args: Vec<u8> },
    CallNative { id: u16, dest: Option<u8>, args: Vec<u8> },
    Ret { src: u8 },
    RetVoid,
    CastI32ToI64 { dest: u8, src: u8 },
    CastI64ToI32 { dest: u8, src: u8 },
    CastI32ToF64 { dest: u8, src: u8 },
    CastF64ToI32 { dest: u8, src: u8 },
    CastBoolToI32 { dest: u8, src: u8 },
    AllocArr { dest: u8, len: u32 },
    LoadIdx { dest: u8, base: u8, index: u8 },
    StoreIdx { base: u8, index: u8, value: u8 },
    AllocObj { dest: u8, fields: u8 },
    LoadField { dest: u8, base: u8, field: u8 },
    StoreField { base: u8, field: u8, value: u8 },
    Concat { dest: u8, lhs: u8, rhs: u8 },
    Nop,
}

#[derive(Debug, Clone)]
pub struct BcFunction {
    pub name: String,
    pub arity: u8,
    pub nregs: u8,
    pub code: Vec<Op>,
    pub is_native: bool,
    pub native_id: Option<u16>,
}

#[derive(Debug, Clone)]
pub struct BytecodeModule {
    pub functions: Vec<BcFunction>,
    pub strings: Vec<String>,
    pub entry: u32, // index of main
}

impl BytecodeModule {
    pub fn function_index(&self, name: &str) -> Option<u32> {
        self.functions
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u32)
    }

    pub fn disassemble(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("; strings: {}\n", self.strings.len()));
        for (i, s) in self.strings.iter().enumerate() {
            out.push_str(&format!(";   [{i}] {s:?}\n"));
        }
        for (fi, f) in self.functions.iter().enumerate() {
            out.push_str(&format!(
                "\nfn {} #{} arity={} regs={}\n",
                f.name, fi, f.arity, f.nregs
            ));
            for (pc, op) in f.code.iter().enumerate() {
                out.push_str(&format!("  {pc:04}  {op}\n"));
            }
        }
        out
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Op::LoadImm { dest, imm } => write!(f, "loadimm r{dest}, {imm:?}"),
            Op::LoadStr { dest, idx } => write!(f, "loadstr r{dest}, str[{idx}]"),
            Op::Move { dest, src } => write!(f, "mov r{dest}, r{src}"),
            Op::AddI32 { dest, lhs, rhs } => write!(f, "addi32 r{dest}, r{lhs}, r{rhs}"),
            Op::SubI32 { dest, lhs, rhs } => write!(f, "subi32 r{dest}, r{lhs}, r{rhs}"),
            Op::MulI32 { dest, lhs, rhs } => write!(f, "muli32 r{dest}, r{lhs}, r{rhs}"),
            Op::DivI32 { dest, lhs, rhs } => write!(f, "divi32 r{dest}, r{lhs}, r{rhs}"),
            Op::RemI32 { dest, lhs, rhs } => write!(f, "remi32 r{dest}, r{lhs}, r{rhs}"),
            Op::NegI32 { dest, src } => write!(f, "negi32 r{dest}, r{src}"),
            Op::AddI64 { dest, lhs, rhs } => write!(f, "addi64 r{dest}, r{lhs}, r{rhs}"),
            Op::SubI64 { dest, lhs, rhs } => write!(f, "subi64 r{dest}, r{lhs}, r{rhs}"),
            Op::MulI64 { dest, lhs, rhs } => write!(f, "muli64 r{dest}, r{lhs}, r{rhs}"),
            Op::DivI64 { dest, lhs, rhs } => write!(f, "divi64 r{dest}, r{lhs}, r{rhs}"),
            Op::AddF64 { dest, lhs, rhs } => write!(f, "addf64 r{dest}, r{lhs}, r{rhs}"),
            Op::SubF64 { dest, lhs, rhs } => write!(f, "subf64 r{dest}, r{lhs}, r{rhs}"),
            Op::MulF64 { dest, lhs, rhs } => write!(f, "mulf64 r{dest}, r{lhs}, r{rhs}"),
            Op::DivF64 { dest, lhs, rhs } => write!(f, "divf64 r{dest}, r{lhs}, r{rhs}"),
            Op::NegF64 { dest, src } => write!(f, "negf64 r{dest}, r{src}"),
            Op::CmpEqI32 { dest, lhs, rhs } => write!(f, "eqi32 r{dest}, r{lhs}, r{rhs}"),
            Op::CmpNeI32 { dest, lhs, rhs } => write!(f, "nei32 r{dest}, r{lhs}, r{rhs}"),
            Op::CmpLtI32 { dest, lhs, rhs } => write!(f, "lti32 r{dest}, r{lhs}, r{rhs}"),
            Op::CmpLeI32 { dest, lhs, rhs } => write!(f, "lei32 r{dest}, r{lhs}, r{rhs}"),
            Op::CmpGtI32 { dest, lhs, rhs } => write!(f, "gti32 r{dest}, r{lhs}, r{rhs}"),
            Op::CmpGeI32 { dest, lhs, rhs } => write!(f, "gei32 r{dest}, r{lhs}, r{rhs}"),
            Op::CmpEqI64 { dest, lhs, rhs } => write!(f, "eqi64 r{dest}, r{lhs}, r{rhs}"),
            Op::CmpLtI64 { dest, lhs, rhs } => write!(f, "lti64 r{dest}, r{lhs}, r{rhs}"),
            Op::CmpEqF64 { dest, lhs, rhs } => write!(f, "eqf64 r{dest}, r{lhs}, r{rhs}"),
            Op::CmpLtF64 { dest, lhs, rhs } => write!(f, "ltf64 r{dest}, r{lhs}, r{rhs}"),
            Op::CmpEqBool { dest, lhs, rhs } => write!(f, "eqbool r{dest}, r{lhs}, r{rhs}"),
            Op::AndBool { dest, lhs, rhs } => write!(f, "and r{dest}, r{lhs}, r{rhs}"),
            Op::OrBool { dest, lhs, rhs } => write!(f, "or r{dest}, r{lhs}, r{rhs}"),
            Op::NotBool { dest, src } => write!(f, "not r{dest}, r{src}"),
            Op::Jump { target } => write!(f, "jmp {target}"),
            Op::JumpIf { cond, target } => write!(f, "jz r{cond}, {target}"),
            Op::JumpIfNot { cond, target } => write!(f, "jnz r{cond}, {target}"),
            Op::Call { func, dest, args } => write!(f, "call f{func} -> {dest:?} {args:?}"),
            Op::CallNative { id, dest, args } => write!(f, "native #{id} -> {dest:?} {args:?}"),
            Op::Ret { src } => write!(f, "ret r{src}"),
            Op::RetVoid => write!(f, "retvoid"),
            Op::CastI32ToI64 { dest, src } => write!(f, "i32toi64 r{dest}, r{src}"),
            Op::CastI64ToI32 { dest, src } => write!(f, "i64toi32 r{dest}, r{src}"),
            Op::CastI32ToF64 { dest, src } => write!(f, "i32tof64 r{dest}, r{src}"),
            Op::CastF64ToI32 { dest, src } => write!(f, "f64toi32 r{dest}, r{src}"),
            Op::CastBoolToI32 { dest, src } => write!(f, "booltoi32 r{dest}, r{src}"),
            Op::AllocArr { dest, len } => write!(f, "allocarr r{dest}, {len}"),
            Op::LoadIdx { dest, base, index } => write!(f, "loadidx r{dest}, r{base}[r{index}]"),
            Op::StoreIdx { base, index, value } => write!(f, "storeidx r{base}[r{index}], r{value}"),
            Op::AllocObj { dest, fields } => write!(f, "allocobj r{dest}, {fields}"),
            Op::LoadField { dest, base, field } => write!(f, "loadfld r{dest}, r{base}.{field}"),
            Op::StoreField { base, field, value } => {
                write!(f, "storefld r{base}.{field}, r{value}")
            }
            Op::Concat { dest, lhs, rhs } => write!(f, "concat r{dest}, r{lhs}, r{rhs}"),
            Op::Nop => write!(f, "nop"),
        }
    }
}

pub fn assemble(module: &IrModule) -> BytecodeModule {
    let mut strings: Vec<String> = Vec::new();
    let intern = |strings: &mut Vec<String>, s: &str| -> u32 {
        if let Some(i) = strings.iter().position(|x| x == s) {
            i as u32
        } else {
            strings.push(s.to_string());
            (strings.len() - 1) as u32
        }
    };

    // built-in natives first so user functions follow
    let natives: &[(&str, u16)] = &[
        ("print", 0),
        ("println", 1),
        ("print_i32", 2),
        ("print_i64", 3),
        ("print_f64", 4),
        ("print_bool", 5),
        ("len", 6),
        ("assert", 7),
    ];

    let mut functions: Vec<BcFunction> = Vec::new();
    for (name, id) in natives {
        functions.push(BcFunction {
            name: name.to_string(),
            arity: 1,
            nregs: 1,
            code: Vec::new(),
            is_native: true,
            native_id: Some(*id),
        });
    }

    let mut name_to_idx: HashMap<String, u32> = HashMap::new();
    for (i, f) in functions.iter().enumerate() {
        name_to_idx.insert(f.name.clone(), i as u32);
    }
    for f in &module.functions {
        if name_to_idx.contains_key(&f.name) {
            continue;
        }
        let idx = functions.len() as u32;
        name_to_idx.insert(f.name.clone(), idx);
        functions.push(BcFunction {
            name: f.name.clone(),
            arity: f.params.len() as u8,
            nregs: 0,
            code: Vec::new(),
            is_native: f.is_extern,
            native_id: None,
        });
    }

    for irf in &module.functions {
        if irf.is_extern {
            continue;
        }
        let idx = *name_to_idx.get(&irf.name).unwrap();
        let (code, nregs) = lower_function(irf, &name_to_idx, &mut strings, intern);
        functions[idx as usize].code = code;
        functions[idx as usize].nregs = nregs;
        functions[idx as usize].arity = irf.params.len() as u8;
    }

    let entry = *name_to_idx.get("main").unwrap_or(&0);
    BytecodeModule {
        functions,
        strings,
        entry,
    }
}

fn lower_function(
    f: &IrFunction,
    names: &HashMap<String, u32>,
    strings: &mut Vec<String>,
    intern: fn(&mut Vec<String>, &str) -> u32,
) -> (Vec<Op>, u8) {
    // Map block ids to instruction offsets after layout.
    let mut block_start: HashMap<u32, u32> = HashMap::new();
    let mut code: Vec<Op> = Vec::new();

    // First pass: emit with placeholder jump targets (block ids + FLAG)
    const BLOCK_FLAG: u32 = 1 << 31;

    for bb in &f.blocks {
        block_start.insert(bb.id.0, code.len() as u32);
        for inst in &bb.insts {
            emit_inst(inst, &mut code, names, strings, intern);
        }
        match &bb.term {
            Terminator::Jump { target } => {
                code.push(Op::Jump {
                    target: target.0 | BLOCK_FLAG,
                });
            }
            Terminator::Branch {
                cond,
                then_bb,
                else_bb,
            } => {
                code.push(Op::JumpIf {
                    cond: cond.0 as u8,
                    target: then_bb.0 | BLOCK_FLAG,
                });
                code.push(Op::Jump {
                    target: else_bb.0 | BLOCK_FLAG,
                });
            }
            Terminator::Return { value } => {
                if let Some(r) = value {
                    code.push(Op::Ret { src: r.0 as u8 });
                } else {
                    code.push(Op::RetVoid);
                }
            }
            Terminator::Unreachable => {
                code.push(Op::RetVoid);
            }
        }
    }

    // Patch jumps
    for op in &mut code {
        match op {
            Op::Jump { target } if *target & BLOCK_FLAG != 0 => {
                let bid = *target & !BLOCK_FLAG;
                *target = *block_start.get(&bid).unwrap_or(&0);
            }
            Op::JumpIf { target, .. } | Op::JumpIfNot { target, .. } if *target & BLOCK_FLAG != 0 => {
                let bid = *target & !BLOCK_FLAG;
                *target = *block_start.get(&bid).unwrap_or(&0);
            }
            _ => {}
        }
    }

    let nregs = f.reg_count.max(1).min(255) as u8;
    (code, nregs)
}

fn emit_inst(
    inst: &Inst,
    code: &mut Vec<Op>,
    names: &HashMap<String, u32>,
    strings: &mut Vec<String>,
    intern: fn(&mut Vec<String>, &str) -> u32,
) {
    match inst {
        Inst::LoadConst { dest, value } => match value {
            ConstValue::String(s) => {
                let idx = intern(strings, s);
                code.push(Op::LoadStr {
                    dest: dest.0 as u8,
                    idx,
                });
            }
            other => {
                code.push(Op::LoadImm {
                    dest: dest.0 as u8,
                    imm: const_to_imm(other, strings, intern),
                });
            }
        },
        Inst::Move { dest, src } => code.push(Op::Move {
            dest: dest.0 as u8,
            src: src.0 as u8,
        }),
        Inst::Bin {
            dest,
            op,
            ty,
            lhs,
            rhs,
        } => {
            let d = dest.0 as u8;
            let l = lhs.0 as u8;
            let r = rhs.0 as u8;
            let op = match (op, ty) {
                (BinOp::Add, Type::I32) => Op::AddI32 { dest: d, lhs: l, rhs: r },
                (BinOp::Sub, Type::I32) => Op::SubI32 { dest: d, lhs: l, rhs: r },
                (BinOp::Mul, Type::I32) => Op::MulI32 { dest: d, lhs: l, rhs: r },
                (BinOp::Div, Type::I32) => Op::DivI32 { dest: d, lhs: l, rhs: r },
                (BinOp::Rem, Type::I32) => Op::RemI32 { dest: d, lhs: l, rhs: r },
                (BinOp::Add, Type::I64) => Op::AddI64 { dest: d, lhs: l, rhs: r },
                (BinOp::Sub, Type::I64) => Op::SubI64 { dest: d, lhs: l, rhs: r },
                (BinOp::Mul, Type::I64) => Op::MulI64 { dest: d, lhs: l, rhs: r },
                (BinOp::Div, Type::I64) => Op::DivI64 { dest: d, lhs: l, rhs: r },
                (BinOp::Add, Type::F64) => Op::AddF64 { dest: d, lhs: l, rhs: r },
                (BinOp::Sub, Type::F64) => Op::SubF64 { dest: d, lhs: l, rhs: r },
                (BinOp::Mul, Type::F64) => Op::MulF64 { dest: d, lhs: l, rhs: r },
                (BinOp::Div, Type::F64) => Op::DivF64 { dest: d, lhs: l, rhs: r },
                (BinOp::Add, Type::String) => Op::Concat { dest: d, lhs: l, rhs: r },
                (BinOp::Eq, Type::I32) => Op::CmpEqI32 { dest: d, lhs: l, rhs: r },
                (BinOp::Ne, Type::I32) => Op::CmpNeI32 { dest: d, lhs: l, rhs: r },
                (BinOp::Lt, Type::I32) => Op::CmpLtI32 { dest: d, lhs: l, rhs: r },
                (BinOp::Le, Type::I32) => Op::CmpLeI32 { dest: d, lhs: l, rhs: r },
                (BinOp::Gt, Type::I32) => Op::CmpGtI32 { dest: d, lhs: l, rhs: r },
                (BinOp::Ge, Type::I32) => Op::CmpGeI32 { dest: d, lhs: l, rhs: r },
                (BinOp::Eq, Type::I64) => Op::CmpEqI64 { dest: d, lhs: l, rhs: r },
                (BinOp::Lt, Type::I64) => Op::CmpLtI64 { dest: d, lhs: l, rhs: r },
                (BinOp::Eq, Type::F64) => Op::CmpEqF64 { dest: d, lhs: l, rhs: r },
                (BinOp::Lt, Type::F64) => Op::CmpLtF64 { dest: d, lhs: l, rhs: r },
                (BinOp::Eq, Type::Bool) => Op::CmpEqBool { dest: d, lhs: l, rhs: r },
                (BinOp::And, Type::Bool) => Op::AndBool { dest: d, lhs: l, rhs: r },
                (BinOp::Or, Type::Bool) => Op::OrBool { dest: d, lhs: l, rhs: r },
                _ => Op::Nop,
            };
            code.push(op);
        }
        Inst::Un { dest, op, ty, src } => {
            let d = dest.0 as u8;
            let s = src.0 as u8;
            match (op, ty) {
                (UnOp::Neg, Type::I32) | (UnOp::Neg, Type::I64) => {
                    code.push(Op::NegI32 { dest: d, src: s })
                }
                (UnOp::Neg, Type::F64) => code.push(Op::NegF64 { dest: d, src: s }),
                (UnOp::Not, _) => code.push(Op::NotBool { dest: d, src: s }),
                _ => code.push(Op::Nop),
            }
        }
        Inst::Call { dest, func, args } => {
            let argv: Vec<u8> = args.iter().map(|r| r.0 as u8).collect();
            if let Some(&idx) = names.get(func) {
                let dest_opt = dest.map(|r| r.0 as u8);
                if idx < 8 {
                    code.push(Op::CallNative {
                        id: idx as u16,
                        dest: dest_opt,
                        args: argv,
                    });
                } else {
                    code.push(Op::Call {
                        func: idx,
                        dest: dest_opt,
                        args: argv,
                    });
                }
            } else {
                code.push(Op::Nop);
            }
        }
        Inst::Cast { dest, src, from, to } => {
            let d = dest.0 as u8;
            let s = src.0 as u8;
            let op = match (from, to) {
                (Type::I32, Type::I64) => Op::CastI32ToI64 { dest: d, src: s },
                (Type::I64, Type::I32) => Op::CastI64ToI32 { dest: d, src: s },
                (Type::I32, Type::F64) => Op::CastI32ToF64 { dest: d, src: s },
                (Type::F64, Type::I32) => Op::CastF64ToI32 { dest: d, src: s },
                (Type::Bool, Type::I32) => Op::CastBoolToI32 { dest: d, src: s },
                _ => Op::Move { dest: d, src: s },
            };
            code.push(op);
        }
        Inst::IndexLoad {
            dest, base, index, ..
        } => code.push(Op::LoadIdx {
            dest: dest.0 as u8,
            base: base.0 as u8,
            index: index.0 as u8,
        }),
        Inst::IndexStore {
            base, index, value, ..
        } => code.push(Op::StoreIdx {
            base: base.0 as u8,
            index: index.0 as u8,
            value: value.0 as u8,
        }),
        Inst::FieldLoad {
            dest, base, index, ..
        } => code.push(Op::LoadField {
            dest: dest.0 as u8,
            base: base.0 as u8,
            field: *index as u8,
        }),
        Inst::FieldStore {
            base, index, value, ..
        } => code.push(Op::StoreField {
            base: base.0 as u8,
            field: *index as u8,
            value: value.0 as u8,
        }),
        Inst::AllocArray { dest, len, .. } => code.push(Op::AllocArr {
            dest: dest.0 as u8,
            len: *len as u32,
        }),
        Inst::AllocStruct { dest, ty } => {
            let n = match ty {
                Type::Struct { fields, .. } => fields.len() as u8,
                _ => 0,
            };
            code.push(Op::AllocObj {
                dest: dest.0 as u8,
                fields: n,
            });
        }
        Inst::Nop => code.push(Op::Nop),
    }
}

fn const_to_imm(
    v: &ConstValue,
    strings: &mut Vec<String>,
    intern: fn(&mut Vec<String>, &str) -> u32,
) -> Immediate {
    match v {
        ConstValue::I32(x) => Immediate::I32(*x),
        ConstValue::I64(x) => Immediate::I64(*x),
        ConstValue::F64(x) => Immediate::F64(x.to_bits()),
        ConstValue::Bool(x) => Immediate::Bool(*x),
        ConstValue::String(s) => Immediate::Str(intern(strings, s)),
        ConstValue::Char(c) => Immediate::Char(*c as u32),
        ConstValue::Unit => Immediate::Unit,
    }
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
    fn assembles_main() {
        let src = "fn main() -> i32 { return 1 + 2; }";
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, _) = analyze(&prog);
        let ir = emit_ir(&hir.unwrap());
        let bc = assemble(&ir);
        assert!(bc.function_index("main").is_some());
        let text = bc.disassemble();
        assert!(text.contains("addi32") || text.contains("loadimm"));
    }
}
