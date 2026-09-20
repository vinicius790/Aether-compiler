//! Aether Intermediate Representation.
//!
//! Three-address code with explicit basic blocks. Not SSA: locals occupy
//! stable virtual registers so lowering to the register VM is a 1:1 map.
//! The optimizer rewrites this IR in place.

use crate::sema::{HirBlock, HirExpr, HirExprKind, HirFn, HirProgram, HirStmt};
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
            Inst::IndexStore { .. } | Inst::FieldStore { .. } | Inst::Nop => None,
        }
    }

    pub fn uses(&self) -> Vec<Reg> {
        match self {
            Inst::LoadConst { .. } | Inst::AllocArray { .. } | Inst::AllocStruct { .. } | Inst::Nop => {
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
            | Inst::FieldStore { .. } => false,
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
        if let Some(bb) = self.blocks.iter_mut().find(|b| b.id == self.current) {
            bb.insts.push(inst);
        }
    }

    fn set_term(&mut self, term: Terminator) {
        if let Some(bb) = self.blocks.iter_mut().find(|b| b.id == self.current) {
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
        lower_block(&mut b, body, None, None);
        // implicit return of unit / default
        match &b.blocks.iter().find(|bb| bb.id == b.current).map(|bb| &bb.term) {
            Some(Terminator::Unreachable) | Some(Terminator::Jump { .. }) | None => {
                if f.return_ty == Type::Unit {
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

fn lower_block(
    b: &mut Builder,
    block: &HirBlock,
    break_bb: Option<BlockId>,
    continue_bb: Option<BlockId>,
) {
    let mark = b.locals.len();
    for stmt in &block.stmts {
        lower_stmt(b, stmt, break_bb, continue_bb);
    }
    if let Some(tail) = &block.tail {
        let _ = lower_expr(b, tail);
    }
    b.unbind_to(mark);
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
                b.blocks.iter().find(|bb| bb.id == b.current).map(|bb| &bb.term),
                Some(Terminator::Unreachable)
            ) {
                b.set_term(Terminator::Jump { target: join });
            }
            b.switch(else_bb);
            if let Some(eb) = else_block {
                lower_block(b, eb, break_bb, continue_bb);
            }
            if matches!(
                b.blocks.iter().find(|bb| bb.id == b.current).map(|bb| &bb.term),
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
                b.blocks.iter().find(|bb| bb.id == b.current).map(|bb| &bb.term),
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
            let limit = lower_expr(b, end);
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
                b.blocks.iter().find(|bb| bb.id == b.current).map(|bb| &bb.term),
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
        }
        HirStmt::Break(_) => {
            if let Some(t) = break_bb {
                b.set_term(Terminator::Jump { target: t });
                let dead = b.new_block();
                b.switch(dead);
            }
        }
        HirStmt::Continue(_) => {
            if let Some(t) = continue_bb {
                b.set_term(Terminator::Jump { target: t });
                let dead = b.new_block();
                b.switch(dead);
            }
        }
        HirStmt::Block(block) => lower_block(b, block, break_bb, continue_bb),
    }
}

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
        }
        HirExprKind::Field { base, index, .. } => {
            let br = lower_expr(b, base);
            b.emit(Inst::FieldStore {
                base: br,
                index: *index,
                value: src,
                ty: target.ty.clone(),
            });
        }
        _ => {}
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
        HirExprKind::Binary { op, lhs, rhs } => {
            let l = lower_expr(b, lhs);
            let r = lower_expr(b, rhs);
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
            let regs: Vec<Reg> = args.iter().map(|a| lower_expr(b, a)).collect();
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
            let br = lower_expr(b, base);
            let ir = lower_expr(b, index);
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
            for (i, (_, e)) in fields.iter().enumerate() {
                let v = lower_expr(b, e);
                b.emit(Inst::FieldStore {
                    base: dest,
                    index: i,
                    value: v,
                    ty: e.ty.clone(),
                });
            }
            dest
        }
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
