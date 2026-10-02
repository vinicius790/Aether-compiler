//! Register bytecode for the Aether VM.
//!
//! Instruction format is a tagged enum stored in a `Vec<Op>`. A compact
//! binary encoding is provided for dump/load and for `disassemble`.
//!
//! Registers are `u16`: a function may use up to 65535 registers. The
//! assembler reports an error (never truncates) when the IR needs more.

use crate::ast::{BinOp, UnOp};
use crate::ir::{ConstValue, Inst, IrFunction, IrModule, Terminator};
use crate::ty::Type;
use std::collections::{HashMap, HashSet};
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

/// Comparison kind for the generic [`Op::Cmp`] instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    pub fn as_str(self) -> &'static str {
        match self {
            CmpOp::Eq => "eq",
            CmpOp::Ne => "ne",
            CmpOp::Lt => "lt",
            CmpOp::Le => "le",
            CmpOp::Gt => "gt",
            CmpOp::Ge => "ge",
        }
    }

    fn from_bin(op: BinOp) -> Option<CmpOp> {
        Some(match op {
            BinOp::Eq => CmpOp::Eq,
            BinOp::Ne => CmpOp::Ne,
            BinOp::Lt => CmpOp::Lt,
            BinOp::Le => CmpOp::Le,
            BinOp::Gt => CmpOp::Gt,
            BinOp::Ge => CmpOp::Ge,
            _ => return None,
        })
    }
}

impl fmt::Display for CmpOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    LoadImm { dest: u16, imm: Immediate },
    LoadStr { dest: u16, idx: u32 },
    Move { dest: u16, src: u16 },
    AddI32 { dest: u16, lhs: u16, rhs: u16 },
    SubI32 { dest: u16, lhs: u16, rhs: u16 },
    MulI32 { dest: u16, lhs: u16, rhs: u16 },
    DivI32 { dest: u16, lhs: u16, rhs: u16 },
    RemI32 { dest: u16, lhs: u16, rhs: u16 },
    NegI32 { dest: u16, src: u16 },
    AddI64 { dest: u16, lhs: u16, rhs: u16 },
    SubI64 { dest: u16, lhs: u16, rhs: u16 },
    MulI64 { dest: u16, lhs: u16, rhs: u16 },
    DivI64 { dest: u16, lhs: u16, rhs: u16 },
    RemI64 { dest: u16, lhs: u16, rhs: u16 },
    NegI64 { dest: u16, src: u16 },
    AddF64 { dest: u16, lhs: u16, rhs: u16 },
    SubF64 { dest: u16, lhs: u16, rhs: u16 },
    MulF64 { dest: u16, lhs: u16, rhs: u16 },
    DivF64 { dest: u16, lhs: u16, rhs: u16 },
    NegF64 { dest: u16, src: u16 },
    CmpEqI32 { dest: u16, lhs: u16, rhs: u16 },
    CmpNeI32 { dest: u16, lhs: u16, rhs: u16 },
    CmpLtI32 { dest: u16, lhs: u16, rhs: u16 },
    CmpLeI32 { dest: u16, lhs: u16, rhs: u16 },
    CmpGtI32 { dest: u16, lhs: u16, rhs: u16 },
    CmpGeI32 { dest: u16, lhs: u16, rhs: u16 },
    CmpEqI64 { dest: u16, lhs: u16, rhs: u16 },
    CmpLtI64 { dest: u16, lhs: u16, rhs: u16 },
    CmpEqF64 { dest: u16, lhs: u16, rhs: u16 },
    CmpLtF64 { dest: u16, lhs: u16, rhs: u16 },
    CmpEqBool { dest: u16, lhs: u16, rhs: u16 },
    /// Generic comparison of two same-typed values (i64/f64 Ne/Le/Gt/Ge,
    /// bool Ne, char and string). The VM dispatches on the runtime tag.
    Cmp { op: CmpOp, dest: u16, lhs: u16, rhs: u16 },
    AndBool { dest: u16, lhs: u16, rhs: u16 },
    OrBool { dest: u16, lhs: u16, rhs: u16 },
    NotBool { dest: u16, src: u16 },
    Jump { target: u32 },
    JumpIf { cond: u16, target: u32 },
    JumpIfNot { cond: u16, target: u32 },
    Call { func: u32, dest: Option<u16>, args: Vec<u16> },
    CallNative { id: u16, dest: Option<u16>, args: Vec<u16> },
    Ret { src: u16 },
    RetVoid,
    CastI32ToI64 { dest: u16, src: u16 },
    CastI64ToI32 { dest: u16, src: u16 },
    CastI32ToF64 { dest: u16, src: u16 },
    CastI64ToF64 { dest: u16, src: u16 },
    CastF64ToI32 { dest: u16, src: u16 },
    CastF64ToI64 { dest: u16, src: u16 },
    CastBoolToI32 { dest: u16, src: u16 },
    CastBoolToI64 { dest: u16, src: u16 },
    CastCharToI32 { dest: u16, src: u16 },
    CastI32ToChar { dest: u16, src: u16 },
    AllocArr { dest: u16, len: u32 },
    LoadIdx { dest: u16, base: u16, index: u16 },
    StoreIdx { base: u16, index: u16, value: u16 },
    AllocObj { dest: u16, fields: u16 },
    LoadField { dest: u16, base: u16, field: u16 },
    StoreField { base: u16, field: u16, value: u16 },
    Concat { dest: u16, lhs: u16, rhs: u16 },
    /// Cooperative scheduling point: `Vm::run_budget` returns `Step::Yielded`
    /// right after this instruction; `Vm::run` treats it as a no-op.
    Yield,
    /// Integer bit operations; the operand kind (i32/i64) is the register's.
    BitAnd { dest: u16, lhs: u16, rhs: u16 },
    BitOr { dest: u16, lhs: u16, rhs: u16 },
    BitXor { dest: u16, lhs: u16, rhs: u16 },
    /// Shift amount masked to the width (`& 31` / `& 63`); `Shr` is arithmetic.
    Shl { dest: u16, lhs: u16, rhs: u16 },
    Shr { dest: u16, lhs: u16, rhs: u16 },
    NotInt { dest: u16, src: u16 },
    Nop,
}

