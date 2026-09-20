//! Semantic analysis: name resolution, type checking, control-flow checks.

use crate::ast::*;
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::span::Span;
use crate::ty::{binop_result, parse_named_type, unop_result, Type};

mod scope;
pub use scope::{DefKind, ScopeStack, Symbol};

#[derive(Debug, Clone)]
pub struct HirProgram {
    pub functions: Vec<HirFn>,
    pub structs: Vec<HirStruct>,
}

#[derive(Debug, Clone)]
pub struct HirStruct {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirFn {
    pub name: String,
    pub params: Vec<(String, Type)>,
    pub return_ty: Type,
    pub body: Option<HirBlock>,
    pub is_extern: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct HirBlock {
    pub stmts: Vec<HirStmt>,
    pub tail: Option<HirExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum HirStmt {
    Let {
        name: String,
        ty: Type,
        mutable: bool,
        init: Option<HirExpr>,
        span: Span,
    },
    Assign {
        target: HirExpr,
        value: HirExpr,
        span: Span,
    },
    Expr(HirExpr),
    Return {
        value: Option<HirExpr>,
        span: Span,
    },
    If {
        cond: HirExpr,
        then_block: HirBlock,
        else_block: Option<HirBlock>,
        span: Span,
    },
    While {
        cond: HirExpr,
        body: HirBlock,
        span: Span,
    },
    For {
        var: String,
        start: HirExpr,
        end: HirExpr,
        body: HirBlock,
        span: Span,
    },
    Break(Span),
    Continue(Span),
    Block(HirBlock),
}

#[derive(Debug, Clone)]
pub struct HirExpr {
    pub kind: HirExprKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum HirExprKind {
    Literal(Literal),
    Local(String),
    Binary {
        op: BinOp,
        lhs: Box<HirExpr>,
        rhs: Box<HirExpr>,
    },
    Unary {
        op: UnOp,
        expr: Box<HirExpr>,
    },
    Call {
        name: String,
        args: Vec<HirExpr>,
    },
    Index {
        base: Box<HirExpr>,
        index: Box<HirExpr>,
    },
    Field {
        base: Box<HirExpr>,
        field: String,
        index: usize,
    },
    Array {
        elements: Vec<HirExpr>,
    },
    StructLit {
        name: String,
        fields: Vec<(String, HirExpr)>,
    },
    Cast {
        expr: Box<HirExpr>,
        to: Type,
    },
}

pub struct Analyzer<'a> {
    program: &'a Program,
    diags: Diagnostics,
    scopes: ScopeStack,
    structs: Vec<HirStruct>,
    functions: Vec<(String, Type, Span, bool)>, // name, fn type, span, is_extern
    current_return: Type,
    loop_depth: usize,
}

impl<'a> Analyzer<'a> {
    pub fn new(program: &'a Program) -> Self {
        Analyzer {
            program,
            diags: Diagnostics::new(),
            scopes: ScopeStack::new(),
            structs: Vec::new(),
            functions: Vec::new(),
            current_return: Type::Unit,
            loop_depth: 0,
        }
    }

    pub fn analyze(mut self) -> (Option<HirProgram>, Diagnostics) {
        self.collect_items();
        if self.diags.has_errors() {
            return (None, self.diags);
        }
        let mut functions = Vec::new();
        for item in &self.program.items {
            match item {
                Item::Fn(f) => {
                    if let Some(hf) = self.check_fn(f, false) {
                        functions.push(hf);
                    }
                }
                Item::Extern(e) => {
                    let dummy = FnDecl {
                        name: e.name.clone(),
                        params: e.params.clone(),
                        return_ty: e.return_ty.clone(),
                        body: None,
                        span: e.span,
                    };
                    if let Some(hf) = self.check_fn(&dummy, true) {
                        functions.push(hf);
                    }
                }
                Item::Struct(_) => {}
            }
        }
        self.check_entry_point(&functions);
        if self.diags.has_errors() {
            (None, self.diags)
        } else {
            (
                Some(HirProgram {
                    functions,
                    structs: self.structs.clone(),
                }),
                self.diags,
            )
        }
    }

    fn collect_items(&mut self) {
        // structs first so function signatures can refer to them
        for item in &self.program.items {
            if let Item::Struct(s) = item {
                if self.structs.iter().any(|h| h.name == s.name.name) {
                    self.err(
                        format!("duplicate struct `{}`", s.name.name),
                        s.name.span,
                        "E0201",
                    );
                    continue;
                }
                let mut fields = Vec::new();
                for f in &s.fields {
                    if fields.iter().any(|(n, _)| n == &f.name.name) {
                        self.err(
                            format!("duplicate field `{}`", f.name.name),
                            f.name.span,
                            "E0202",
                        );
                        continue;
                    }
                    let ty = self.resolve_type(&f.ty);
                    fields.push((f.name.name.clone(), ty));
                }
                let ty = Type::Struct {
                    name: s.name.name.clone(),
                    fields,
                };
                self.structs.push(HirStruct {
                    name: s.name.name.clone(),
                    ty,
                    span: s.span,
                });
            }
        }

        for item in &self.program.items {
            match item {
                Item::Fn(f) => self.register_fn(&f.name, &f.params, &f.return_ty, f.span, false),
                Item::Extern(e) => {
                    self.register_fn(&e.name, &e.params, &e.return_ty, e.span, true)
                }
                Item::Struct(_) => {}
            }
        }

        // built-ins
        self.register_builtin(
            "print",
            Type::Fn {
                params: vec![Type::String],
                ret: Box::new(Type::Unit),
            },
        );
        self.register_builtin(
            "print_i32",
            Type::Fn {
                params: vec![Type::I32],
                ret: Box::new(Type::Unit),
            },
        );
        self.register_builtin(
            "print_i64",
            Type::Fn {
                params: vec![Type::I64],
                ret: Box::new(Type::Unit),
            },
        );
        self.register_builtin(
            "print_f64",
            Type::Fn {
                params: vec![Type::F64],
                ret: Box::new(Type::Unit),
            },
        );
        self.register_builtin(
            "print_bool",
            Type::Fn {
                params: vec![Type::Bool],
                ret: Box::new(Type::Unit),
            },
        );
        self.register_builtin(
            "println",
            Type::Fn {
                params: vec![Type::String],
                ret: Box::new(Type::Unit),
            },
        );
        self.register_builtin(
            "len",
            Type::Fn {
                params: vec![Type::String],
                ret: Box::new(Type::I32),
            },
        );
        self.register_builtin(
            "assert",
            Type::Fn {
                params: vec![Type::Bool],
                ret: Box::new(Type::Unit),
            },
        );
    }

    fn register_builtin(&mut self, name: &str, ty: Type) {
        if self.functions.iter().any(|(n, _, _, _)| n == name) {
            return;
        }
        self.functions
            .push((name.to_string(), ty, Span::DUMMY, true));
    }

    fn register_fn(
        &mut self,
        name: &Ident,
        params: &[Param],
        return_ty: &TypeExpr,
        span: Span,
        is_extern: bool,
    ) {
        if self.functions.iter().any(|(n, _, _, _)| n == &name.name) {
            self.err(format!("duplicate function `{}`", name.name), name.span, "E0203");
            return;
        }
        let param_tys: Vec<Type> = params.iter().map(|p| self.resolve_type(&p.ty)).collect();
        let ret = self.resolve_type(return_ty);
        self.functions.push((
            name.name.clone(),
            Type::Fn {
                params: param_tys,
                ret: Box::new(ret),
            },
            span,
            is_extern,
        ));
    }

    fn check_entry_point(&mut self, functions: &[HirFn]) {
        match functions.iter().find(|f| f.name == "main") {
            None => {
                self.err(
                    "program is missing an entry point `fn main()`",
                    Span::DUMMY,
                    "E0210",
                );
            }
            Some(m) => {
                if !m.params.is_empty() {
                    self.err("`main` must not take parameters", m.span, "E0211");
                }
                if m.return_ty != Type::I32 && m.return_ty != Type::Unit {
                    self.err("`main` must return `i32` or `unit`", m.span, "E0212");
                }
            }
        }
    }

    fn check_fn(&mut self, f: &FnDecl, is_extern: bool) -> Option<HirFn> {
        let params: Vec<(String, Type)> = f
            .params
            .iter()
            .map(|p| (p.name.name.clone(), self.resolve_type(&p.ty)))
            .collect();
        let return_ty = self.resolve_type(&f.return_ty);
        self.current_return = return_ty.clone();
        let body = if let Some(body) = &f.body {
            self.scopes.push();
            for (name, ty) in &params {
                self.scopes.define(
                    name.clone(),
                    Symbol {
                        name: name.clone(),
                        ty: ty.clone(),
                        mutable: false,
                        kind: DefKind::Local,
                        span: f.span,
                    },
                );
            }
            let hb = self.check_block(body);
            self.scopes.pop();
            if return_ty != Type::Unit && !block_always_returns(&hb) {
                self.err(
                    format!("function `{}` may not return a value of type `{return_ty}` on all paths", f.name.name),
                    f.span,
                    "E0220",
                );
            }
            Some(hb)
        } else {
            None
        };
        Some(HirFn {
            name: f.name.name.clone(),
            params,
            return_ty,
            body,
            is_extern,
            span: f.span,
        })
    }

    fn check_block(&mut self, block: &Block) -> HirBlock {
        self.scopes.push();
        let mut stmts = Vec::new();
        for s in &block.stmts {
            stmts.push(self.check_stmt(s));
        }
        let tail = block.tail.as_ref().map(|e| self.check_expr(e, None));
        self.scopes.pop();
        HirBlock {
            stmts,
            tail,
            span: block.span,
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt) -> HirStmt {
        match stmt {
            Stmt::Let {
                mutable,
                name,
                ty,
                init,
                span,
            } => {
                let hint = ty.as_ref().map(|t| self.resolve_type(t));
                let init_e = init.as_ref().map(|e| self.check_expr(e, hint.as_ref()));
                let inferred = init_e
                    .as_ref()
                    .map(|e| e.ty.clone())
                    .or_else(|| hint.clone());
                let final_ty = match (hint, inferred) {
                    (Some(t), Some(i)) => {
                        if !t.assignable_from(&i) {
                            self.err(
                                format!("cannot assign `{i}` to variable of type `{t}`"),
                                *span,
                                "E0230",
                            );
                            Type::Error
                        } else {
                            t
                        }
                    }
                    (Some(t), None) => t,
                    (None, Some(i)) => i,
                    (None, None) => {
                        self.err(
                            format!("variable `{}` needs a type annotation or initializer", name.name),
                            name.span,
                            "E0231",
                        );
                        Type::Error
                    }
                };
                if self.scopes.define(
                    name.name.clone(),
                    Symbol {
                        name: name.name.clone(),
                        ty: final_ty.clone(),
                        mutable: *mutable,
                        kind: DefKind::Local,
                        span: name.span,
                    },
                ) == false
                {
                    self.diags.push(
                        Diagnostic::warning(
                            format!("shadows existing binding `{}`", name.name),
                            name.span,
                        )
                        .with_code("W0232"),
                    );
                }
                HirStmt::Let {
                    name: name.name.clone(),
                    ty: final_ty,
                    mutable: *mutable,
                    init: init_e,
                    span: *span,
                }
            }
            Stmt::Assign {
                target,
                value,
                span,
            } => {
                let t = self.check_expr(target, None);
                self.check_lvalue(&t);
                let v = self.check_expr(value, Some(&t.ty));
                if !t.ty.assignable_from(&v.ty) {
                    self.err(
                        format!("cannot assign `{}` to `{}`", v.ty, t.ty),
                        *span,
                        "E0233",
                    );
                }
                HirStmt::Assign {
                    target: t,
                    value: v,
                    span: *span,
                }
            }
            Stmt::Expr { expr, .. } => HirStmt::Expr(self.check_expr(expr, None)),
            Stmt::Return { value, span } => {
                let expected = self.current_return.clone();
                let v = value.as_ref().map(|e| self.check_expr(e, Some(&expected)));
                match (&v, &self.current_return) {
                    (None, Type::Unit) => {}
                    (None, t) => {
                        self.err(
                            format!("missing return value (expected `{t}`)"),
                            *span,
                            "E0234",
                        );
                    }
                    (Some(e), t) => {
                        if !t.assignable_from(&e.ty) {
                            self.err(
                                format!("returning `{}` from function of type `{t}`", e.ty),
                                *span,
                                "E0235",
                            );
                        }
                    }
                }
                HirStmt::Return { value: v, span: *span }
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                span,
            } => {
                let c = self.check_expr(cond, Some(&Type::Bool));
                if c.ty != Type::Bool && !c.ty.is_error() {
                    self.err(
                        format!("condition has type `{}`, expected `bool`", c.ty),
                        cond.span,
                        "E0236",
                    );
                }
                let then_b = self.check_block(then_block);
                let else_b = else_block.as_ref().map(|b| self.check_block(b));
                HirStmt::If {
                    cond: c,
                    then_block: then_b,
                    else_block: else_b,
                    span: *span,
                }
            }
            Stmt::While { cond, body, span } => {
                let c = self.check_expr(cond, Some(&Type::Bool));
                if c.ty != Type::Bool && !c.ty.is_error() {
                    self.err(
                        format!("while-condition has type `{}`, expected `bool`", c.ty),
                        cond.span,
                        "E0237",
                    );
                }
                self.loop_depth += 1;
                let body = self.check_block(body);
                self.loop_depth -= 1;
                HirStmt::While {
                    cond: c,
                    body,
                    span: *span,
                }
            }
            Stmt::For {
                var,
                start,
                end,
                body,
                span,
            } => {
                let s = self.check_expr(start, Some(&Type::I32));
                let e = self.check_expr(end, Some(&Type::I32));
                if !s.ty.is_integer() && !s.ty.is_error() {
                    self.err("for-range start must be an integer", start.span, "E0238");
                }
                if !e.ty.is_integer() && !e.ty.is_error() {
                    self.err("for-range end must be an integer", end.span, "E0238");
                }
                self.scopes.push();
                self.scopes.define(
                    var.name.clone(),
                    Symbol {
                        name: var.name.clone(),
                        ty: Type::I32,
                        mutable: true,
                        kind: DefKind::Local,
                        span: var.span,
                    },
                );
                self.loop_depth += 1;
                let body = self.check_block(body);
                self.loop_depth -= 1;
                self.scopes.pop();
                HirStmt::For {
                    var: var.name.clone(),
                    start: s,
                    end: e,
                    body,
                    span: *span,
                }
            }
            Stmt::Break { span } => {
                if self.loop_depth == 0 {
                    self.err("`break` outside of a loop", *span, "E0239");
                }
                HirStmt::Break(*span)
            }
            Stmt::Continue { span } => {
                if self.loop_depth == 0 {
                    self.err("`continue` outside of a loop", *span, "E0240");
                }
                HirStmt::Continue(*span)
            }
            Stmt::Block { block, .. } => HirStmt::Block(self.check_block(block)),
        }
    }

    fn check_lvalue(&mut self, expr: &HirExpr) {
        match &expr.kind {
            HirExprKind::Local(name) => {
                if let Some(sym) = self.scopes.lookup(name) {
                    if !sym.mutable {
                        self.err(
                            format!("cannot assign to immutable binding `{name}`"),
                            expr.span,
                            "E0241",
                        );
                    }
                }
            }
            HirExprKind::Index { .. } | HirExprKind::Field { .. } => {}
            _ => {
                self.err("invalid assignment target", expr.span, "E0242");
            }
        }
    }

    fn check_expr(&mut self, expr: &Expr, expected: Option<&Type>) -> HirExpr {
        let mut hir = match &expr.kind {
            ExprKind::Literal(lit) => {
                let ty = match lit {
                    Literal::Int(_) => expected
                        .cloned()
                        .filter(|t| t.is_integer())
                        .unwrap_or(Type::I32),
                    Literal::Float(_) => Type::F64,
                    Literal::Bool(_) => Type::Bool,
                    Literal::String(_) => Type::String,
                    Literal::Char(_) => Type::Char,
                    Literal::Unit => Type::Unit,
                };
                HirExpr {
                    kind: HirExprKind::Literal(lit.clone()),
                    ty,
                    span: expr.span,
                }
            }
            ExprKind::Ident(id) => {
                if let Some(sym) = self.scopes.lookup(&id.name) {
                    HirExpr {
                        kind: HirExprKind::Local(id.name.clone()),
                        ty: sym.ty.clone(),
                        span: expr.span,
                    }
                } else if let Some((_, ty, _, _)) =
                    self.functions.iter().find(|(n, _, _, _)| n == &id.name)
                {
                    HirExpr {
                        kind: HirExprKind::Local(id.name.clone()),
                        ty: ty.clone(),
                        span: expr.span,
                    }
                } else {
                    self.err(
                        format!("cannot find value `{0}` in this scope", id.name),
                        id.span,
                        "E0243",
                    );
                    HirExpr {
                        kind: HirExprKind::Local(id.name.clone()),
                        ty: Type::Error,
                        span: expr.span,
                    }
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.check_expr(lhs, None);
                let r = self.check_expr(rhs, Some(&l.ty));
                let ty = match binop_result(*op, &l.ty, &r.ty) {
                    Some(t) => t,
                    None => {
                        self.err(
                            format!("operator `{op}` is not defined for `{}` and `{}`", l.ty, r.ty),
                            expr.span,
                            "E0244",
                        );
                        Type::Error
                    }
                };
                HirExpr {
                    kind: HirExprKind::Binary {
                        op: *op,
                        lhs: Box::new(l),
                        rhs: Box::new(r),
                    },
                    ty,
                    span: expr.span,
                }
            }
            ExprKind::Unary { op, expr: inner } => {
                let e = self.check_expr(inner, None);
                let ty = match unop_result(*op, &e.ty) {
                    Some(t) => t,
                    None => {
                        self.err(
                            format!("unary `{}` is not defined for `{}`", op.as_str(), e.ty),
                            expr.span,
                            "E0245",
                        );
                        Type::Error
                    }
                };
                HirExpr {
                    kind: HirExprKind::Unary {
                        op: *op,
                        expr: Box::new(e),
                    },
                    ty,
                    span: expr.span,
                }
            }
            ExprKind::Call { callee, args } => self.check_call(callee, args, expr.span),
            ExprKind::Index { base, index } => {
                let b = self.check_expr(base, None);
                let i = self.check_expr(index, Some(&Type::I32));
                if !i.ty.is_integer() && !i.ty.is_error() {
                    self.err("array index must be an integer", index.span, "E0246");
                }
                let ty = match &b.ty {
                    Type::Array { elem, .. } => *elem.clone(),
                    Type::String => Type::Char,
                    Type::Error => Type::Error,
                    other => {
                        self.err(format!("cannot index into `{other}`"), base.span, "E0247");
                        Type::Error
                    }
                };
                HirExpr {
                    kind: HirExprKind::Index {
                        base: Box::new(b),
                        index: Box::new(i),
                    },
                    ty,
                    span: expr.span,
                }
            }
            ExprKind::Field { base, field } => {
                let b = self.check_expr(base, None);
                match b.ty.field(&field.name) {
                    Some((idx, fty)) => HirExpr {
                        ty: fty.clone(),
                        kind: HirExprKind::Field {
                            base: Box::new(b),
                            field: field.name.clone(),
                            index: idx,
                        },
                        span: expr.span,
                    },
                    None => {
                        if !b.ty.is_error() {
                            self.err(
                                format!("no field `{}` on type `{}`", field.name, b.ty),
                                field.span,
                                "E0248",
                            );
                        }
                        HirExpr {
                            kind: HirExprKind::Field {
                                base: Box::new(b),
                                field: field.name.clone(),
                                index: 0,
                            },
                            ty: Type::Error,
                            span: expr.span,
                        }
                    }
                }
            }
            ExprKind::Array { elements } => {
                let mut checked = Vec::new();
                let mut elem_ty = Type::Error;
                for (i, e) in elements.iter().enumerate() {
                    let hint = if i == 0 { None } else { Some(&elem_ty) };
                    let c = self.check_expr(e, hint);
                    if i == 0 {
                        elem_ty = c.ty.clone();
                    } else if !elem_ty.assignable_from(&c.ty) {
                        self.err(
                            format!("array element has type `{}`, expected `{elem_ty}`", c.ty),
                            e.span,
                            "E0249",
                        );
                    }
                    checked.push(c);
                }
                if elements.is_empty() {
                    self.err("cannot infer type of empty array", expr.span, "E0250");
                }
                HirExpr {
                    ty: Type::Array {
                        elem: Box::new(elem_ty),
                        len: elements.len() as i64,
                    },
                    kind: HirExprKind::Array { elements: checked },
                    span: expr.span,
                }
            }
            ExprKind::StructLit { name, fields } => {
                let sty = self.lookup_struct(&name.name);
                match sty {
                    Some(Type::Struct {
                        name: sname,
                        fields: decl_fields,
                    }) => {
                        let mut out_fields = Vec::new();
                        for (fname, fexpr) in fields {
                            match decl_fields.iter().find(|(n, _)| n == &fname.name) {
                                Some((_, fty)) => {
                                    let e = self.check_expr(fexpr, Some(fty));
                                    if !fty.assignable_from(&e.ty) {
                                        self.err(
                                            format!(
                                                "field `{}` has type `{fty}`, found `{}`",
                                                fname.name, e.ty
                                            ),
                                            fexpr.span,
                                            "E0251",
                                        );
                                    }
                                    out_fields.push((fname.name.clone(), e));
                                }
                                None => {
                                    self.err(
                                        format!("struct `{sname}` has no field `{}`", fname.name),
                                        fname.span,
                                        "E0252",
                                    );
                                }
                            }
                        }
                        for (n, _) in &decl_fields {
                            if !out_fields.iter().any(|(fnm, _)| fnm == n) {
                                self.err(
                                    format!("missing field `{n}` in `{sname}` literal"),
                                    expr.span,
                                    "E0253",
                                );
                            }
                        }
                        HirExpr {
                            ty: Type::Struct {
                                name: sname,
                                fields: decl_fields,
                            },
                            kind: HirExprKind::StructLit {
                                name: name.name.clone(),
                                fields: out_fields,
                            },
                            span: expr.span,
                        }
                    }
                    _ => {
                        self.err(
                            format!("unknown struct `{}`", name.name),
                            name.span,
                            "E0254",
                        );
                        HirExpr {
                            kind: HirExprKind::StructLit {
                                name: name.name.clone(),
                                fields: Vec::new(),
                            },
                            ty: Type::Error,
                            span: expr.span,
                        }
                    }
                }
            }
            ExprKind::Cast { expr: inner, ty } => {
                let to = self.resolve_type(ty);
                let e = self.check_expr(inner, None);
                if !e.ty.can_cast_to(&to) && !e.ty.is_error() {
                    self.err(
                        format!("cannot cast `{}` to `{to}`", e.ty),
                        expr.span,
                        "E0255",
                    );
                }
                HirExpr {
                    kind: HirExprKind::Cast {
                        expr: Box::new(e),
                        to: to.clone(),
                    },
                    ty: to,
                    span: expr.span,
                }
            }
            ExprKind::Group(inner) => self.check_expr(inner, expected),
        };
        if let Some(exp) = expected {
            if hir.ty == Type::I32 && *exp == Type::I64 {
                if let HirExprKind::Literal(Literal::Int(_)) = &hir.kind {
                    hir.ty = Type::I64;
                }
            }
        }
        hir
    }

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> HirExpr {
        let name = match &callee.kind {
            ExprKind::Ident(id) => id.name.clone(),
            _ => {
                self.err(
                    "calling computed function values is not supported",
                    callee.span,
                    "E0256",
                );
                return HirExpr {
                    kind: HirExprKind::Call {
                        name: "<invalid>".into(),
                        args: Vec::new(),
                    },
                    ty: Type::Error,
                    span,
                };
            }
        };
        let fty = self
            .functions
            .iter()
            .find(|(n, _, _, _)| n == &name)
            .map(|(_, t, _, _)| t.clone());
        match fty {
            Some(Type::Fn { params, ret }) => {
                if params.len() != args.len() {
                    self.err(
                        format!(
                            "function `{name}` takes {} argument(s), found {}",
                            params.len(),
                            args.len()
                        ),
                        span,
                        "E0257",
                    );
                }
                let mut checked = Vec::new();
                for (i, arg) in args.iter().enumerate() {
                    let hint = params.get(i);
                    let e = self.check_expr(arg, hint);
                    if let Some(pty) = hint {
                        if !pty.assignable_from(&e.ty) {
                            self.err(
                                format!(
                                    "argument {} to `{name}` has type `{}`, expected `{pty}`",
                                    i + 1,
                                    e.ty
                                ),
                                arg.span,
                                "E0258",
                            );
                        }
                    }
                    checked.push(e);
                }
                HirExpr {
                    ty: *ret,
                    kind: HirExprKind::Call {
                        name,
                        args: checked,
                    },
                    span,
                }
            }
            Some(_) => {
                self.err(format!("`{name}` is not a function"), callee.span, "E0259");
                HirExpr {
                    kind: HirExprKind::Call {
                        name,
                        args: Vec::new(),
                    },
                    ty: Type::Error,
                    span,
                }
            }
            None => {
                self.err(format!("unknown function `{name}`"), callee.span, "E0260");
                HirExpr {
                    kind: HirExprKind::Call {
                        name,
                        args: Vec::new(),
                    },
                    ty: Type::Error,
                    span,
                }
            }
        }
    }

    fn resolve_type(&mut self, te: &TypeExpr) -> Type {
        match &te.kind {
            TypeExprKind::Named(n) => {
                if let Some(t) = parse_named_type(n) {
                    t
                } else if let Some(s) = self.structs.iter().find(|s| s.name == *n) {
                    s.ty.clone()
                } else {
                    self.err(format!("unknown type `{n}`"), te.span, "E0261");
                    Type::Error
                }
            }
            TypeExprKind::Array { elem, len } => {
                if *len < 0 {
                    self.err("array length must be non-negative", te.span, "E0262");
                }
                Type::Array {
                    elem: Box::new(self.resolve_type(elem)),
                    len: *len,
                }
            }
            TypeExprKind::Unit => Type::Unit,
        }
    }

    fn lookup_struct(&self, name: &str) -> Option<Type> {
        self.structs
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.ty.clone())
    }

    fn err(&mut self, message: impl Into<String>, span: Span, code: &'static str) {
        self.diags
            .push(Diagnostic::error(message, span).with_code(code));
    }
}

fn block_always_returns(block: &HirBlock) -> bool {
    if block.tail.is_some() {
        return true;
    }
    block.stmts.iter().any(stmt_always_returns)
}

fn stmt_always_returns(stmt: &HirStmt) -> bool {
    match stmt {
        HirStmt::Return { .. } => true,
        HirStmt::If {
            then_block,
            else_block,
            ..
        } => {
            block_always_returns(then_block)
                && else_block
                    .as_ref()
                    .map(block_always_returns)
                    .unwrap_or(false)
        }
        HirStmt::Block(b) => block_always_returns(b),
        _ => false,
    }
}

pub fn analyze(program: &Program) -> (Option<HirProgram>, Diagnostics) {
    Analyzer::new(program).analyze()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::span::Session;

    fn sema(src: &str) -> (Option<HirProgram>, String) {
        let mut sess = Session::new();
        let id = sess.add_file("t.ae".into(), src.into());
        let (toks, mut diags) = tokenize(id, src);
        let (prog, pdiags) = parse(toks);
        diags.extend(pdiags);
        let (hir, sdiags) = analyze(&prog);
        diags.extend(sdiags);
        (hir, diags.render(&sess, false))
    }

    #[test]
    fn accepts_well_typed_program() {
        let src = r#"
            fn add(a: i32, b: i32) -> i32 { return a + b; }
            fn main() -> i32 {
                let x = add(1, 2);
                return x;
            }
        "#;
        let (hir, msg) = sema(src);
        assert!(hir.is_some(), "{msg}");
    }

    #[test]
    fn rejects_type_mismatch() {
        let src = r#"
            fn main() -> i32 {
                let x: i32 = true;
                return 0;
            }
        "#;
        let (hir, msg) = sema(src);
        assert!(hir.is_none());
        assert!(msg.contains("cannot assign"));
    }

    #[test]
    fn rejects_unknown_name() {
        let src = "fn main() -> i32 { return missing; }";
        let (_, msg) = sema(src);
        assert!(msg.contains("cannot find value"));
    }
}
