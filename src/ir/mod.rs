//! Aether Intermediate Representation.
//!
//! Three-address code with explicit basic blocks. Not SSA: locals occupy
//! stable virtual registers so lowering to the register VM is a 1:1 map.
//! The optimizer rewrites this IR in place.

use crate::sema::{HirArm, HirBlock, HirExpr, HirExprKind, HirFn, HirPattern, HirProgram, HirStmt};
use crate::span::Span;
use crate::ty::Type;
use crate::ast::{BinOp, Literal, UnOp};
use std::fmt::{self, Write};

pub mod cfg;
pub mod dump;
pub mod verify;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Reg(pub u32);

impl fmt::Display for Reg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "%{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bb{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FuncId(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    I32(i32),
    I64(i64),
    F64(f64),
    Bool(bool),
    String(String),
    Char(char),
    Unit,
}

impl ConstValue {
    pub fn ty(&self) -> Type {
        match self {
            ConstValue::I32(_) => Type::I32,
            ConstValue::I64(_) => Type::I64,
            ConstValue::F64(_) => Type::F64,
            ConstValue::Bool(_) => Type::Bool,
            ConstValue::String(_) => Type::String,
            ConstValue::Char(_) => Type::Char,
            ConstValue::Unit => Type::Unit,
        }
    }
}