#[derive(Debug, Clone)]
pub struct BcFunction {
    pub name: String,
    pub arity: u16,
    pub nregs: u16,
    pub code: Vec<Op>,
    pub is_native: bool,
    pub native_id: Option<u16>,
    /// Declared return type of an `extern fn` bound to the host (`None` for
    /// everything else). The VM checks the value a host closure returns
    /// against it, so a mistyped binding is a runtime error, not garbage.
    pub ret_ty: Option<Type>,
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
            Op::RemI64 { dest, lhs, rhs } => write!(f, "remi64 r{dest}, r{lhs}, r{rhs}"),
            Op::NegI64 { dest, src } => write!(f, "negi64 r{dest}, r{src}"),
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
            Op::Cmp { op, dest, lhs, rhs } => write!(f, "cmp.{op} r{dest}, r{lhs}, r{rhs}"),
            Op::AndBool { dest, lhs, rhs } => write!(f, "and r{dest}, r{lhs}, r{rhs}"),
            Op::OrBool { dest, lhs, rhs } => write!(f, "or r{dest}, r{lhs}, r{rhs}"),
            Op::NotBool { dest, src } => write!(f, "not r{dest}, r{src}"),
            Op::Jump { target } => write!(f, "jmp {target}"),
            Op::JumpIf { cond, target } => write!(f, "jnz r{cond}, {target}"),
            Op::JumpIfNot { cond, target } => write!(f, "jz r{cond}, {target}"),
            Op::Call { func, dest, args } => write!(f, "call f{func} -> {dest:?} {args:?}"),
            Op::CallNative { id, dest, args } => write!(f, "native #{id} -> {dest:?} {args:?}"),
            Op::Ret { src } => write!(f, "ret r{src}"),
            Op::RetVoid => write!(f, "retvoid"),
            Op::CastI32ToI64 { dest, src } => write!(f, "i32toi64 r{dest}, r{src}"),
            Op::CastI64ToI32 { dest, src } => write!(f, "i64toi32 r{dest}, r{src}"),
            Op::CastI32ToF64 { dest, src } => write!(f, "i32tof64 r{dest}, r{src}"),
            Op::CastI64ToF64 { dest, src } => write!(f, "i64tof64 r{dest}, r{src}"),
            Op::CastF64ToI32 { dest, src } => write!(f, "f64toi32 r{dest}, r{src}"),
            Op::CastF64ToI64 { dest, src } => write!(f, "f64toi64 r{dest}, r{src}"),
            Op::CastBoolToI32 { dest, src } => write!(f, "booltoi32 r{dest}, r{src}"),
            Op::CastBoolToI64 { dest, src } => write!(f, "booltoi64 r{dest}, r{src}"),
            Op::CastCharToI32 { dest, src } => write!(f, "chartoi32 r{dest}, r{src}"),
            Op::CastI32ToChar { dest, src } => write!(f, "i32tochar r{dest}, r{src}"),
            Op::AllocArr { dest, len } => write!(f, "allocarr r{dest}, {len}"),
            Op::LoadIdx { dest, base, index } => write!(f, "loadidx r{dest}, r{base}[r{index}]"),
            Op::StoreIdx { base, index, value } => write!(f, "storeidx r{base}[r{index}], r{value}"),
            Op::AllocObj { dest, fields } => write!(f, "allocobj r{dest}, {fields}"),
            Op::LoadField { dest, base, field } => write!(f, "loadfld r{dest}, r{base}.{field}"),
            Op::StoreField { base, field, value } => {
                write!(f, "storefld r{base}.{field}, r{value}")
            }
            Op::Concat { dest, lhs, rhs } => write!(f, "concat r{dest}, r{lhs}, r{rhs}"),
            Op::Yield => write!(f, "yield"),
            Op::BitAnd { dest, lhs, rhs } => write!(f, "band r{dest}, r{lhs}, r{rhs}"),
            Op::BitOr { dest, lhs, rhs } => write!(f, "bor r{dest}, r{lhs}, r{rhs}"),
            Op::BitXor { dest, lhs, rhs } => write!(f, "bxor r{dest}, r{lhs}, r{rhs}"),
            Op::Shl { dest, lhs, rhs } => write!(f, "shl r{dest}, r{lhs}, r{rhs}"),
            Op::Shr { dest, lhs, rhs } => write!(f, "shr r{dest}, r{lhs}, r{rhs}"),
            Op::NotInt { dest, src } => write!(f, "bnot r{dest}, r{src}"),
            Op::Nop => write!(f, "nop"),
        }
    }
}

