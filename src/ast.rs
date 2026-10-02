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
    Enum(EnumDecl),
    Extern(ExternDecl),
    /// `use "path";` — a file import, resolved by the driver before sema.
    Use(UseDecl),
}

/// `use "relative/path.ae";` (`.ae` may be omitted). `path` is the literal
/// as written; the driver resolves it relative to the importing file.
#[derive(Debug, Clone, PartialEq)]
pub struct UseDecl {
    pub path: String,
    pub span: Span,
}

impl Item {
    pub fn span(&self) -> Span {
        match self {
            Item::Fn(f) => f.span,
            Item::Struct(s) => s.span,
            Item::Enum(e) => e.span,
            Item::Extern(e) => e.span,
            Item::Use(u) => u.span,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Item::Fn(f) => &f.name.name,
            Item::Struct(s) => &s.name.name,
            Item::Enum(e) => &e.name.name,
            Item::Extern(e) => &e.name.name,
            Item::Use(u) => &u.path,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    /// `pub` was written. Recorded only; visibility is not enforced in 0.3.
    pub is_pub: bool,
    pub name: Ident,
    pub params: Vec<Param>,
    pub return_ty: TypeExpr,
    pub body: Option<Block>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternDecl {
    pub is_pub: bool,
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
    pub is_pub: bool,
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

/// `enum Name { Variant(T1, T2), Unit }` — variants carry 0..n positional
/// payloads.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub name: Ident,
    pub variants: Vec<VariantDecl>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariantDecl {
    pub name: Ident,
    pub payload: Vec<TypeExpr>,
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
    /// `let (a, b) = t;` — tuple destructuring; sema desugars it into a
    /// temporary plus one `let` per element.
    LetTuple {
        mutable: bool,
        names: Vec<Ident>,
        init: Expr,
        span: Span,
    },
    /// `match e { Pattern => Block ... }`; `if let` is parsed into this.
    Match {
        scrutinee: Expr,
        arms: Vec<MatchArm>,
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
    /// Suspends a budgeted VM run (`Vm::run_budget`); a no-op otherwise.
    Yield {
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
            | Stmt::LetTuple { span, .. }
            | Stmt::Match { span, .. }
            | Stmt::Assign { span, .. }
            | Stmt::Expr { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::If { span, .. }
            | Stmt::While { span, .. }
            | Stmt::For { span, .. }
            | Stmt::Break { span }
            | Stmt::Continue { span }
            | Stmt::Yield { span }
            | Stmt::Block { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PatternKind {
    /// `_`
    Wildcard,
    /// `name` — binds the scrutinee (or payload slot) to a fresh local.
    Binding(Ident),
    /// `1`, `-1`, `true`, `'c'`, `"s"`
    Literal(Literal),
    /// `Enum::Variant(p1, p2)` / `Enum::Variant`
    Variant {
        enum_name: Ident,
        variant: Ident,
        fields: Vec<Pattern>,
    },
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
    /// `(a, b, ...)` with at least two elements (`()` is `Literal::Unit`).
    Tuple {
        elements: Vec<Expr>,
    },
    /// `Enum::Variant(args)` / `Enum::Variant`
    EnumLit {
        enum_name: Ident,
        variant: Ident,
        args: Vec<Expr>,
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
    /// Integer-only bitwise operators; `Shl`/`Shr` take a shift amount of
    /// the left operand's type and mask it to the bit width (see
    /// `docs/language.md`).
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
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
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
        }
    }

    pub fn is_logical(self) -> bool {
        matches!(self, BinOp::And | BinOp::Or)
    }

    pub fn is_bitwise(self) -> bool {
        matches!(
            self,
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr
        )
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
    /// `(T1, T2, ...)` with at least two elements.
    Tuple(Vec<TypeExpr>),
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
        Item::Enum(e) => {
            out.push_str(&format!("{}enum {} {{\n", indent(n), e.name.name));
            for v in &e.variants {
                out.push_str(&format!("{}{},\n", indent(n + 1), variant_str(v)));
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
        Item::Use(u) => {
            out.push_str(&format!("{}use {:?};\n", indent(n), u.path));
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
        Stmt::LetTuple {
            mutable, names, init, ..
        } => {
            let names: Vec<_> = names.iter().map(|n| n.name.clone()).collect();
            out.push_str(&format!(
                "{}let {}({}) = {};\n",
                indent(n),
                if *mutable { "mut " } else { "" },
                names.join(", "),
                expr_str(init)
            ));
        }
        Stmt::Match { scrutinee, arms, .. } => {
            out.push_str(&format!("{}match {} {{\n", indent(n), expr_str(scrutinee)));
            for arm in arms {
                out.push_str(&format!("{}{} => ", indent(n + 1), pattern_str(&arm.pattern)));
                dump_block(&arm.body, n + 1, out);
            }
            out.push_str(&format!("{}}}\n", indent(n)));
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
        Stmt::Yield { .. } => out.push_str(&format!("{}yield;\n", indent(n))),
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
        ExprKind::Tuple { elements } => {
            let e: Vec<_> = elements.iter().map(expr_str).collect();
            format!("({})", e.join(", "))
        }
        ExprKind::EnumLit {
            enum_name,
            variant,
            args,
        } => {
            let a: Vec<_> = args.iter().map(expr_str).collect();
            if a.is_empty() {
                format!("{}::{}", enum_name.name, variant.name)
            } else {
                format!("{}::{}({})", enum_name.name, variant.name, a.join(", "))
            }
        }
        ExprKind::Cast { expr, ty } => format!("({} as {})", expr_str(expr), type_str(ty)),
        ExprKind::Group(e) => format!("({})", expr_str(e)),
    }
}

/// Source form of a pattern (shared by `dump-ast` and the pretty-printer).
pub fn pattern_str(p: &Pattern) -> String {
    match &p.kind {
        PatternKind::Wildcard => "_".into(),
        PatternKind::Binding(id) => id.name.clone(),
        PatternKind::Literal(lit) => literal_str(lit),
        PatternKind::Variant {
            enum_name,
            variant,
            fields,
        } => {
            if fields.is_empty() {
                format!("{}::{}", enum_name.name, variant.name)
            } else {
                let f: Vec<_> = fields.iter().map(pattern_str).collect();
                format!("{}::{}({})", enum_name.name, variant.name, f.join(", "))
            }
        }
    }
}

/// Source form of a literal.
pub fn literal_str(lit: &Literal) -> String {
    match lit {
        Literal::Int(v) => v.to_string(),
        Literal::Float(v) => format!("{v}"),
        Literal::Bool(v) => v.to_string(),
        Literal::String(s) => format!("{s:?}"),
        Literal::Char(c) => format!("{c:?}"),
        Literal::Unit => "()".into(),
    }
}

/// Source form of an enum variant declaration: `Circle(f64)` / `Empty`.
pub fn variant_str(v: &VariantDecl) -> String {
    if v.payload.is_empty() {
        v.name.name.clone()
    } else {
        let p: Vec<_> = v.payload.iter().map(type_str).collect();
        format!("{}({})", v.name.name, p.join(", "))
    }
}

fn type_str(ty: &TypeExpr) -> String {
    match &ty.kind {
        TypeExprKind::Named(n) => n.clone(),
        TypeExprKind::Array { elem, len } => format!("[{}; {len}]", type_str(elem)),
        TypeExprKind::Tuple(elems) => {
            let e: Vec<_> = elems.iter().map(type_str).collect();
            format!("({})", e.join(", "))
        }
        TypeExprKind::Unit => "unit".into(),
    }
}