impl fmt::Display for ConstValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstValue::I32(v) => write!(f, "{v}_i32"),
            ConstValue::I64(v) => write!(f, "{v}_i64"),
            ConstValue::F64(v) => write!(f, "{v}_f64"),
            ConstValue::Bool(v) => write!(f, "{v}"),
            ConstValue::String(s) => write!(f, "{s:?}"),
            ConstValue::Char(c) => write!(f, "{c:?}"),
            ConstValue::Unit => write!(f, "()"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inst {
    LoadConst {
        dest: Reg,
        value: ConstValue,
    },
    Move {
        dest: Reg,
        src: Reg,
    },
    Bin {
        dest: Reg,
        op: BinOp,
        ty: Type,
        lhs: Reg,
        rhs: Reg,
    },
    Un {
        dest: Reg,
        op: UnOp,
        ty: Type,
        src: Reg,
    },
    Call {
        dest: Option<Reg>,
        func: String,
        args: Vec<Reg>,
    },
    Cast {
        dest: Reg,
        src: Reg,
        from: Type,
        to: Type,
    },
    IndexLoad {
        dest: Reg,
        base: Reg,
        index: Reg,
        elem: Type,
    },
    IndexStore {
        base: Reg,
        index: Reg,
        value: Reg,
        elem: Type,
    },
    FieldLoad {
        dest: Reg,
        base: Reg,
        index: usize,
        ty: Type,
    },
    FieldStore {
        base: Reg,
        index: usize,
        value: Reg,
        ty: Type,
    },
    AllocArray {
        dest: Reg,
        elem: Type,
        len: i64,
    },
    AllocStruct {
        dest: Reg,
        ty: Type,
    },
    /// Suspends a budgeted VM run; never removed (it is an effect).
    Yield,
    /// Marker used by DCE: instruction has no effect.
    Nop,
}

impl Inst {
    pub fn dest_reg(&self) -> Option<Reg> {
        match self {
            Inst::LoadConst { dest, .. }
            | Inst::Move { dest, .. }
            | Inst::Bin { dest, .. }
            | Inst::Un { dest, .. }
            | Inst::Cast { dest, .. }
            | Inst::IndexLoad { dest, .. }
            | Inst::FieldLoad { dest, .. }
            | Inst::AllocArray { dest, .. }
            | Inst::AllocStruct { dest, .. } => Some(*dest),
            Inst::Call { dest, .. } => *dest,
            Inst::IndexStore { .. } | Inst::FieldStore { .. } | Inst::Yield | Inst::Nop => None,
        }
    }

    pub fn uses(&self) -> Vec<Reg> {
        match self {
            Inst::LoadConst { .. }
            | Inst::AllocArray { .. }
            | Inst::AllocStruct { .. }
            | Inst::Yield
            | Inst::Nop => {
                Vec::new()
            }
            Inst::Move { src, .. } | Inst::Un { src, .. } | Inst::Cast { src, .. } => vec![*src],
            Inst::Bin { lhs, rhs, .. } => vec![*lhs, *rhs],
            Inst::Call { args, .. } => args.clone(),
            Inst::IndexLoad { base, index, .. } => vec![*base, *index],
            Inst::IndexStore {
                base, index, value, ..
            } => vec![*base, *index, *value],
            Inst::FieldLoad { base, .. } => vec![*base],
            Inst::FieldStore { base, value, .. } => vec![*base, *value],
        }
    }

    pub fn is_pure(&self) -> bool {
        match self {
            Inst::Call { .. }
            | Inst::IndexStore { .. }
            | Inst::FieldStore { .. }
            | Inst::Yield => false,
            Inst::Nop => true,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Terminator {
    Jump {
        target: BlockId,
    },
    Branch {
        cond: Reg,
        then_bb: BlockId,
        else_bb: BlockId,
    },
    Return {
        value: Option<Reg>,
    },
    Unreachable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BasicBlock {
    pub id: BlockId,
    pub insts: Vec<Inst>,
    pub term: Terminator,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrFunction {
    pub name: String,
    pub params: Vec<(String, Type, Reg)>,
    pub return_ty: Type,
    pub blocks: Vec<BasicBlock>,
    pub reg_count: u32,
    pub is_extern: bool,
    pub span: Span,
}

impl IrFunction {
    pub fn block_mut(&mut self, id: BlockId) -> Option<&mut BasicBlock> {
        self.blocks.iter_mut().find(|b| b.id == id)
    }

    pub fn block(&self, id: BlockId) -> Option<&BasicBlock> {
        self.blocks.iter().find(|b| b.id == id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IrModule {
    pub functions: Vec<IrFunction>,
    pub structs: Vec<(String, Type)>,
}

impl IrModule {
    pub fn function(&self, name: &str) -> Option<&IrFunction> {
        self.functions.iter().find(|f| f.name == name)
    }
}

pub fn emit_ir(hir: &HirProgram) -> IrModule {
    let mut functions = Vec::new();
    for f in &hir.functions {
        functions.push(lower_fn(f));
    }
    IrModule {
        functions,
        structs: hir
            .structs
            .iter()
            .map(|s| (s.name.clone(), s.ty.clone()))
            .collect(),
    }
}

struct Builder {
    blocks: Vec<BasicBlock>,
    current: BlockId,
    next_reg: u32,
    next_block: u32,
    locals: Vec<(String, Reg, Type)>,
    /// `break` / `continue` targets of the innermost loop, for `match`
    /// expressions whose arms may contain them.
    loop_ctx: (Option<BlockId>, Option<BlockId>),
}

impl Builder {
    fn new() -> Self {
        let entry = BlockId(0);
        Builder {
            blocks: vec![BasicBlock {
                id: entry,
                insts: Vec::new(),
                term: Terminator::Unreachable,
            }],
            current: entry,
            next_reg: 0,
            next_block: 1,
            locals: Vec::new(),
            loop_ctx: (None, None),
        }
    }

    fn alloc_reg(&mut self) -> Reg {
        let r = Reg(self.next_reg);
        self.next_reg += 1;
        r
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        self.blocks.push(BasicBlock {
            id,
            insts: Vec::new(),
            term: Terminator::Unreachable,
        });
        id
    }

    fn emit(&mut self, inst: Inst) {
        // block ids are indices: `new_block` pushes in id order
        if let Some(bb) = self.blocks.get_mut(self.current.0 as usize) {
            bb.insts.push(inst);
        }
    }

    fn set_term(&mut self, term: Terminator) {
        if let Some(bb) = self.blocks.get_mut(self.current.0 as usize) {
            bb.term = term;
        }
    }

    fn switch(&mut self, id: BlockId) {
        self.current = id;
    }

    fn bind(&mut self, name: String, reg: Reg, ty: Type) {
        self.locals.push((name, reg, ty));
    }

    fn lookup(&self, name: &str) -> Option<Reg> {
        self.locals
            .iter()
            .rev()
            .find(|(n, _, _)| n == name)
            .map(|(_, r, _)| *r)
    }

    fn unbind_to(&mut self, len: usize) {
        self.locals.truncate(len);
    }
}

fn lower_fn(f: &HirFn) -> IrFunction {
    let mut b = Builder::new();
    let mut params = Vec::new();
    for (name, ty) in &f.params {
        let r = b.alloc_reg();
        b.bind(name.clone(), r, ty.clone());
        params.push((name.clone(), ty.clone(), r));
    }
    if let Some(body) = &f.body {
        let tail = lower_block(&mut b, body, None, None);
        // implicit return: the block's tail expression, unit, or a default
        match &b.blocks.get(b.current.0 as usize).map(|bb| &bb.term) {
            Some(Terminator::Unreachable) | Some(Terminator::Jump { .. }) | None => {
                if let (Some(r), false) = (tail, f.return_ty == Type::Unit) {
                    b.set_term(Terminator::Return { value: Some(r) });
                } else if f.return_ty == Type::Unit {
                    b.set_term(Terminator::Return { value: None });
                } else if f.return_ty == Type::I32 {
                    let r = b.alloc_reg();
                    b.emit(Inst::LoadConst {
                        dest: r,
                        value: ConstValue::I32(0),
                    });
                    b.set_term(Terminator::Return { value: Some(r) });
                } else {
                    b.set_term(Terminator::Return { value: None });
                }
            }
            _ => {}
        }
    } else {
        b.set_term(Terminator::Return { value: None });
    }
    IrFunction {
        name: f.name.clone(),
        params,
        return_ty: f.return_ty.clone(),
        blocks: b.blocks,
        reg_count: b.next_reg,
        is_extern: f.is_extern,
        span: f.span,
    }
}

/// Lowers a block and returns the register holding its tail expression, if any.
fn lower_block(
    b: &mut Builder,
    block: &HirBlock,
    break_bb: Option<BlockId>,
    continue_bb: Option<BlockId>,
) -> Option<Reg> {
    let mark = b.locals.len();
    let saved = b.loop_ctx;
    b.loop_ctx = (break_bb, continue_bb);
    for stmt in &block.stmts {
        lower_stmt(b, stmt, break_bb, continue_bb);
    }
    let tail = block.tail.as_ref().map(|tail| lower_expr(b, tail));
    b.loop_ctx = saved;
    b.unbind_to(mark);
    tail
}

fn lower_stmt(
    b: &mut Builder,
    stmt: &HirStmt,
    break_bb: Option<BlockId>,
    continue_bb: Option<BlockId>,
) {
    match stmt {
        HirStmt::Let { name, ty, init, .. } => {
            let dest = b.alloc_reg();
            if let Some(init) = init {
                let src = lower_expr(b, init);
                b.emit(Inst::Move { dest, src });
            } else {
                b.emit(Inst::LoadConst {
                    dest,
                    value: default_const(ty),
                });
            }
            b.bind(name.clone(), dest, ty.clone());
        }
        HirStmt::Assign { target, value, .. } => {
            let src = lower_expr(b, value);
            assign_to(b, target, src);
        }
        HirStmt::Expr(e) => {
            let _ = lower_expr(b, e);
        }
        HirStmt::Return { value, .. } => {
            let r = value.as_ref().map(|e| lower_expr(b, e));
            b.set_term(Terminator::Return { value: r });
            // subsequent code goes into a fresh unreachable-looking block
            let dead = b.new_block();
            b.switch(dead);
        }
        HirStmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            let c = lower_expr(b, cond);
            let then_bb = b.new_block();
            let else_bb = b.new_block();
            let join = b.new_block();
            b.set_term(Terminator::Branch {
                cond: c,
                then_bb,
                else_bb,
            });
            b.switch(then_bb);
            lower_block(b, then_block, break_bb, continue_bb);
            if matches!(
                b.blocks.get(b.current.0 as usize).map(|bb| &bb.term),
                Some(Terminator::Unreachable)
            ) {
                b.set_term(Terminator::Jump { target: join });
            }
            b.switch(else_bb);
            if let Some(eb) = else_block {
                lower_block(b, eb, break_bb, continue_bb);
            }
            if matches!(
                b.blocks.get(b.current.0 as usize).map(|bb| &bb.term),
                Some(Terminator::Unreachable)
            ) {
                b.set_term(Terminator::Jump { target: join });
            }
            b.switch(join);
        }
        HirStmt::While { cond, body, .. } => {
            let header = b.new_block();
            let body_bb = b.new_block();
            let exit = b.new_block();
            b.set_term(Terminator::Jump { target: header });
            b.switch(header);
            let c = lower_expr(b, cond);
            b.set_term(Terminator::Branch {
                cond: c,
                then_bb: body_bb,
                else_bb: exit,
            });
            b.switch(body_bb);
            lower_block(b, body, Some(exit), Some(header));
            if matches!(
                b.blocks.get(b.current.0 as usize).map(|bb| &bb.term),
                Some(Terminator::Unreachable)
            ) {
                b.set_term(Terminator::Jump { target: header });
            }
            b.switch(exit);
        }
        HirStmt::For {
            var,
            start,
            end,
            body,
            ..
        } => {
            let i = b.alloc_reg();
            let s = lower_expr(b, start);
            b.emit(Inst::Move { dest: i, src: s });
            // The bound is evaluated once: copy it, since a plain local's
            // register would follow later writes to that variable.
            let end_reg = lower_expr(b, end);
            let limit = b.alloc_reg();
            b.emit(Inst::Move {
                dest: limit,
                src: end_reg,
            });
            // the loop variable is scoped to the loop
            let mark = b.locals.len();
            b.bind(var.clone(), i, Type::I32);
            let header = b.new_block();
            let body_bb = b.new_block();
            let incr = b.new_block();
            let exit = b.new_block();
            b.set_term(Terminator::Jump { target: header });
            b.switch(header);
            let cmp = b.alloc_reg();
            b.emit(Inst::Bin {
                dest: cmp,
                op: BinOp::Lt,
                ty: Type::I32,
                lhs: i,
                rhs: limit,
            });
            b.set_term(Terminator::Branch {
                cond: cmp,
                then_bb: body_bb,
                else_bb: exit,
            });
            b.switch(body_bb);
            lower_block(b, body, Some(exit), Some(incr));
            if matches!(
                b.blocks.get(b.current.0 as usize).map(|bb| &bb.term),
                Some(Terminator::Unreachable)
            ) {
                b.set_term(Terminator::Jump { target: incr });
            }
            b.switch(incr);
            let one = b.alloc_reg();
            b.emit(Inst::LoadConst {
                dest: one,
                value: ConstValue::I32(1),
            });
            b.emit(Inst::Bin {
                dest: i,
                op: BinOp::Add,
                ty: Type::I32,
                lhs: i,
                rhs: one,
            });
            b.set_term(Terminator::Jump { target: header });
            b.switch(exit);
            b.unbind_to(mark);
        }
        HirStmt::Break(_) => {
            if let Some(t) = break_bb {
                b.set_term(Terminator::Jump { target: t });
                let dead = b.new_block();
                b.switch(dead);
            }
        }
        HirStmt::Yield(_) => b.emit(Inst::Yield),
        HirStmt::Continue(_) => {
            if let Some(t) = continue_bb {
                b.set_term(Terminator::Jump { target: t });
                let dead = b.new_block();
                b.switch(dead);
            }
        }
        HirStmt::Block(block) => {
            lower_block(b, block, break_bb, continue_bb);
        }
        HirStmt::Match {
            scrutinee, arms, ..
        } => {
            lower_match(b, scrutinee, arms, None, break_bb, continue_bb);
        }
    }
}

/// True when the current block has no terminator yet (fell through).
fn current_is_open(b: &Builder) -> bool {
    matches!(
        b.blocks.get(b.current.0 as usize).map(|bb| &bb.term),
        Some(Terminator::Unreachable)
    )
}

/// `match`: arms are tried in order. Each arm's pattern is lowered to a
/// chain of tests (`Branch`es to the next arm on failure) and bindings
/// (`FieldLoad`s into fresh registers scoped to the arm); all arms jump to a
/// common join block. With `result` (a `match` expression) every arm moves
/// its tail value there.
fn lower_match(
    b: &mut Builder,
    scrutinee: &HirExpr,
    arms: &[HirArm],
    result: Option<Reg>,
    break_bb: Option<BlockId>,
    continue_bb: Option<BlockId>,
) {
    let s = lower_expr(b, scrutinee);
    // A copy keeps the arms independent of later writes to the scrutinee's
    // register when it is a plain local.
    let value = b.alloc_reg();
    b.emit(Inst::Move { dest: value, src: s });
    // The tag of an enum scrutinee is read once for all the arms.
    let tag = if matches!(scrutinee.ty, Type::Enum { .. }) {
        let t = b.alloc_reg();
        b.emit(Inst::FieldLoad {
            dest: t,
            base: value,
            index: 0,
            ty: Type::I32,
        });
        Some(t)
    } else {
        None
    };
    let join = b.new_block();
    for arm in arms {
        let mark = b.locals.len();
        let mut next = None;
        lower_pattern(b, &arm.pattern, value, tag, &mut next);
        let tail = lower_block(b, &arm.body, break_bb, continue_bb);
        b.unbind_to(mark);
        if let (Some(dest), Some(src)) = (result, tail) {
            b.emit(Inst::Move { dest, src });
        }
        if current_is_open(b) {
            b.set_term(Terminator::Jump { target: join });
        }
        match next {
            Some(next) => b.switch(next),
            None => {
                // irrefutable arm: the remaining arms are unreachable
                let dead = b.new_block();
                b.switch(dead);
                break;
            }
        }
    }
    // no arm matched (impossible for an exhaustive match): fall through
    if current_is_open(b) {
        b.set_term(Terminator::Jump { target: join });
    }
    b.switch(join);
}

/// Lowers a `match` used as a value; its arms may `break` / `continue` the
/// enclosing loop.
fn lower_match_expr(b: &mut Builder, expr: &HirExpr, scrutinee: &HirExpr, arms: &[HirArm]) -> Reg {
    let dest = b.alloc_reg();
    // Defined on every path, including the unreachable fall-through.
    b.emit(Inst::LoadConst {
        dest,
        value: default_const(&expr.ty),
    });
    let (brk, cont) = b.loop_ctx;
    lower_match(b, scrutinee, arms, Some(dest), brk, cont);
    dest
}

/// Emits the tests and bindings of `pat` against the value in `value`
/// (of type `ty`). A failing test branches to `*next`, created on first use;
/// it stays `None` when the pattern cannot fail.
fn lower_pattern(
    b: &mut Builder,
    pat: &HirPattern,
    value: Reg,
    known_tag: Option<Reg>,
    next: &mut Option<BlockId>,
) {
    match pat {
        HirPattern::Wildcard => {}
        HirPattern::Binding { name, ty } => {
            let r = b.alloc_reg();
            b.emit(Inst::Move { dest: r, src: value });
            b.bind(name.clone(), r, ty.clone());
        }
        HirPattern::Literal { lit, ty } => {
            if *lit == Literal::Unit {
                return;
            }
            let c = b.alloc_reg();
            b.emit(Inst::LoadConst {
                dest: c,
                value: lit_to_const(lit, ty),
            });
            let cond = b.alloc_reg();
            b.emit(Inst::Bin {
                dest: cond,
                op: BinOp::Eq,
                ty: ty.clone(),
                lhs: value,
                rhs: c,
            });
            branch_or_next(b, cond, next);
        }
        HirPattern::Variant {
            tag,
            variants,
            fields,
            tys,
        } => {
            if *variants > 1 {
                let t = known_tag.unwrap_or_else(|| {
                    let t = b.alloc_reg();
                    b.emit(Inst::FieldLoad {
                        dest: t,
                        base: value,
                        index: 0,
                        ty: Type::I32,
                    });
                    t
                });
                let c = b.alloc_reg();
                b.emit(Inst::LoadConst {
                    dest: c,
                    value: ConstValue::I32(*tag as i32),
                });
                let cond = b.alloc_reg();
                b.emit(Inst::Bin {
                    dest: cond,
                    op: BinOp::Eq,
                    ty: Type::I32,
                    lhs: t,
                    rhs: c,
                });
                branch_or_next(b, cond, next);
            }
            lower_subpatterns(b, fields, tys, 1, value, next);
        }
        HirPattern::Tuple { elems, tys } => {
            lower_subpatterns(b, elems, tys, 0, value, next);
        }
    }
}

/// Sub-patterns of an aggregate: the one for field `first + i` is loaded
/// only when it tests or binds something.
fn lower_subpatterns(
    b: &mut Builder,
    pats: &[HirPattern],
    tys: &[Type],
    first: usize,
    value: Reg,
    next: &mut Option<BlockId>,
) {
    for (i, (p, fty)) in pats.iter().zip(tys).enumerate() {
        if matches!(p, HirPattern::Wildcard) {
            continue;
        }
        let r = b.alloc_reg();
        b.emit(Inst::FieldLoad {
            dest: r,
            base: value,
            index: first + i,
            ty: fty.clone(),
        });
        match p {
            // bind the loaded register itself, no copy
            HirPattern::Binding { name, ty } => b.bind(name.clone(), r, ty.clone()),
            _ => lower_pattern(b, p, r, None, next),
        }
    }
}

/// Continues in a fresh block when `cond` holds, else goes to `*next`.
fn branch_or_next(b: &mut Builder, cond: Reg, next: &mut Option<BlockId>) {
    let ok = b.new_block();
    let fail = match *next {
        Some(n) => n,
        None => {
            let n = b.new_block();
            *next = Some(n);
            n
        }
    };
    b.set_term(Terminator::Branch {
        cond,
        then_bb: ok,
        else_bb: fail,
    });
    b.switch(ok);
}

/// `==` on tuples, enums and structs is elementwise: a short-circuit chain
/// that compares the tag (enums) and each payload slot / element / field
/// with the comparison of its own type, recursively. (The VM has no
/// object comparison opcode; arrays compare with an index loop.)
fn lower_agg_eq(b: &mut Builder, lhs: Reg, rhs: Reg, ty: &Type) -> Reg {
    let dest = b.alloc_reg();
    match ty {
        Type::Tuple(_) | Type::Struct { .. } => {
            b.emit(Inst::LoadConst {
                dest,
                value: ConstValue::Bool(true),
            });
            let join = b.new_block();
            let slots: Vec<(usize, Type)> = ty
                .layout_fields()
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .collect();
            lower_slots_eq(b, dest, lhs, rhs, &slots, join);
            b.switch(join);
        }
        Type::Enum { variants, .. } => {
            let (lt, rt) = (b.alloc_reg(), b.alloc_reg());
            b.emit(Inst::FieldLoad {
                dest: lt,
                base: lhs,
                index: 0,
                ty: Type::I32,
            });
            b.emit(Inst::FieldLoad {
                dest: rt,
                base: rhs,
                index: 0,
                ty: Type::I32,
            });
            b.emit(Inst::Bin {
                dest,
                op: BinOp::Eq,
                ty: Type::I32,
                lhs: lt,
                rhs: rt,
            });
            let join = b.new_block();
            let payload_bb = b.new_block();
            b.set_term(Terminator::Branch {
                cond: dest,
                then_bb: payload_bb,
                else_bb: join,
            });
            b.switch(payload_bb);
            for (tag, (_, payload)) in variants.iter().enumerate() {
                if payload.is_empty() {
                    continue;
                }
                let c = b.alloc_reg();
                b.emit(Inst::LoadConst {
                    dest: c,
                    value: ConstValue::I32(tag as i32),
                });
                let cond = b.alloc_reg();
                b.emit(Inst::Bin {
                    dest: cond,
                    op: BinOp::Eq,
                    ty: Type::I32,
                    lhs: lt,
                    rhs: c,
                });
                let this_bb = b.new_block();
                let next_bb = b.new_block();
                b.set_term(Terminator::Branch {
                    cond,
                    then_bb: this_bb,
                    else_bb: next_bb,
                });
                b.switch(this_bb);
                let slots: Vec<(usize, Type)> = payload
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(i, t)| (i + 1, t))
                    .collect();
                lower_slots_eq(b, dest, lhs, rhs, &slots, join);
                b.switch(next_bb);
            }
            // payload-less variant: tags were equal, so the values are equal
            b.set_term(Terminator::Jump { target: join });
            b.switch(join);
        }
        Type::Array { elem, len } => {
            // a loop over the index: stops at the first differing element
            b.emit(Inst::LoadConst {
                dest,
                value: ConstValue::Bool(true),
            });
            let i = b.alloc_reg();
            b.emit(Inst::LoadConst {
                dest: i,
                value: ConstValue::I32(0),
            });
            let limit = b.alloc_reg();
            b.emit(Inst::LoadConst {
                dest: limit,
                value: ConstValue::I32(*len as i32),
            });
            let header = b.new_block();
            let body = b.new_block();
            let step = b.new_block();
            let join = b.new_block();
            b.set_term(Terminator::Jump { target: header });
            b.switch(header);
            let more = b.alloc_reg();
            b.emit(Inst::Bin {
                dest: more,
                op: BinOp::Lt,
                ty: Type::I32,
                lhs: i,
                rhs: limit,
            });
            b.set_term(Terminator::Branch {
                cond: more,
                then_bb: body,
                else_bb: join,
            });
            b.switch(body);
            let (l, r) = (b.alloc_reg(), b.alloc_reg());
            for (d, base) in [(l, lhs), (r, rhs)] {
                b.emit(Inst::IndexLoad {
                    dest: d,
                    base,
                    index: i,
                    elem: (**elem).clone(),
                });
            }
            let eq = lower_agg_eq(b, l, r, elem);
            b.emit(Inst::Move { dest, src: eq });
            b.set_term(Terminator::Branch {
                cond: eq,
                then_bb: step,
                else_bb: join,
            });
            b.switch(step);
            let one = b.alloc_reg();
            b.emit(Inst::LoadConst {
                dest: one,
                value: ConstValue::I32(1),
            });
            b.emit(Inst::Bin {
                dest: i,
                op: BinOp::Add,
                ty: Type::I32,
                lhs: i,
                rhs: one,
            });
            b.set_term(Terminator::Jump { target: header });
            b.switch(join);
        }
        other => {
            b.emit(Inst::Bin {
                dest,
                op: BinOp::Eq,
                ty: other.clone(),
                lhs,
                rhs,
            });
        }
    }
    dest
}

/// Compares the given `(field index, type)` slots of two aggregates in
/// order, writing each partial result to `dest` and jumping to `join` as
/// soon as one differs. Ends in `join`.
fn lower_slots_eq(
    b: &mut Builder,
    dest: Reg,
    lhs: Reg,
    rhs: Reg,
    slots: &[(usize, Type)],
    join: BlockId,
) {
    for (index, ty) in slots {
        let (l, r) = (b.alloc_reg(), b.alloc_reg());
        b.emit(Inst::FieldLoad {
            dest: l,
            base: lhs,
            index: *index,
            ty: ty.clone(),
        });
        b.emit(Inst::FieldLoad {
            dest: r,
            base: rhs,
            index: *index,
            ty: ty.clone(),
        });
        let eq = lower_agg_eq(b, l, r, ty);
        b.emit(Inst::Move { dest, src: eq });
        let cont = b.new_block();
        b.set_term(Terminator::Branch {
            cond: eq,
            then_bb: cont,
            else_bb: join,
        });
        b.switch(cont);
    }
    b.set_term(Terminator::Jump { target: join });
}

/// Stores `src` into an lvalue.
///
/// Aggregates have value semantics: loading `a[i]` yields a copy, so a store
/// into a nested place (`a[i][j] = v`, `o.p.x = v`) mutates that copy and then
/// writes the copy back into its parent, recursively, until a local is reached.
fn assign_to(b: &mut Builder, target: &HirExpr, src: Reg) {
    match &target.kind {
        HirExprKind::Local(name) => {
            if let Some(dest) = b.lookup(name) {
                b.emit(Inst::Move { dest, src });
            }
        }
        HirExprKind::Index { base, index } => {
            let br = lower_expr(b, base);
            let ir = lower_expr(b, index);
            b.emit(Inst::IndexStore {
                base: br,
                index: ir,
                value: src,
                elem: target.ty.clone(),
            });
            write_back(b, base, br);
        }
        HirExprKind::Field { base, index, .. } => {
            let br = lower_expr(b, base);
            b.emit(Inst::FieldStore {
                base: br,
                index: *index,
                value: src,
                ty: target.ty.clone(),
            });
            write_back(b, base, br);
        }
        _ => {}
    }
}

/// After mutating the copy `reg` of a nested place, store it back into its parent.
/// A local's register is mutated in place, so nothing is needed there.
fn write_back(b: &mut Builder, base: &HirExpr, reg: Reg) {
    if !matches!(base.kind, HirExprKind::Local(_)) {
        assign_to(b, base, reg);
    }
}

/// Lowers operands left to right. A bare local's register is the variable
/// itself, so when a later operand contains a `match` (whose arms may assign
/// that variable) the value is copied first.
fn lower_seq(b: &mut Builder, es: &[&HirExpr]) -> Vec<Reg> {
    let mut regs = Vec::with_capacity(es.len());
    for (i, e) in es.iter().enumerate() {
        let mut r = lower_expr(b, e);
        if matches!(e.kind, HirExprKind::Local(_)) && es[i + 1..].iter().any(|x| contains_match(x)) {
            let t = b.alloc_reg();
            b.emit(Inst::Move { dest: t, src: r });
            r = t;
        }
        regs.push(r);
    }
    regs
}

fn lower_pair(b: &mut Builder, l: &HirExpr, r: &HirExpr) -> (Reg, Reg) {
    let regs = lower_seq(b, &[l, r]);
    (regs[0], regs[1])
}

fn contains_match(e: &HirExpr) -> bool {
    match &e.kind {
        HirExprKind::Match { .. } => true,
        HirExprKind::Literal(_) | HirExprKind::Local(_) => false,
        HirExprKind::Binary { lhs, rhs, .. } => contains_match(lhs) || contains_match(rhs),
        HirExprKind::Unary { expr, .. } | HirExprKind::Cast { expr, .. } => contains_match(expr),
        HirExprKind::Call { args, .. } => args.iter().any(contains_match),
        HirExprKind::Index { base, index } => contains_match(base) || contains_match(index),
        HirExprKind::Field { base, .. } => contains_match(base),
        HirExprKind::Array { elements } | HirExprKind::Tuple { elements } => {
            elements.iter().any(contains_match)
        }
        HirExprKind::StructLit { fields, .. } => fields.iter().any(|(_, e)| contains_match(e)),
        HirExprKind::EnumLit { args, .. } => args.iter().any(contains_match),
    }
}

fn lower_expr(b: &mut Builder, expr: &HirExpr) -> Reg {
    match &expr.kind {
        HirExprKind::Literal(lit) => {
            let dest = b.alloc_reg();
            b.emit(Inst::LoadConst {
                dest,
                value: lit_to_const(lit, &expr.ty),
            });
            dest
        }
        HirExprKind::Local(name) => {
            if let Some(r) = b.lookup(name) {
                r
            } else {
                // function name used as value — not supported; produce 0
                let dest = b.alloc_reg();
                b.emit(Inst::LoadConst {
                    dest,
                    value: ConstValue::I32(0),
                });
                dest
            }
        }
        HirExprKind::Binary { op, lhs, rhs } if op.is_logical() => {
            // short-circuit: the right operand runs only when it decides the result
            let dest = b.alloc_reg();
            let l = lower_expr(b, lhs);
            b.emit(Inst::Move { dest, src: l });
            let rhs_bb = b.new_block();
            let join = b.new_block();
            let (then_bb, else_bb) = if *op == BinOp::And {
                (rhs_bb, join)
            } else {
                (join, rhs_bb)
            };
            b.set_term(Terminator::Branch {
                cond: l,
                then_bb,
                else_bb,
            });
            b.switch(rhs_bb);
            let r = lower_expr(b, rhs);
            b.emit(Inst::Move { dest, src: r });
            b.set_term(Terminator::Jump { target: join });
            b.switch(join);
            dest
        }
        HirExprKind::Binary { op, lhs, rhs }
            if matches!(op, BinOp::Eq | BinOp::Ne)
                && matches!(
                    lhs.ty,
                    Type::Tuple(_) | Type::Enum { .. } | Type::Struct { .. } | Type::Array { .. }
                ) =>
        {
            let (l, r) = lower_pair(b, lhs, rhs);
            let eq = lower_agg_eq(b, l, r, &lhs.ty);
            if *op == BinOp::Eq {
                eq
            } else {
                let dest = b.alloc_reg();
                b.emit(Inst::Un {
                    dest,
                    op: UnOp::Not,
                    ty: Type::Bool,
                    src: eq,
                });
                dest
            }
        }
        HirExprKind::Binary { op, lhs, rhs } => {
            let (l, r) = lower_pair(b, lhs, rhs);
            let dest = b.alloc_reg();
            b.emit(Inst::Bin {
                dest,
                op: *op,
                ty: lhs.ty.clone(),
                lhs: l,
                rhs: r,
            });
            dest
        }
        HirExprKind::Unary { op, expr: inner } => {
            let s = lower_expr(b, inner);
            let dest = b.alloc_reg();
            b.emit(Inst::Un {
                dest,
                op: *op,
                ty: inner.ty.clone(),
                src: s,
            });
            dest
        }
        HirExprKind::Call { name, args } => {
            let regs = lower_seq(b, &args.iter().collect::<Vec<_>>());
            let dest = if expr.ty == Type::Unit {
                None
            } else {
                Some(b.alloc_reg())
            };
            b.emit(Inst::Call {
                dest,
                func: name.clone(),
                args: regs,
            });
            dest.unwrap_or_else(|| {
                let d = b.alloc_reg();
                b.emit(Inst::LoadConst {
                    dest: d,
                    value: ConstValue::Unit,
                });
                d
            })
        }
        HirExprKind::Index { base, index } => {
            let (br, ir) = lower_pair(b, base, index);
            let dest = b.alloc_reg();
            b.emit(Inst::IndexLoad {
                dest,
                base: br,
                index: ir,
                elem: expr.ty.clone(),
            });
            dest
        }
        HirExprKind::Field { base, index, .. } => {
            let br = lower_expr(b, base);
            let dest = b.alloc_reg();
            b.emit(Inst::FieldLoad {
                dest,
                base: br,
                index: *index,
                ty: expr.ty.clone(),
            });
            dest
        }
        HirExprKind::Array { elements } => {
            let dest = b.alloc_reg();
            let elem_ty = match &expr.ty {
                Type::Array { elem, .. } => *elem.clone(),
                _ => Type::I32,
            };
            b.emit(Inst::AllocArray {
                dest,
                elem: elem_ty.clone(),
                len: elements.len() as i64,
            });
            for (i, e) in elements.iter().enumerate() {
                let v = lower_expr(b, e);
                let idx = b.alloc_reg();
                b.emit(Inst::LoadConst {
                    dest: idx,
                    value: ConstValue::I32(i as i32),
                });
                b.emit(Inst::IndexStore {
                    base: dest,
                    index: idx,
                    value: v,
                    elem: elem_ty.clone(),
                });
            }
            dest
        }
        HirExprKind::StructLit { fields, .. } => {
            let dest = b.alloc_reg();
            b.emit(Inst::AllocStruct {
                dest,
                ty: expr.ty.clone(),
            });
            // evaluate in source order, store at the declared field index
            for (name, e) in fields {
                let v = lower_expr(b, e);
                let index = expr.ty.field(name).map(|(i, _)| i).unwrap_or(0);
                b.emit(Inst::FieldStore {
                    base: dest,
                    index,
                    value: v,
                    ty: e.ty.clone(),
                });
            }
            dest
        }
        HirExprKind::Tuple { elements } => {
            let dest = b.alloc_reg();
            b.emit(Inst::AllocStruct {
                dest,
                ty: expr.ty.clone(),
            });
            for (index, e) in elements.iter().enumerate() {
                let v = lower_expr(b, e);
                b.emit(Inst::FieldStore {
                    base: dest,
                    index,
                    value: v,
                    ty: e.ty.clone(),
                });
            }
            dest
        }
        HirExprKind::EnumLit { tag, args } => {
            let dest = b.alloc_reg();
            b.emit(Inst::AllocStruct {
                dest,
                ty: expr.ty.clone(),
            });
            let t = b.alloc_reg();
            b.emit(Inst::LoadConst {
                dest: t,
                value: ConstValue::I32(*tag as i32),
            });
            b.emit(Inst::FieldStore {
                base: dest,
                index: 0,
                value: t,
                ty: Type::I32,
            });
            for (i, e) in args.iter().enumerate() {
                let v = lower_expr(b, e);
                b.emit(Inst::FieldStore {
                    base: dest,
                    index: i + 1,
                    value: v,
                    ty: e.ty.clone(),
                });
            }
            dest
        }
        HirExprKind::Match { scrutinee, arms } => lower_match_expr(b, expr, scrutinee, arms),
        HirExprKind::Cast { expr: inner, to } => {
            let s = lower_expr(b, inner);
            let dest = b.alloc_reg();
            b.emit(Inst::Cast {
                dest,
                src: s,
                from: inner.ty.clone(),
                to: to.clone(),
            });
            dest
        }
    }
}

fn lit_to_const(lit: &Literal, ty: &Type) -> ConstValue {
    match lit {
        Literal::Int(v) => {
            if *ty == Type::I64 {
                ConstValue::I64(*v)
            } else {
                ConstValue::I32(*v as i32)
            }
        }
        Literal::Float(v) => ConstValue::F64(*v),
        Literal::Bool(v) => ConstValue::Bool(*v),
        Literal::String(s) => ConstValue::String(s.clone()),
        Literal::Char(c) => ConstValue::Char(*c),
        Literal::Unit => ConstValue::Unit,
    }
}

fn default_const(ty: &Type) -> ConstValue {
    match ty {
        Type::I32 => ConstValue::I32(0),
        Type::I64 => ConstValue::I64(0),
        Type::F64 => ConstValue::F64(0.0),
        Type::Bool => ConstValue::Bool(false),
        Type::String => ConstValue::String(String::new()),
        Type::Char => ConstValue::Char('\0'),
        _ => ConstValue::Unit,
    }
}

impl fmt::Display for Inst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Inst::LoadConst { dest, value } => write!(f, "  {dest} = const {value}"),
            Inst::Move { dest, src } => write!(f, "  {dest} = mov {src}"),
            Inst::Bin {
                dest,
                op,
                ty,
                lhs,
                rhs,
            } => write!(f, "  {dest} = {op}.{ty} {lhs}, {rhs}"),
            Inst::Un { dest, op, ty, src } => write!(f, "  {dest} = {}.{ty} {src}", op.as_str()),
            Inst::Call { dest, func, args } => {
                let a: Vec<_> = args.iter().map(|r| r.to_string()).collect();
                match dest {
                    Some(d) => write!(f, "  {d} = call {func}({})", a.join(", ")),
                    None => write!(f, "  call {func}({})", a.join(", ")),
                }
            }
            Inst::Cast { dest, src, from, to } => write!(f, "  {dest} = cast {from}->{to} {src}"),
            Inst::IndexLoad {
                dest, base, index, ..
            } => write!(f, "  {dest} = load {base}[{index}]"),
            Inst::IndexStore {
                base, index, value, ..
            } => write!(f, "  store {base}[{index}] = {value}"),
            Inst::FieldLoad {
                dest, base, index, ..
            } => write!(f, "  {dest} = field {base}.{index}"),
            Inst::FieldStore {
                base, index, value, ..
            } => write!(f, "  store {base}.{index} = {value}"),
            Inst::AllocArray { dest, elem, len } => {
                write!(f, "  {dest} = alloc [{elem}; {len}]")
            }
            Inst::AllocStruct { dest, ty } => write!(f, "  {dest} = alloc {ty}"),
            Inst::Yield => write!(f, "  yield"),
            Inst::Nop => write!(f, "  nop"),
        }
    }
}

impl fmt::Display for Terminator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Terminator::Jump { target } => write!(f, "  jmp {target}"),
            Terminator::Branch {
                cond,
                then_bb,
                else_bb,
            } => write!(f, "  br {cond}, {then_bb}, {else_bb}"),
            Terminator::Return { value } => match value {
                Some(r) => write!(f, "  ret {r}"),
                None => write!(f, "  ret"),
            },
            Terminator::Unreachable => write!(f, "  unreachable"),
        }
    }
}

impl fmt::Display for IrFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let params: Vec<_> = self
            .params
            .iter()
            .map(|(n, t, r)| format!("{r}: {t} /* {n} */"))
            .collect();
        writeln!(
            f,
            "fn {}({}) -> {} {{",
            self.name,
            params.join(", "),
            self.return_ty
        )?;
        for bb in &self.blocks {
            writeln!(f, "{}:", bb.id)?;
            for inst in &bb.insts {
                writeln!(f, "{inst}")?;
            }
            writeln!(f, "{}", bb.term)?;
        }
        writeln!(f, "}}")
    }
}