/// Largest register index the VM can address.
pub const MAX_REGS: u32 = u16::MAX as u32;

/// Largest number of fields an object (struct, tuple or enum slot list) may
/// have: `AllocObj`/`LoadField`/`StoreField` carry a `u16`.
pub const MAX_FIELDS: usize = u16::MAX as usize;

/// Largest array the VM allocates (`AllocArr`). A literal beyond this is a
/// compile error (E0300) and the VM refuses the same size at runtime instead
/// of aborting the process on allocation failure.
pub const MAX_ARRAY_LEN: usize = 1 << 28;

/// Constant pool with O(1) interning.
#[derive(Default)]
struct Pool {
    strings: Vec<String>,
    index: HashMap<String, u32>,
}

impl Pool {
    fn intern(&mut self, s: &str) -> Result<u32, String> {
        if let Some(&i) = self.index.get(s) {
            return Ok(i);
        }
        let i = u32::try_from(self.strings.len()).map_err(|_| "too many string constants".to_string())?;
        self.strings.push(s.to_string());
        self.index.insert(s.to_string(), i);
        Ok(i)
    }
}

/// What the assembler knows about the module while lowering one function.
struct Ctx<'a> {
    names: &'a HashMap<String, u32>,
    /// Parameter count of every IR function, to check call arity.
    arities: &'a HashMap<&'a str, usize>,
}

