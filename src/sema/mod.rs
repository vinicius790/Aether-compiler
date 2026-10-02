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

/// A named aggregate type: `ty` is `Type::Struct` or `Type::Enum`.
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
    /// Exhaustive by construction (sema checks it); arms are tried in order.
    Match {
        scrutinee: HirExpr,
        arms: Vec<HirArm>,
        span: Span,
    },
    Break(Span),
    Continue(Span),
    Yield(Span),
    Block(HirBlock),
}

#[derive(Debug, Clone)]
pub struct HirArm {
    pub pattern: HirPattern,
    pub body: HirBlock,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum HirPattern {
    Wildcard,
    /// Binds the whole scrutinee.
    Binding { name: String, ty: Type },
    /// Compared with `==` at the scrutinee's type.
    Literal { lit: Literal, ty: Type },
    /// `fields[i]` binds payload slot `i + 1`, or is `None` for `_`.
    Variant {
        tag: usize,
        fields: Vec<Option<(String, Type)>>,
    },
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
    /// Laid out like a struct with fields `0`, `1`, ...
    Tuple {
        elements: Vec<HirExpr>,
    },
    /// Field 0 is the tag, fields `1..=args.len()` the payload.
    EnumLit {
        tag: usize,
        args: Vec<HirExpr>,
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
    /// Aggregates being resolved on demand (cycle detection).
    resolving: Vec<String>,
    /// Counter for compiler-generated locals (`$tN`), which cannot clash
    /// with source identifiers.
    temp_counter: usize,
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
            resolving: Vec::new(),
            temp_counter: 0,
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
                        is_pub: e.is_pub,
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
                // imports are resolved by the driver before sema
                Item::Struct(_) | Item::Use(_) | Item::Enum(_) => {}
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
        // structs and enums first so function signatures can refer to them;
        // they are resolved on demand so fields may name types declared later
        let program = self.program;
        let mut seen: Vec<&str> = Vec::new();
        for item in &program.items {
            let (name, kind) = match item {
                Item::Struct(s) => (&s.name, "struct"),
                Item::Enum(e) => (&e.name, "enum"),
                _ => continue,
            };
            if seen.contains(&name.name.as_str()) {
                self.err(format!("duplicate {kind} `{}`", name.name), name.span, "E0201");
            } else {
                seen.push(name.name.as_str());
            }
        }
        for name in seen {
            self.resolve_aggregate(name);
        }

        for item in &program.items {
            match item {
                Item::Fn(f) => self.register_fn(&f.name, &f.params, &f.return_ty, f.span, false),
                Item::Extern(e) => {
                    self.register_fn(&e.name, &e.params, &e.return_ty, e.span, true)
                }
                Item::Struct(_) | Item::Use(_) | Item::Enum(_) => {}
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
        // runtime::NATIVES ids 8.. (see the table there)
        use Type::{Char, F64, I32, I64};
        self.register_builtin("print_char", fn_ty(vec![Char], Type::Unit));
        self.register_builtin("to_string", fn_ty(vec![I32], Type::String));
        self.register_builtin("i64_to_string", fn_ty(vec![I64], Type::String));
        self.register_builtin("f64_to_string", fn_ty(vec![F64], Type::String));
        self.register_builtin("char_to_string", fn_ty(vec![Char], Type::String));
        self.register_builtin("abs", fn_ty(vec![I32], I32));
        self.register_builtin("min", fn_ty(vec![I32, I32], I32));
        self.register_builtin("max", fn_ty(vec![I32, I32], I32));
        self.register_builtin("clamp", fn_ty(vec![I32, I32, I32], I32));
        self.register_builtin("sqrt", fn_ty(vec![F64], F64));
        self.register_builtin("floor", fn_ty(vec![F64], F64));
        self.register_builtin("ceil", fn_ty(vec![F64], F64));
        self.register_builtin("pow_i32", fn_ty(vec![I32, I32], I32));
    }

    /// Resolves the struct or enum `name` (first declaration wins), caching
    /// it in `self.structs`. Returns `None` for an unknown name or a type
    /// that contains itself.
    fn resolve_aggregate(&mut self, name: &str) -> Option<Type> {
        if let Some(s) = self.structs.iter().find(|s| s.name == name) {
            return Some(s.ty.clone());
        }
        if self.resolving.iter().any(|n| n == name) {
            return None;
        }
        let program = self.program;
        let item = program.items.iter().find(|it| {
            matches!(it, Item::Struct(_) | Item::Enum(_)) && it.name() == name
        })?;
        self.resolving.push(name.to_string());
        let ty = match item {
            Item::Struct(s) => {
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
                Type::Struct {
                    name: s.name.name.clone(),
                    fields,
                }
            }
            Item::Enum(e) => {
                let mut variants: Vec<(String, Vec<Type>)> = Vec::new();
                for v in &e.variants {
                    if variants.iter().any(|(n, _)| n == &v.name.name) {
                        self.err(
                            format!("duplicate variant `{}`", v.name.name),
                            v.name.span,
                            "E0204",
                        );
                        continue;
                    }
                    let payload = v.payload.iter().map(|t| self.resolve_type(t)).collect();
                    variants.push((v.name.name.clone(), payload));
                }
                if variants.is_empty() {
                    self.err(
                        format!("enum `{}` has no variants", e.name.name),
                        e.name.span,
                        "E0206",
                    );
                }
                Type::Enum {
                    name: e.name.name.clone(),
                    variants,
                }
            }
            _ => unreachable!(),
        };
        self.resolving.pop();
        self.structs.push(HirStruct {
            name: name.to_string(),
            ty: ty.clone(),
            span: item.span(),
        });
        Some(ty)
    }

    /// Built-ins are registered after user functions, so a user `fn` with
    /// the same name shadows the built-in (e.g. `stdlib/math.ae` defines its
    /// own `abs`/`min`/`max`/`clamp`).
    fn register_builtin(&mut self, name: &str, ty: Type) {
        if self.functions.iter().any(|(n, _, _, _)| n == name) {
            return;
        }
        self.functions
            .push((name.to_string(), ty, Span::DUMMY, true));
    }

    /// `name` resolves to a built-in (not shadowed by a user function).
    fn is_builtin(&self, name: &str) -> bool {
        self.functions
            .iter()
            .any(|(n, _, span, is_extern)| n == name && *is_extern && *span == Span::DUMMY)
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
            let hb = self.check_block_with(body, Some(&return_ty));
            self.scopes.pop();
            if let Some(tail) = &hb.tail {
                if !return_ty.assignable_from(&tail.ty) {
                    self.err(
                        format!(
                            "tail expression has type `{}`, but `{}` returns `{return_ty}`",
                            tail.ty, f.name.name
                        ),
                        tail.span,
                        "E0221",
                    );
                }
            }
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

    /// A nested block: a trailing expression without `;` is evaluated as a
    /// statement. Only a function body (`check_block_with`) keeps it as a tail.
    fn check_block(&mut self, block: &Block) -> HirBlock {
        self.check_block_with(block, None)
    }

    fn check_block_with(&mut self, block: &Block, tail_expected: Option<&Type>) -> HirBlock {
        self.scopes.push();
        let mut stmts = Vec::new();
        for s in &block.stmts {
            if let Stmt::LetTuple {
                mutable,
                names,
                init,
                span,
            } = s
            {
                self.check_let_tuple(*mutable, names, init, *span, &mut stmts);
            } else {
                stmts.push(self.check_stmt(s));
            }
        }
        let mut tail = block
            .tail
            .as_ref()
            .map(|e| self.check_expr(e, tail_expected));
        if tail_expected.is_none() {
            if let Some(e) = tail.take() {
                stmts.push(HirStmt::Expr(e));
            }
        }
        self.scopes.pop();
        HirBlock {
            stmts,
            tail,
            span: block.span,
        }
    }

    /// `let (a, b) = t;` → `let $t = t; let a = $t.0; let b = $t.1;`
    fn check_let_tuple(
        &mut self,
        mutable: bool,
        names: &[Ident],
        init: &Expr,
        span: Span,
        out: &mut Vec<HirStmt>,
    ) {
        let init_e = self.check_expr(init, None);
        let elems: Vec<Type> = match &init_e.ty {
            Type::Tuple(elems) if elems.len() == names.len() => elems.clone(),
            Type::Error => vec![Type::Error; names.len()],
            other => {
                self.err(
                    format!(
                        "cannot destructure `{other}` into {} names",
                        names.len()
                    ),
                    init.span,
                    "E0269",
                );
                vec![Type::Error; names.len()]
            }
        };
        let temp = format!("$t{}", self.temp_counter);
        self.temp_counter += 1;
        let tuple_ty = init_e.ty.clone();
        out.push(HirStmt::Let {
            name: temp.clone(),
            ty: tuple_ty.clone(),
            mutable: false,
            init: Some(init_e),
            span,
        });
        for (i, name) in names.iter().enumerate() {
            if name.name == "_" {
                continue;
            }
            let ty = elems[i].clone();
            self.define_local(name, ty.clone(), mutable);
            let base = HirExpr {
                kind: HirExprKind::Local(temp.clone()),
                ty: tuple_ty.clone(),
                span: init.span,
            };
            out.push(HirStmt::Let {
                name: name.name.clone(),
                ty: ty.clone(),
                mutable,
                init: Some(HirExpr {
                    kind: HirExprKind::Field {
                        base: Box::new(base),
                        field: i.to_string(),
                        index: i,
                    },
                    ty,
                    span: name.span,
                }),
                span: name.span,
            });
        }
    }

    /// Defines a local, warning when it shadows an existing binding.
    fn define_local(&mut self, name: &Ident, ty: Type, mutable: bool) {
        if !self.scopes.define(
            name.name.clone(),
            Symbol {
                name: name.name.clone(),
                ty,
                mutable,
                kind: DefKind::Local,
                span: name.span,
            },
        ) {
            self.diags.push(
                Diagnostic::warning(
                    format!("shadows existing binding `{}`", name.name),
                    name.span,
                )
                .with_code("W0232"),
            );
        }
    }

    fn check_match(&mut self, scrutinee: &Expr, arms: &[MatchArm], span: Span) -> HirStmt {
        let s = self.check_expr(scrutinee, None);
        let sty = s.ty.clone();
        let mut seen_tags: Vec<usize> = Vec::new();
        let mut catch_all = false;
        let mut out = Vec::new();
        for arm in arms {
            self.scopes.push();
            let pattern = self.check_pattern(&arm.pattern, &sty);
            match &pattern {
                HirPattern::Variant { tag, .. } => {
                    if seen_tags.contains(tag) {
                        self.err(
                            "duplicate match arm: this variant is already covered",
                            arm.pattern.span,
                            "E0271",
                        );
                    } else {
                        seen_tags.push(*tag);
                    }
                }
                HirPattern::Wildcard | HirPattern::Binding { .. } => catch_all = true,
                HirPattern::Literal { .. } => {}
            }
            let body = self.check_block(&arm.body);
            self.scopes.pop();
            out.push(HirArm {
                pattern,
                body,
                span: arm.span,
            });
        }
        if !catch_all {
            match &sty {
                Type::Enum { variants, .. } => {
                    let missing: Vec<&str> = variants
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !seen_tags.contains(i))
                        .map(|(_, (n, _))| n.as_str())
                        .collect();
                    if !missing.is_empty() {
                        self.diags.push(
                            Diagnostic::error(
                                format!(
                                    "non-exhaustive match: variant(s) `{}` not covered",
                                    missing.join("`, `")
                                ),
                                span,
                            )
                            .with_code("E0270")
                            .with_help("add the missing arms or a `_ => { ... }` arm"),
                        );
                    }
                }
                Type::Error => {}
                _ => {
                    self.diags.push(
                        Diagnostic::error(
                            format!("non-exhaustive match on `{sty}`"),
                            span,
                        )
                        .with_code("E0270")
                        .with_help("a match on a scalar needs a `_` or binding arm"),
                    );
                }
            }
        }
        HirStmt::Match {
            scrutinee: s,
            arms: out,
            span,
        }
    }

    /// Checks a pattern against the scrutinee type and defines its bindings
    /// in the current scope.
    fn check_pattern(&mut self, pat: &Pattern, sty: &Type) -> HirPattern {
        match &pat.kind {
            PatternKind::Wildcard => HirPattern::Wildcard,
            PatternKind::Binding(id) => {
                self.define_local(id, sty.clone(), false);
                HirPattern::Binding {
                    name: id.name.clone(),
                    ty: sty.clone(),
                }
            }
            PatternKind::Literal(lit) => {
                let lit_ty = match lit {
                    Literal::Int(v) => {
                        if sty.is_integer() || sty.is_error() {
                            self.int_literal(*v, Some(sty), pat.span).ty
                        } else {
                            Type::I32
                        }
                    }
                    Literal::Float(_) => {
                        self.err(
                            "float literals cannot be used as patterns",
                            pat.span,
                            "E0268",
                        );
                        return HirPattern::Wildcard;
                    }
                    Literal::Bool(_) => Type::Bool,
                    Literal::String(_) => Type::String,
                    Literal::Char(_) => Type::Char,
                    Literal::Unit => Type::Unit,
                };
                if !sty.assignable_from(&lit_ty) {
                    self.err(
                        format!("pattern has type `{lit_ty}`, but the value matched has type `{sty}`"),
                        pat.span,
                        "E0269",
                    );
                }
                HirPattern::Literal {
                    lit: lit.clone(),
                    ty: sty.clone(),
                }
            }
            PatternKind::Variant {
                enum_name,
                variant,
                fields,
            } => {
                let ety = match self.resolve_aggregate(&enum_name.name) {
                    Some(t @ Type::Enum { .. }) => t,
                    _ => {
                        self.err(
                            format!("unknown enum `{}`", enum_name.name),
                            enum_name.span,
                            "E0265",
                        );
                        return HirPattern::Wildcard;
                    }
                };
                if !sty.assignable_from(&ety) {
                    self.err(
                        format!("pattern has type `{ety}`, but the value matched has type `{sty}`"),
                        pat.span,
                        "E0269",
                    );
                }
                let (tag, payload) = match ety.variant(&variant.name) {
                    Some((tag, p)) => (tag, p.to_vec()),
                    None => {
                        self.err(
                            format!("enum `{}` has no variant `{}`", enum_name.name, variant.name),
                            variant.span,
                            "E0266",
                        );
                        return HirPattern::Wildcard;
                    }
                };
                if fields.len() != payload.len() {
                    self.err(
                        format!(
                            "variant `{}::{}` has {} payload value(s), but the pattern names {}",
                            enum_name.name,
                            variant.name,
                            payload.len(),
                            fields.len()
                        ),
                        pat.span,
                        "E0267",
                    );
                }
                let mut out = Vec::new();
                for (i, f) in fields.iter().enumerate() {
                    let fty = payload.get(i).cloned().unwrap_or(Type::Error);
                    match &f.kind {
                        PatternKind::Wildcard => out.push(None),
                        PatternKind::Binding(id) => {
                            self.define_local(id, fty.clone(), false);
                            out.push(Some((id.name.clone(), fty)));
                        }
                        _ => {
                            self.err(
                                "nested patterns are not supported; use a name or `_`",
                                f.span,
                                "E0268",
                            );
                            out.push(None);
                        }
                    }
                }
                out.truncate(payload.len());
                HirPattern::Variant { tag, fields: out }
            }
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
                if s.ty != Type::I32 && !s.ty.is_error() {
                    self.err(
                        format!("for-range start must be `i32`, found `{}`", s.ty),
                        start.span,
                        "E0238",
                    );
                }
                if e.ty != Type::I32 && !e.ty.is_error() {
                    self.err(
                        format!("for-range end must be `i32`, found `{}`", e.ty),
                        end.span,
                        "E0238",
                    );
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
            Stmt::Yield { span } => HirStmt::Yield(*span),
            Stmt::Continue { span } => {
                if self.loop_depth == 0 {
                    self.err("`continue` outside of a loop", *span, "E0240");
                }
                HirStmt::Continue(*span)
            }
            Stmt::Block { block, .. } => HirStmt::Block(self.check_block(block)),
            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => self.check_match(scrutinee, arms, *span),
            // Normally expanded by `check_block_with`; a stray one (not
            // directly in a block) is scoped to itself.
            Stmt::LetTuple {
                mutable,
                names,
                init,
                span,
            } => {
                let mut stmts = Vec::new();
                self.check_let_tuple(*mutable, names, init, *span, &mut stmts);
                HirStmt::Block(HirBlock {
                    stmts,
                    tail: None,
                    span: *span,
                })
            }
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

    /// `len(e)` accepts a `string` or any `[T; N]` and yields `i32`. The
    /// registered signature only says `string`, so the call is special-cased
    /// here unless a user function named `len` shadows the built-in.
    fn check_len_call(&mut self, args: &[Expr], span: Span) -> HirExpr {
        if args.len() != 1 {
            self.err(
                format!("function `len` takes 1 argument(s), found {}", args.len()),
                span,
                "E0257",
            );
        }
        let checked: Vec<HirExpr> = args.iter().map(|a| self.check_expr(a, None)).collect();
        if let Some(arg) = checked.first() {
            if !matches!(arg.ty, Type::String | Type::Array { .. } | Type::Error) {
                self.err(
                    format!(
                        "argument 1 to `len` has type `{}`, expected `string` or an array",
                        arg.ty
                    ),
                    args[0].span,
                    "E0258",
                );
            }
        }
        HirExpr {
            kind: HirExprKind::Call {
                name: "len".into(),
                args: checked,
            },
            ty: Type::I32,
            span,
        }
    }

    /// An integer literal takes the expected integer type (`i32` by default)
    /// and must fit in it.
    fn int_literal(&mut self, value: i64, expected: Option<&Type>, span: Span) -> HirExpr {
        let ty = expected
            .cloned()
            .filter(|t| t.is_integer())
            .unwrap_or(Type::I32);
        if ty == Type::I32 && i32::try_from(value).is_err() {
            self.err(
                format!("integer literal `{value}` is out of range for `i32`"),
                span,
                "E0263",
            );
        }
        HirExpr {
            kind: HirExprKind::Literal(Literal::Int(value)),
            ty,
            span,
        }
    }

    // `check_expr` recurses once per nesting level, so it is kept to a thin
    // dispatcher: debug builds reserve stack for every arm of a `match` up
    // front, and with the arms inlined its frame was several KiB, which
    // overflowed a 2 MiB thread at ~200 nested parentheses. Each `check_*`
    // helper owns its own locals instead.

    fn check_expr(&mut self, expr: &Expr, expected: Option<&Type>) -> HirExpr {
        match &expr.kind {
            ExprKind::Literal(Literal::Int(v)) => self.int_literal(*v, expected, expr.span),
            ExprKind::Literal(lit) => self.check_literal(lit, expr.span),
            ExprKind::Ident(id) => self.check_ident(id, expr.span),
            ExprKind::Binary { op, lhs, rhs } => self.check_binary(*op, lhs, rhs, expected, expr.span),
            ExprKind::Unary { op, expr: inner } => self.check_unary(*op, inner, expected, expr.span),
            ExprKind::Call { callee, args } => self.check_call(callee, args, expr.span),
            ExprKind::Index { base, index } => self.check_index(base, index, expr.span),
            ExprKind::Field { base, field } => self.check_field(base, field, expr.span),
            ExprKind::Array { elements } => self.check_array(elements, expr.span),
            ExprKind::StructLit { name, fields } => self.check_struct_lit(name, fields, expr.span),
            ExprKind::Tuple { elements } => self.check_tuple(elements, expected, expr.span),
            ExprKind::EnumLit {
                enum_name,
                variant,
                args,
            } => self.check_enum_lit(enum_name, variant, args, expr.span),
            ExprKind::Cast { expr: inner, ty } => self.check_cast(inner, ty, expr.span),
            ExprKind::Group(inner) => self.check_expr(inner, expected),
        }
    }

    fn check_literal(&mut self, lit: &Literal, span: Span) -> HirExpr {
        let ty = match lit {
            Literal::Int(_) => Type::I32, // handled by `int_literal`
            Literal::Float(_) => Type::F64,
            Literal::Bool(_) => Type::Bool,
            Literal::String(_) => Type::String,
            Literal::Char(_) => Type::Char,
            Literal::Unit => Type::Unit,
        };
        HirExpr {
            kind: HirExprKind::Literal(lit.clone()),
            ty,
            span,
        }
    }

    fn check_ident(&mut self, id: &Ident, span: Span) -> HirExpr {
        let ty = if let Some(sym) = self.scopes.lookup(&id.name) {
            sym.ty.clone()
        } else {
            if self.functions.iter().any(|(n, _, _, _)| n == &id.name) {
                self.err(
                    format!("function `{}` cannot be used as a value", id.name),
                    id.span,
                    "E0264",
                );
            } else {
                self.err(
                    format!("cannot find value `{0}` in this scope", id.name),
                    id.span,
                    "E0243",
                );
            }
            Type::Error
        };
        HirExpr {
            kind: HirExprKind::Local(id.name.clone()),
            ty,
            span,
        }
    }

    fn check_binary(
        &mut self,
        op: BinOp,
        lhs: &Expr,
        rhs: &Expr,
        expected: Option<&Type>,
        span: Span,
    ) -> HirExpr {
        // Arithmetic inherits the expected numeric type; a bare integer
        // literal on either side adopts the type of the other operand.
        let hint = expected.filter(|t| t.is_numeric() && !op.is_cmp() && !op.is_logical());
        let (l, r) = if is_int_literal_expr(lhs) && !is_int_literal_expr(rhs) {
            let r = self.check_expr(rhs, hint);
            let l = self.check_expr(lhs, Some(&r.ty));
            (l, r)
        } else {
            let l = self.check_expr(lhs, hint);
            let r = self.check_expr(rhs, Some(&l.ty));
            (l, r)
        };
        let ty = match binop_result(op, &l.ty, &r.ty) {
            Some(t) => t,
            None => {
                self.err(
                    format!("operator `{op}` is not defined for `{}` and `{}`", l.ty, r.ty),
                    span,
                    "E0244",
                );
                Type::Error
            }
        };
        HirExpr {
            kind: HirExprKind::Binary {
                op,
                lhs: Box::new(l),
                rhs: Box::new(r),
            },
            ty,
            span,
        }
    }

    fn check_unary(&mut self, op: UnOp, inner: &Expr, expected: Option<&Type>, span: Span) -> HirExpr {
        // `-5` is one literal, so `let x: i64 = -1;` and `-2147483648` type-check
        if op == UnOp::Neg {
            if let Some(v) = int_literal_value(inner) {
                return self.int_literal(v.wrapping_neg(), expected, span);
            }
        }
        // `-` keeps a numeric hint, `!` an integer one (`let m: i64 = !0;`)
        let hint = expected.filter(|t| match op {
            UnOp::Neg => t.is_numeric(),
            UnOp::Not => t.is_integer(),
        });
        let e = self.check_expr(inner, hint);
        let ty = match unop_result(op, &e.ty) {
            Some(t) => t,
            None => {
                self.err(
                    format!("unary `{}` is not defined for `{}`", op.as_str(), e.ty),
                    span,
                    "E0245",
                );
                Type::Error
            }
        };
        HirExpr {
            kind: HirExprKind::Unary {
                op,
                expr: Box::new(e),
            },
            ty,
            span,
        }
    }

    fn check_index(&mut self, base: &Expr, index: &Expr, span: Span) -> HirExpr {
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
            span,
        }
    }

    fn check_field(&mut self, base: &Expr, field: &Ident, span: Span) -> HirExpr {
        let b = self.check_expr(base, None);
        let (index, ty) = match b.ty.field(&field.name) {
            Some((idx, fty)) => (idx, fty.clone()),
            None => {
                if !b.ty.is_error() {
                    self.err(
                        format!("no field `{}` on type `{}`", field.name, b.ty),
                        field.span,
                        "E0248",
                    );
                }
                (0, Type::Error)
            }
        };
        HirExpr {
            kind: HirExprKind::Field {
                base: Box::new(b),
                field: field.name.clone(),
                index,
            },
            ty,
            span,
        }
    }

    fn check_array(&mut self, elements: &[Expr], span: Span) -> HirExpr {
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
            self.err("cannot infer type of empty array", span, "E0250");
        }
        HirExpr {
            ty: Type::Array {
                elem: Box::new(elem_ty),
                len: elements.len() as i64,
            },
            kind: HirExprKind::Array { elements: checked },
            span,
        }
    }

    fn check_struct_lit(&mut self, name: &Ident, fields: &[(Ident, Expr)], span: Span) -> HirExpr {
        let (sname, decl_fields) = match self.lookup_struct(&name.name) {
            Some(Type::Struct { name, fields }) => (name, fields),
            _ => {
                self.err(
                    format!("unknown struct `{}`", name.name),
                    name.span,
                    "E0254",
                );
                return HirExpr {
                    kind: HirExprKind::StructLit {
                        name: name.name.clone(),
                        fields: Vec::new(),
                    },
                    ty: Type::Error,
                    span,
                };
            }
        };
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
                    span,
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
            span,
        }
    }

    fn check_tuple(&mut self, elements: &[Expr], expected: Option<&Type>, span: Span) -> HirExpr {
        let hints: Vec<Option<&Type>> = match expected {
            Some(Type::Tuple(ts)) if ts.len() == elements.len() => ts.iter().map(Some).collect(),
            _ => vec![None; elements.len()],
        };
        let mut checked = Vec::new();
        let mut tys = Vec::new();
        for (e, hint) in elements.iter().zip(hints) {
            let c = self.check_expr(e, hint);
            tys.push(c.ty.clone());
            checked.push(c);
        }
        HirExpr {
            kind: HirExprKind::Tuple { elements: checked },
            ty: Type::Tuple(tys),
            span,
        }
    }

    fn check_enum_lit(&mut self, enum_name: &Ident, variant: &Ident, args: &[Expr], span: Span) -> HirExpr {
        let error = |args: Vec<HirExpr>| HirExpr {
            kind: HirExprKind::EnumLit { tag: 0, args },
            ty: Type::Error,
            span,
        };
        let ety = match self.resolve_aggregate(&enum_name.name) {
            Some(t @ Type::Enum { .. }) => t,
            _ => {
                self.err(
                    format!("unknown enum `{}`", enum_name.name),
                    enum_name.span,
                    "E0265",
                );
                let args = args.iter().map(|a| self.check_expr(a, None)).collect();
                return error(args);
            }
        };
        let (tag, payload) = match ety.variant(&variant.name) {
            Some((tag, p)) => (tag, p.to_vec()),
            None => {
                self.err(
                    format!("enum `{}` has no variant `{}`", enum_name.name, variant.name),
                    variant.span,
                    "E0266",
                );
                let args = args.iter().map(|a| self.check_expr(a, None)).collect();
                return error(args);
            }
        };
        if args.len() != payload.len() {
            self.err(
                format!(
                    "variant `{}::{}` takes {} value(s), but {} were given",
                    enum_name.name,
                    variant.name,
                    payload.len(),
                    args.len()
                ),
                span,
                "E0267",
            );
        }
        let mut checked = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let want = payload.get(i);
            let c = self.check_expr(a, want);
            if let Some(w) = want {
                if !w.assignable_from(&c.ty) {
                    self.err(
                        format!(
                            "payload {i} of `{}::{}` has type `{w}`, found `{}`",
                            enum_name.name, variant.name, c.ty
                        ),
                        a.span,
                        "E0269",
                    );
                }
            }
            checked.push(c);
        }
        HirExpr {
            kind: HirExprKind::EnumLit { tag, args: checked },
            ty: ety,
            span,
        }
    }

    fn check_cast(&mut self, inner: &Expr, ty: &TypeExpr, span: Span) -> HirExpr {
        let to = self.resolve_type(ty);
        let e = self.check_expr(inner, None);
        if !e.ty.can_cast_to(&to) && !e.ty.is_error() {
            self.err(
                format!("cannot cast `{}` to `{to}`", e.ty),
                span,
                "E0255",
            );
        }
        HirExpr {
            kind: HirExprKind::Cast {
                expr: Box::new(e),
                to: to.clone(),
            },
            ty: to,
            span,
        }
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
        if name == "len" && self.is_builtin("len") {
            return self.check_len_call(args, span);
        }
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
                } else if let Some(t) = self.resolve_aggregate(n) {
                    t
                } else if self.resolving.iter().any(|r| r == n) {
                    self.err(
                        format!("recursive type `{n}` has infinite size"),
                        te.span,
                        "E0205",
                    );
                    Type::Error
                } else {
                    self.err(format!("unknown type `{n}`"), te.span, "E0261");
                    Type::Error
                }
            }
            TypeExprKind::Tuple(elems) => {
                Type::Tuple(elems.iter().map(|t| self.resolve_type(t)).collect())
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

fn fn_ty(params: Vec<Type>, ret: Type) -> Type {
    Type::Fn {
        params,
        ret: Box::new(ret),
    }
}

/// Value of a (possibly negated or parenthesised) integer literal.
fn int_literal_value(e: &Expr) -> Option<i64> {
    match &e.kind {
        ExprKind::Literal(Literal::Int(v)) => Some(*v),
        ExprKind::Group(inner) => int_literal_value(inner),
        ExprKind::Unary {
            op: UnOp::Neg,
            expr,
        } => int_literal_value(expr).map(i64::wrapping_neg),
        _ => None,
    }
}

/// An expression made only of integer literals, parentheses, unary minus or
/// bitwise not, arithmetic and bitwise operators — it has no type of its own
/// and adopts the other operand's.
fn is_int_literal_expr(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Literal(Literal::Int(_)) => true,
        ExprKind::Group(inner) => is_int_literal_expr(inner),
        ExprKind::Unary { expr, .. } => is_int_literal_expr(expr),
        ExprKind::Binary { op, lhs, rhs } if !op.is_cmp() && !op.is_logical() => {
            is_int_literal_expr(lhs) && is_int_literal_expr(rhs)
        }
        _ => false,
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
        HirStmt::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|a| block_always_returns(&a.body))
        }
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
    fn enums_tuples_and_match_type_check() {
        let src = "
            struct P { x: i32 }
            enum E { A(f64), B(P, i64), C }
            fn f(e: E) -> i32 {
                match e {
                    E::A(r) => { return r as i32; }
                    E::B(p, n) => { return p.x + n as i32; }
                    E::C => { return 0; }
                }
            }
            fn main() -> i32 {
                let t: (i32, E) = (1, E::B(P { x: 2 }, 3));
                let (a, e) = t;
                if let E::C = e { return 1; }
                let ok = t == (1, E::C) || e != E::A(1.0);
                match a { 1 => { } x => { } }
                return f(e) + t.0;
            }";
        let (hir, msg) = sema(src);
        assert!(hir.is_some(), "{msg}");
        for (src, code) in [
            ("enum E { A, B } fn main() -> i32 { match E::A { E::A => { } } return 0; }", "E0270"),
            ("fn main() -> i32 { match 1 { 1 => { } } return 0; }", "E0270"),
            ("enum E { A, B } fn main() -> i32 { match E::A { E::A => { } E::A => { } _ => { } } return 0; }", "E0271"),
            ("enum E { A(i32) } fn main() -> i32 { match E::A(1) { E::A => { } } return 0; }", "E0267"),
            ("enum E { A } fn main() -> i32 { let x = E::B; return 0; }", "E0266"),
            ("fn main() -> i32 { let t = (1, 2); return t.5; }", "E0248"),
            ("fn main() -> i32 { let t = (1, 2); let (a, b, c) = t; return 0; }", "E0269"),
            ("enum E { A } fn main() -> i32 { return (E::A < E::A) as i32; }", "E0244"),
            ("struct S { e: E } enum E { A(S) } fn main() -> i32 { return 0; }", "E0205"),
        ] {
            let (hir, msg) = sema(src);
            assert!(hir.is_none() && msg.contains(code), "expected {code} for {src}, got:\n{msg}");
        }
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

    #[test]
    fn user_fn_shadows_builtin() {
        // stdlib/math.ae defines abs/min/max/clamp itself; the user versions win
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/stdlib/math.ae")).unwrap();
        let (hir, msg) = sema(&src);
        let hir = hir.unwrap_or_else(|| panic!("{msg}"));
        assert!(hir.functions.iter().any(|f| f.name == "abs" && !f.is_extern));
        // a shadowing user fn may even change the signature
        let src = r#"
            fn abs(x: f64) -> f64 { if x < 0.0 { return 0.0 - x; } return x; }
            fn len(a: i32) -> i32 { return a; }
            fn main() -> i32 { let y = abs(1.5); return len(3); }
        "#;
        let (hir, msg) = sema(src);
        assert!(hir.is_some(), "{msg}");
        // without the user fn the built-in signature applies
        let (hir, msg) = sema("fn main() -> i32 { return abs(1.5); }");
        assert!(hir.is_none());
        assert!(msg.contains("expected `i32`"), "{msg}");
    }

    #[test]
    fn new_builtins_type_check() {
        let src = r#"
            fn main() -> i32 {
                print_char('a');
                let s = to_string(42) + i64_to_string(7 as i64) + f64_to_string(1.5) + char_to_string('x');
                let a = abs(0 - 3) + min(1, 2) + max(1, 2) + clamp(5, 0, 3) + pow_i32(2, 10);
                let f = sqrt(2.0) + floor(1.5) + ceil(1.5);
                let n: i32 = len(s) + len([1, 2, 3]) + len([[1], [2]]);
                return a + n;
            }
        "#;
        let (hir, msg) = sema(src);
        assert!(hir.is_some(), "{msg}");
        let (_, msg) = sema("fn main() -> i32 { return len(3); }");
        assert!(msg.contains("expected `string` or an array"), "{msg}");
        let (_, msg) = sema("fn main() -> i32 { return len(); }");
        assert!(msg.contains("takes 1 argument"), "{msg}");
    }

    #[test]
    fn bitwise_operators_type_check() {
        let ok = r#"
            fn f(a: i32, b: i32) -> i32 { return (a & b) | (a ^ b) << 1 >> 1 ^ !a; }
            fn g(a: i64) -> i64 { let m: i64 = !0; return a & m | 1 << 3 ^ (a >> 2); }
            fn main() -> i32 {
                let mut x = 0xF0;
                x &= 0x3C; x |= 1; x ^= 2; x <<= 1; x >>= 1; x += 1; x -= 1; x *= 2; x /= 2; x %= 7;
                let b = !true;
                return f(x, 1) + g(1 as i64) as i32;
            }
        "#;
        let (hir, msg) = sema(ok);
        assert!(hir.is_some(), "{msg}");
        for (src, code) in [
            ("fn main() -> i32 { return 1 & true; }", "E0244"),
            ("fn main() -> i32 { return 1.0 ^ 2.0; }", "E0244"),
            ("fn main() -> i32 { let a: i64 = 1; return (a << 1) as i32; }", ""),
            ("fn main() -> i32 { let a: i64 = 1; let b: i32 = 1; return (a << b) as i32; }", "E0244"),
            ("fn main() -> i32 { return (!1.5) as i32; }", "E0245"),
            ("fn main() -> i32 { let s = \"a\"; s += 1; return 0; }", "E0244"),
        ] {
            let (hir, msg) = sema(src);
            if code.is_empty() {
                assert!(hir.is_some(), "{src}: {msg}");
            } else {
                assert!(msg.contains(code), "{src}: {msg}");
            }
        }
    }
}