impl fmt::Display for IrModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for func in &self.functions {
            writeln!(f, "{func}")?;
        }
        Ok(())
    }
}

pub fn dump_ir(module: &IrModule) -> String {
    let mut s = String::new();
    let _ = write!(s, "{module}");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::sema::analyze;
    use crate::span::FileId;

    #[test]
    fn match_lowers_to_tag_load_and_branch_chain() {
        let src = "
            enum E { A(i32), B }
            fn main() -> i32 {
                let e = E::A(4);
                match e {
                    E::A(x) => { return x; }
                    E::B => { return 0; }
                }
            }";
        let (toks, _) = crate::lexer::tokenize(crate::span::FileId(0), src);
        let (prog, _) = crate::parser::parse(toks);
        let (hir, diags) = crate::sema::analyze(&prog);
        assert!(!diags.has_errors(), "{:?}", diags);
        let ir = emit_ir(&hir.unwrap());
        let main = ir.function("main").unwrap();
        let insts: Vec<&Inst> = main.blocks.iter().flat_map(|bb| bb.insts.iter()).collect();
        // enum literal: alloc + tag store at field 0 + payload at field 1
        assert!(insts.iter().any(|i| matches!(i, Inst::AllocStruct { ty: Type::Enum { .. }, .. })));
        assert!(insts.iter().any(|i| matches!(i, Inst::FieldStore { index: 0, .. })));
        assert!(insts.iter().any(|i| matches!(i, Inst::FieldStore { index: 1, .. })));
        // match: tag load, i32 compare, payload binding load
        assert!(insts.iter().any(|i| matches!(i, Inst::FieldLoad { index: 0, ty: Type::I32, .. })));
        assert!(insts.iter().any(|i| matches!(i, Inst::Bin { op: BinOp::Eq, ty: Type::I32, .. })));
        assert!(insts.iter().any(|i| matches!(i, Inst::FieldLoad { index: 1, ty: Type::I32, .. })));
        let branches = main
            .blocks
            .iter()
            .filter(|bb| matches!(bb.term, Terminator::Branch { .. }))
            .count();
        assert!(branches >= 2, "{}", dump_ir(&ir));
        assert!(crate::ir::verify::verify_module(&ir).is_ok(), "{}", dump_ir(&ir));
    }

    #[test]
    fn tuple_equality_lowers_elementwise() {
        let src = "fn main() -> i32 { let a = (1, 2.5); let b = (1, 2.5); if a == b { return 1; } return 0; }";
        let (toks, _) = crate::lexer::tokenize(crate::span::FileId(0), src);
        let (prog, _) = crate::parser::parse(toks);
        let (hir, diags) = crate::sema::analyze(&prog);
        assert!(!diags.has_errors(), "{:?}", diags);
        let ir = emit_ir(&hir.unwrap());
        let main = ir.function("main").unwrap();
        let insts: Vec<&Inst> = main.blocks.iter().flat_map(|bb| bb.insts.iter()).collect();
        // no `==` at the tuple type: one i32 compare and one f64 compare instead
        assert!(!insts.iter().any(|i| matches!(i, Inst::Bin { ty: Type::Tuple(_), .. })));
        assert!(insts.iter().any(|i| matches!(i, Inst::Bin { op: BinOp::Eq, ty: Type::I32, .. })));
        assert!(insts.iter().any(|i| matches!(i, Inst::Bin { op: BinOp::Eq, ty: Type::F64, .. })));
    }

    #[test]
    fn lowers_add() {
        let src = "fn main() -> i32 { return 1 + 2; }";
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let (hir, _) = analyze(&prog);
        let ir = emit_ir(&hir.unwrap());
        let text = dump_ir(&ir);
        assert!(text.contains("+.i32") || text.contains("add"));
        assert!(text.contains("ret"));
    }
}