pub fn assemble(module: &IrModule) -> Result<BytecodeModule, String> {
    let mut pool = Pool::default();

    // built-in natives first so user functions follow; the id is the index
    // in `runtime::NATIVES`
    let natives: Vec<(&str, u16)> = crate::runtime::NATIVES
        .iter()
        .enumerate()
        .map(|(i, n)| (*n, i as u16))
        .collect();

    let mut functions: Vec<BcFunction> = Vec::new();
    for (name, id) in &natives {
        functions.push(BcFunction {
            name: name.to_string(),
            arity: 1,
            nregs: 1,
            code: Vec::new(),
            is_native: true,
            native_id: Some(*id),
            ret_ty: None,
        });
    }

    let mut name_to_idx: HashMap<String, u32> = HashMap::new();
    for (i, f) in functions.iter().enumerate() {
        name_to_idx.insert(f.name.clone(), i as u32);
    }
    // A user function with a built-in's name shadows the native (sema agrees).
    let mut arities: HashMap<&str, usize> = HashMap::new();
    for f in &module.functions {
        if arities.insert(f.name.as_str(), f.params.len()).is_some() {
            return Err(format!("duplicate function `{}`", f.name));
        }
        let idx = functions.len() as u32;
        name_to_idx.insert(f.name.clone(), idx);
        functions.push(BcFunction {
            name: f.name.clone(),
            arity: arity_of(f)?,
            nregs: 0,
            code: Vec::new(),
            is_native: f.is_extern,
            native_id: None,
            ret_ty: f.is_extern.then(|| f.return_ty.clone()),
        });
    }

    let ctx = Ctx {
        names: &name_to_idx,
        arities: &arities,
    };
    for irf in &module.functions {
        if irf.is_extern {
            continue;
        }
        let idx = *name_to_idx.get(&irf.name).unwrap();
        let (code, nregs) = lower_function(irf, &ctx, &mut pool)?;
        functions[idx as usize].code = code;
        functions[idx as usize].nregs = nregs;
        functions[idx as usize].arity = arity_of(irf)?;
    }

    let entry = *name_to_idx.get("main").unwrap_or(&0);
    Ok(BytecodeModule {
        functions,
        strings: pool.strings,
        entry,
    })
}

/// Parameter count as a `u16`; parameters occupy the first registers, so
/// a function with too many of them cannot fit either.
fn arity_of(f: &IrFunction) -> Result<u16, String> {
    let n = f.params.len();
    if n > MAX_REGS as usize {
        return Err(too_many_regs(&f.name, n as u64));
    }
    Ok(n as u16)
}

fn too_many_regs(name: &str, n: u64) -> String {
    format!("function `{name}` needs {n} registers; the VM supports at most {MAX_REGS}")
}

/// Narrow an IR register to a VM register. `check_function` has already
/// verified every register against `reg_count`, so this only fires on a
/// declared count that exceeds what a `u16` can address.
fn reg(r: crate::ir::Reg, fname: &str) -> Result<u16, String> {
    u16::try_from(r.0).map_err(|_| too_many_regs(fname, r.0 as u64 + 1))
}

