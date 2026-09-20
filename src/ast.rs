//! Abstract syntax tree. Backend-agnostic and owned.

use crate::span::Span;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Fn(FnDecl),
    Struct(StructDecl),
    Extern(ExternDecl),
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Fn(f) => f.span,
            Item::Struct(s) => s.span,
            Item::Extern(e) => e.span,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Item::Fn(f) => &f.name.name,
            Item::Struct(s) => &s.name.name,
            Item::Extern(e) => &e.name.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_ty: TypeExpr,
    pub body: Option<Block>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDecl {
    pub name: Ident,
    pub fields: Vec<FieldDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Let {
        mutable: bool,
        name: Ident,
        ty: Option<TypeExpr>,
        init: Option<Expr>,
        span: Span,
    },
    Assign {
        target: Expr,
        value: Expr,
        span: Span,
    },
    Expr {
        expr: Expr,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    If {
        cond: Expr,
        then_block: Block,
        else_block: Option<Block>,
        span: Span,
    },
    While {
        cond: Expr,
        body: Block,
        span: Span,
    },
    For {
        var: Ident,
        start: Expr,
        end: Expr,
        body: Block,
        span: Span,
    },
    Break {
        span: Span,
    },
    Continue {
        span: Span,
    },
    Block {
        block: Block,
        span: Span,
    },
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let { span, .. }
            | Stmt::Assign { span, .. }
            | Stmt::Expr { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::If { span, .. }
            | Stmt::While { span, .. }
            | Stmt::For { span, .. }
            | Stmt::Break { span }
            | Stmt::Continue { span }
            | Stmt::Block { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Literal(Literal),
    Ident(Ident),
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Field {
        base: Box<Expr>,
        field: Ident,
    },
    Array {
        elements: Vec<Expr>,
    },
    StructLit {
        name: Ident,
        fields: Vec<(Ident, Expr)>,
    },
    Cast {
        expr: Box<Expr>,
        ty: TypeExpr,
    },
    Group(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Char(char),
    Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }

    pub fn is_logical(self) -> bool {
        matches!(self, BinOp::And | BinOp::Or)
    }

    pub fn is_cmp(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    Neg,
    Not,
}

impl UnOp {
    pub fn as_str(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Not => "!",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

impl Ident {
    pub fn new(name: impl Into<String>, span: Span) -> Self {
        Ident {
            name: name.into(),
            span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeExprKind {
    Named(String),
    Array {
        elem: Box<TypeExpr>,
        len: i64,
    },
    Unit,
}

impl TypeExpr {
    pub fn named(name: impl Into<String>, span: Span) -> Self {
        TypeExpr {
            kind: TypeExprKind::Named(name.into()),
            span,
        }
    }

    pub fn unit(span: Span) -> Self {
        TypeExpr {
            kind: TypeExprKind::Unit,
            span,
        }
    }
}

/// Pretty-print an AST for `dump-ast`.
pub fn dump_program(program: &Program) -> String {
    let mut out = String::new();
    for item in &program.items {
        dump_item(item, 0, &mut out);
    }
    out
}

fn indent(n: usize) -> String {
    "  ".repeat(n)
}

fn dump_item(item: &Item, n: usize, out: &mut String) {
    match item {
        Item::Fn(f) => {
            out.push_str(&format!(
                "{}fn {}(",
                indent(n),
                f.name.name
            ));
            for (i, p) in f.params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&format!("{}: {}", p.name.name, type_str(&p.ty)));
            }
            out.push_str(&format!(") -> {} ", type_str(&f.return_ty)));
            if let Some(body) = &f.body {
                dump_block(body, n, out);
            } else {
                out.push_str(";\n");
            }
        }
        Item::Struct(s) => {
            out.push_str(&format!("{}struct {} {{\n", indent(n), s.name.name));
            for field in &s.fields {
                out.push_str(&format!(
                    "{}{}: {},\n",
                    indent(n + 1),
                    field.name.name,
                    type_str(&field.ty)
                ));
            }
            out.push_str(&format!("{}}}\n", indent(n)));
        }
        Item::Extern(e) => {
            out.push_str(&format!(
                "{}extern fn {}(...) -> {};\n",
                indent(n),
                e.name.name,
                type_str(&e.return_ty)
            ));
        }
    }
}

fn dump_block(block: &Block, n: usize, out: &mut String) {
    out.push_str("{\n");
    for stmt in &block.stmts {
        dump_stmt(stmt, n + 1, out);
    }
    if let Some(tail) = &block.tail {
        out.push_str(&format!("{}{}\n", indent(n + 1), expr_str(tail)));
    }
    out.push_str(&format!("{}}}\n", indent(n)));
}

fn dump_stmt(stmt: &Stmt, n: usize, out: &mut String) {
    match stmt {
        Stmt::Let {
            mutable,
            name,
            ty,
            init,
            ..
        } => {
            out.push_str(&format!(
                "{}let {}{}",
                indent(n),
                if *mutable { "mut " } else { "" },
                name.name
            ));
            if let Some(ty) = ty {
                out.push_str(&format!(": {}", type_str(ty)));
            }
            if let Some(init) = init {
                out.push_str(&format!(" = {}", expr_str(init)));
            }
            out.push_str(";\n");
        }
        Stmt::Assign { target, value, .. } => {
            out.push_str(&format!(
                "{}{} = {};\n",
                indent(n),
                expr_str(target),
                expr_str(value)
            ));
        }
        Stmt::Expr { expr, .. } => {
            out.push_str(&format!("{}{};\n", indent(n), expr_str(expr)));
        }
        Stmt::Return { value, .. } => {
            if let Some(v) = value {
                out.push_str(&format!("{}return {};\n", indent(n), expr_str(v)));
            } else {
                out.push_str(&format!("{}return;\n", indent(n)));
            }
        }
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            out.push_str(&format!("{}if {} ", indent(n), expr_str(cond)));
            dump_block(then_block, n, out);
            if let Some(e) = else_block {
                out.push_str(&format!("{}else ", indent(n)));
                dump_block(e, n, out);
            }
        }
        Stmt::While { cond, body, .. } => {
            out.push_str(&format!("{}while {} ", indent(n), expr_str(cond)));
            dump_block(body, n, out);
        }
        Stmt::For {
            var,
            start,
            end,
            body,
            ..
        } => {
            out.push_str(&format!(
                "{}for {} in {}..{} ",
                indent(n),
                var.name,
                expr_str(start),
                expr_str(end)
            ));
            dump_block(body, n, out);
        }
        Stmt::Break { .. } => out.push_str(&format!("{}break;\n", indent(n))),
        Stmt::Continue { .. } => out.push_str(&format!("{}continue;\n", indent(n))),
        Stmt::Block { block, .. } => {
            out.push_str(&indent(n));
            dump_block(block, n, out);
        }
    }
}

fn expr_str(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Literal(Literal::Int(v)) => v.to_string(),
        ExprKind::Literal(Literal::Float(v)) => format!("{v}"),
        ExprKind::Literal(Literal::Bool(v)) => v.to_string(),
        ExprKind::Literal(Literal::String(s)) => format!("{s:?}"),
        ExprKind::Literal(Literal::Char(c)) => format!("{c:?}"),
        ExprKind::Literal(Literal::Unit) => "()".into(),
        ExprKind::Ident(id) => id.name.clone(),
        ExprKind::Binary { op, lhs, rhs } => {
            format!("({} {} {})", expr_str(lhs), op, expr_str(rhs))
        }
        ExprKind::Unary { op, expr } => format!("({}{})", op.as_str(), expr_str(expr)),
        ExprKind::Call { callee, args } => {
            let a: Vec<_> = args.iter().map(expr_str).collect();
            format!("{}({})", expr_str(callee), a.join(", "))
        }
        ExprKind::Index { base, index } => format!("{}[{}]", expr_str(base), expr_str(index)),
        ExprKind::Field { base, field } => format!("{}.{}", expr_str(base), field.name),
        ExprKind::Array { elements } => {
            let e: Vec<_> = elements.iter().map(expr_str).collect();
            format!("[{}]", e.join(", "))
        }
        ExprKind::StructLit { name, fields } => {
            let f: Vec<_> = fields
                .iter()
                .map(|(n, e)| format!("{}: {}", n.name, expr_str(e)))
                .collect();
            format!("{} {{ {} }}", name.name, f.join(", "))
        }
        ExprKind::Cast { expr, ty } => format!("({} as {})", expr_str(expr), type_str(ty)),
        ExprKind::Group(e) => format!("({})", expr_str(e)),
    }
}

fn type_str(ty: &TypeExpr) -> String {
    match &ty.kind {
        TypeExprKind::Named(n) => n.clone(),
        TypeExprKind::Array { elem, len } => format!("[{}; {len}]", type_str(elem)),
        TypeExprKind::Unit => "unit".into(),
    }
}