/// Rejects, before any code is emitted, everything that would make the VM
/// read or write outside a frame, jump outside a function or call with the
/// wrong number of arguments. Well-formed IR from the compiler never trips
/// these; hand-built or fuzzed IR gets a diagnostic instead of silent `Unit`s.
fn check_function(f: &IrFunction, ctx: &Ctx<'_>) -> Result<(), String> {
    if f.reg_count > MAX_REGS {
        return Err(too_many_regs(&f.name, f.reg_count as u64));
    }
    if f.blocks.is_empty() {
        return Err(format!("function `{}` has no blocks", f.name));
    }
    let name = &f.name;
    let in_range = |r: crate::ir::Reg, what: &str| -> Result<(), String> {
        if r.0 >= f.reg_count {
            Err(format!(
                "function `{name}`: {what} register %{} is outside the {} declared registers",
                r.0, f.reg_count
            ))
        } else {
            Ok(())
        }
    };
    for (i, (_, _, r)) in f.params.iter().enumerate() {
        if r.0 as usize != i {
            return Err(format!(
                "function `{name}`: parameter {i} lives in %{}, parameters must occupy the first registers",
                r.0
            ));
        }
        in_range(*r, "parameter")?;
    }
    let mut ids: HashSet<u32> = HashSet::new();
    for bb in &f.blocks {
        if !ids.insert(bb.id.0) {
            return Err(format!("function `{name}` has two blocks numbered {}", bb.id.0));
        }
    }
    let target_ok = |t: crate::ir::BlockId| -> Result<(), String> {
        if ids.contains(&t.0) {
            Ok(())
        } else {
            Err(format!("function `{name}` jumps to the missing block bb{}", t.0))
        }
    };
    for bb in &f.blocks {
        for inst in &bb.insts {
            for u in inst.uses() {
                in_range(u, "source")?;
            }
            if let Some(d) = inst.dest_reg() {
                in_range(d, "destination")?;
            }
            if let Inst::Call { func, args, .. } = inst {
                if !ctx.names.contains_key(func) {
                    return Err(format!("unknown function `{func}`"));
                }
                if let Some(&n) = ctx.arities.get(func.as_str()) {
                    if n != args.len() {
                        return Err(format!(
                            "function `{name}` calls `{func}` with {} arguments, it takes {n}",
                            args.len()
                        ));
                    }
                }
            }
        }
        match &bb.term {
            Terminator::Jump { target } => target_ok(*target)?,
            Terminator::Branch {
                cond,
                then_bb,
                else_bb,
            } => {
                in_range(*cond, "branch condition")?;
                target_ok(*then_bb)?;
                target_ok(*else_bb)?;
            }
            Terminator::Return { value: Some(r) } => in_range(*r, "return value")?,
            Terminator::Return { value: None } | Terminator::Unreachable => {}
        }
    }
    Ok(())
}

fn lower_function(f: &IrFunction, ctx: &Ctx<'_>, pool: &mut Pool) -> Result<(Vec<Op>, u16), String> {
    check_function(f, ctx)?;

    // Map block ids to instruction offsets after layout.
    let mut block_start: HashMap<u32, u32> = HashMap::new();
    let mut code: Vec<Op> = Vec::new();

    // First pass: emit with placeholder jump targets (block ids + FLAG)
    const BLOCK_FLAG: u32 = 1 << 31;

    for bb in &f.blocks {
        block_start.insert(bb.id.0, code.len() as u32);
        for inst in &bb.insts {
            emit_inst(inst, &f.name, &mut code, ctx, pool)?;
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
                    cond: reg(*cond, &f.name)?,
                    target: then_bb.0 | BLOCK_FLAG,
                });
                code.push(Op::Jump {
                    target: else_bb.0 | BLOCK_FLAG,
                });
            }
            Terminator::Return { value } => {
                if let Some(r) = value {
                    code.push(Op::Ret {
                        src: reg(*r, &f.name)?,
                    });
                } else {
                    code.push(Op::RetVoid);
                }
            }
            Terminator::Unreachable => {
                code.push(Op::RetVoid);
            }
        }
    }

    // Patch jumps; `check_function` guarantees every target block exists.
    for op in &mut code {
        match op {
            Op::Jump { target } if *target & BLOCK_FLAG != 0 => {
                let bid = *target & !BLOCK_FLAG;
                *target = block_start[&bid];
            }
            Op::JumpIf { target, .. } | Op::JumpIfNot { target, .. } if *target & BLOCK_FLAG != 0 => {
                let bid = *target & !BLOCK_FLAG;
                *target = block_start[&bid];
            }
            _ => {}
        }
    }

    let nregs = f.reg_count.max(1) as u16;
    Ok((code, nregs))
}

/// Field index / field count as the `u16` the field opcodes carry.
fn field_u16(n: usize, what: &str, fname: &str) -> Result<u16, String> {
    u16::try_from(n).map_err(|_| {
        format!("function `{fname}`: {what} {n} exceeds the {MAX_FIELDS} fields a struct, tuple or enum may have")
    })
}

fn emit_inst(
    inst: &Inst,
    fname: &str,
    code: &mut Vec<Op>,
    ctx: &Ctx<'_>,
    pool: &mut Pool,
) -> Result<(), String> {
    let names = ctx.names;
    match inst {
        Inst::LoadConst { dest, value } => match value {
            ConstValue::String(s) => {
                let idx = pool.intern(s)?;
                code.push(Op::LoadStr {
                    dest: reg(*dest, fname)?,
                    idx,
                });
            }
            other => {
                code.push(Op::LoadImm {
                    dest: reg(*dest, fname)?,
                    imm: const_to_imm(other, pool)?,
                });
            }
        },
        Inst::Move { dest, src } => code.push(Op::Move {
            dest: reg(*dest, fname)?,
            src: reg(*src, fname)?,
        }),
        Inst::Bin {
            dest,
            op,
            ty,
            lhs,
            rhs,
        } => {
            let d = reg(*dest, fname)?;
            let l = reg(*lhs, fname)?;
            let r = reg(*rhs, fname)?;
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
                (BinOp::Rem, Type::I64) => Op::RemI64 { dest: d, lhs: l, rhs: r },
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
                (BinOp::BitAnd, Type::I32 | Type::I64) => Op::BitAnd { dest: d, lhs: l, rhs: r },
                (BinOp::BitOr, Type::I32 | Type::I64) => Op::BitOr { dest: d, lhs: l, rhs: r },
                (BinOp::BitXor, Type::I32 | Type::I64) => Op::BitXor { dest: d, lhs: l, rhs: r },
                (BinOp::Shl, Type::I32 | Type::I64) => Op::Shl { dest: d, lhs: l, rhs: r },
                (BinOp::Shr, Type::I32 | Type::I64) => Op::Shr { dest: d, lhs: l, rhs: r },
                // Remaining comparisons sema accepts: i64/f64 Ne/Le/Gt/Ge,
                // bool Ne, every char ordering, string Eq/Ne.
                (BinOp::Ne | BinOp::Le | BinOp::Gt | BinOp::Ge, Type::I64 | Type::F64)
                | (BinOp::Ne, Type::Bool)
                | (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, Type::Char)
                | (BinOp::Eq | BinOp::Ne, Type::String)
                | (
                    BinOp::Eq | BinOp::Ne,
                    Type::Array { .. } | Type::Struct { .. } | Type::Tuple(_) | Type::Enum { .. },
                ) => Op::Cmp {
                    op: CmpOp::from_bin(*op).expect("comparison operator"),
                    dest: d,
                    lhs: l,
                    rhs: r,
                },
                _ => return Err(format!("unsupported operation `{op}` on `{ty}`")),
            };
            code.push(op);
        }
        Inst::Un { dest, op, ty, src } => {
            let d = reg(*dest, fname)?;
            let s = reg(*src, fname)?;
            match (op, ty) {
                (UnOp::Neg, Type::I32) => code.push(Op::NegI32 { dest: d, src: s }),
                (UnOp::Neg, Type::I64) => code.push(Op::NegI64 { dest: d, src: s }),
                (UnOp::Neg, Type::F64) => code.push(Op::NegF64 { dest: d, src: s }),
                (UnOp::Not, Type::Bool) => code.push(Op::NotBool { dest: d, src: s }),
                (UnOp::Not, Type::I32 | Type::I64) => code.push(Op::NotInt { dest: d, src: s }),
                _ => {
                    return Err(format!(
                        "unsupported operation `{}` on `{ty}`",
                        op.as_str()
                    ))
                }
            }
        }
        Inst::Call { dest, func, args } => {
            let mut argv: Vec<u16> = Vec::with_capacity(args.len());
            for r in args {
                argv.push(reg(*r, fname)?);
            }
            let Some(&idx) = names.get(func) else {
                return Err(format!("unknown function `{func}`"));
            };
            let dest_opt = match dest {
                Some(r) => Some(reg(*r, fname)?),
                None => None,
            };
            if (idx as usize) < crate::runtime::NATIVES.len() {
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
        }
        Inst::Cast { dest, src, from, to } => {
            let d = reg(*dest, fname)?;
            let s = reg(*src, fname)?;
            let op = match (from, to) {
                (Type::I32, Type::I64) => Op::CastI32ToI64 { dest: d, src: s },
                (Type::I64, Type::I32) => Op::CastI64ToI32 { dest: d, src: s },
                (Type::I32, Type::F64) => Op::CastI32ToF64 { dest: d, src: s },
                (Type::I64, Type::F64) => Op::CastI64ToF64 { dest: d, src: s },
                (Type::F64, Type::I32) => Op::CastF64ToI32 { dest: d, src: s },
                (Type::F64, Type::I64) => Op::CastF64ToI64 { dest: d, src: s },
                (Type::Bool, Type::I32) => Op::CastBoolToI32 { dest: d, src: s },
                (Type::Bool, Type::I64) => Op::CastBoolToI64 { dest: d, src: s },
                (Type::Char, Type::I32) => Op::CastCharToI32 { dest: d, src: s },
                (Type::I32, Type::Char) => Op::CastI32ToChar { dest: d, src: s },
                // `x as T` where x: T is accepted by sema and is a no-op.
                (a, b) if a == b => Op::Move { dest: d, src: s },
                _ => return Err(format!("unsupported cast `{from}` to `{to}`")),
            };
            code.push(op);
        }
        Inst::IndexLoad {
            dest, base, index, ..
        } => code.push(Op::LoadIdx {
            dest: reg(*dest, fname)?,
            base: reg(*base, fname)?,
            index: reg(*index, fname)?,
        }),
        Inst::IndexStore {
            base, index, value, ..
        } => code.push(Op::StoreIdx {
            base: reg(*base, fname)?,
            index: reg(*index, fname)?,
            value: reg(*value, fname)?,
        }),
        Inst::FieldLoad {
            dest, base, index, ..
        } => code.push(Op::LoadField {
            dest: reg(*dest, fname)?,
            base: reg(*base, fname)?,
            field: field_u16(*index, "field index", fname)?,
        }),
        Inst::FieldStore {
            base, index, value, ..
        } => code.push(Op::StoreField {
            base: reg(*base, fname)?,
            field: field_u16(*index, "field index", fname)?,
            value: reg(*value, fname)?,
        }),
        Inst::AllocArray { dest, len, .. } => {
            let n = usize::try_from(*len).ok().filter(|n| *n <= MAX_ARRAY_LEN).ok_or_else(|| {
                format!("function `{fname}`: array of {len} elements exceeds the VM limit of {MAX_ARRAY_LEN}")
            })?;
            code.push(Op::AllocArr {
                dest: reg(*dest, fname)?,
                len: n as u32,
            });
        }
        Inst::AllocStruct { dest, ty } => {
            // structs, tuples and enums (tag + payload slots) are all objects
            let Some(fields) = ty.layout_fields() else {
                return Err(format!("function `{fname}`: cannot allocate an object of type `{ty}`"));
            };
            code.push(Op::AllocObj {
                dest: reg(*dest, fname)?,
                fields: field_u16(fields.len(), "field count", fname)?,
            });
        }
        Inst::Yield => code.push(Op::Yield),
        Inst::Nop => code.push(Op::Nop),
    }
    Ok(())
}

fn const_to_imm(v: &ConstValue, pool: &mut Pool) -> Result<Immediate, String> {
    Ok(match v {
        ConstValue::I32(x) => Immediate::I32(*x),
        ConstValue::I64(x) => Immediate::I64(*x),
        ConstValue::F64(x) => Immediate::F64(x.to_bits()),
        ConstValue::Bool(x) => Immediate::Bool(*x),
        ConstValue::String(s) => Immediate::Str(pool.intern(s)?),
        ConstValue::Char(c) => Immediate::Char(*c as u32),
        ConstValue::Unit => Immediate::Unit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::emit_ir;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::sema::analyze;
    use crate::span::FileId;

    fn compile_ir(src: &str) -> IrModule {
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, d) = analyze(&prog);
        assert!(!d.has_errors(), "{d:?}");
        emit_ir(&hir.unwrap())
    }

    #[test]
    fn assembles_main() {
        let ir = compile_ir("fn main() -> i32 { return 1 + 2; }");
        let bc = assemble(&ir).expect("assemble");
        assert!(bc.function_index("main").is_some());
        let text = bc.disassemble();
        assert!(text.contains("addi32") || text.contains("loadimm"));
    }

    #[test]
    fn generic_cmp_and_new_casts_have_opcodes() {
        let src = r#"
            fn main() -> i32 {
                let a: i64 = 7;
                let b: i64 = 3;
                print_bool(a >= b);
                print_bool('a' < 'b');
                print_bool("x" == "x");
                print_bool(true != false);
                print_f64(a as f64);
                print_i32('A' as i32);
                print_i32((65 as char) as i32);
                print_i64(true as i64);
                print_i64(-a);
                print_i64(a % b);
                return 0;
            }
        "#;
        let bc = assemble(&compile_ir(src)).expect("assemble");
        let text = bc.disassemble();
        for needle in [
            "cmp.ge", "cmp.lt", "cmp.eq", "cmp.ne", "i64tof64", "chartoi32", "i32tochar",
            "booltoi64", "negi64", "remi64",
        ] {
            assert!(text.contains(needle), "missing {needle}\n{text}");
        }
        assert!(!text.contains("nop"), "{text}");
    }

    #[test]
    fn rejects_too_many_registers() {
        let mut ir = compile_ir("fn main() -> i32 { return 0; }");
        ir.functions[0].reg_count = 70_000;
        let err = assemble(&ir).unwrap_err();
        assert_eq!(
            err,
            "function `main` needs 70000 registers; the VM supports at most 65535"
        );
    }

    #[test]
    fn rejects_unknown_callee() {
        let mut ir = compile_ir("fn main() -> i32 { return 0; }");
        ir.functions[0].blocks[0].insts.push(Inst::Call {
            dest: None,
            func: "ghost".into(),
            args: Vec::new(),
        });
        let err = assemble(&ir).unwrap_err();
        assert_eq!(err, "unknown function `ghost`");
    }

    #[test]
    fn rejects_unsupported_cast() {
        let mut ir = compile_ir("fn main() -> i32 { return 0; }");
        ir.functions[0].blocks[0].insts.push(Inst::Cast {
            dest: crate::ir::Reg(0),
            src: crate::ir::Reg(0),
            from: Type::String,
            to: Type::F64,
        });
        let err = assemble(&ir).unwrap_err();
        assert_eq!(err, "unsupported cast `string` to `f64`");
    }
}
