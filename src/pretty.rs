//! Source pretty-printer from the AST.

use crate::ast::{
    literal_str, pattern_str, variant_str, Block, Expr, ExprKind, Item, Literal, Program, Stmt,
};

pub fn pretty_program(p: &Program) -> String {
    let mut s = String::new();
    for (i, item) in p.items.iter().enumerate() {
        // consecutive `use` lines stay together; other items are separated
        let uses = matches!(item, Item::Use(_))
            && i > 0
            && matches!(p.items[i - 1], Item::Use(_));
        if i > 0 && !uses {
            s.push('\n');
        }
        s.push_str(&pretty_item(item));
        s.push('\n');
    }
    s
}

fn pretty_item(item: &Item) -> String {
    match item {
        Item::Fn(f) => {
            let params: Vec<String> = f
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name.name, type_str(&p.ty)))
                .collect();
            let ret = type_str(&f.return_ty);
            let body = f
                .body
                .as_ref()
                .map(|b| pretty_block(b, 0))
                .unwrap_or_else(|| ";".into());
            format!(
                "{}fn {}({}) -> {ret} {body}",
                vis(f.is_pub),
                f.name.name,
                params.join(", ")
            )
        }
        Item::Struct(st) => {
            let fields: Vec<String> = st
                .fields
                .iter()
                .map(|f| format!("    {}: {},", f.name.name, type_str(&f.ty)))
                .collect();
            format!(
                "{}struct {} {{\n{}\n}}",
                vis(st.is_pub),
                st.name.name,
                fields.join("\n")
            )
        }
        Item::Enum(en) => {
            let variants: Vec<String> = en
                .variants
                .iter()
                .map(|v| format!("    {},", variant_str(v)))
                .collect();
            format!("enum {} {{\n{}\n}}", en.name.name, variants.join("\n"))
        }
        Item::Extern(e) => {
            let params: Vec<String> = e
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name.name, type_str(&p.ty)))
                .collect();
            format!(
                "{}extern fn {}({}) -> {};",
                vis(e.is_pub),
                e.name.name,
                params.join(", "),
                type_str(&e.return_ty)
            )
        }
        Item::Use(u) => format!("use {:?};", u.path),
    }
}

/// `pub ` when the item was declared public (recorded, not enforced).
fn vis(is_pub: bool) -> &'static str {
    if is_pub {
        "pub "
    } else {
        ""
    }
}

fn pretty_block(b: &Block, indent: usize) -> String {
    let pad = "    ".repeat(indent);
    let inn = "    ".repeat(indent + 1);
    let mut s = String::from("{\n");
    for st in &b.stmts {
        s.push_str(&inn);
        s.push_str(&pretty_stmt(st, indent + 1));
        s.push('\n');
    }
    if let Some(t) = &b.tail {
        s.push_str(&inn);
        s.push_str(&pretty_expr(t, indent + 1));
        s.push('\n');
    }
    s.push_str(&pad);
    s.push('}');
    s
}

fn pretty_stmt(st: &Stmt, indent: usize) -> String {
    match st {
        Stmt::Let {
            mutable,
            name,
            ty,
            init,
            ..
        } => {
            let mut s = String::from("let ");
            if *mutable {
                s.push_str("mut ");
            }
            s.push_str(&name.name);
            if let Some(t) = ty {
                s.push_str(": ");
                s.push_str(&type_str(t));
            }
            if let Some(e) = init {
                s.push_str(" = ");
                s.push_str(&pretty_expr(e, indent));
            }
            s.push(';');
            s
        }
        Stmt::LetTuple {
            mutable,
            pattern,
            init,
            ..
        } => format!(
            "let {}{} = {};",
            if *mutable { "mut " } else { "" },
            pattern_str(pattern),
            pretty_expr(init, indent)
        ),
        Stmt::Match { scrutinee, arms, .. } => pretty_match(scrutinee, arms, indent),
        Stmt::Assign { target, value, .. } => {
            format!("{} = {};", pretty_expr(target, indent), pretty_expr(value, indent))
        }
        Stmt::Expr { expr, .. } => format!("{};", pretty_expr(expr, indent)),
        Stmt::Return { value, .. } => match value {
            Some(e) => format!("return {};", pretty_expr(e, indent)),
            None => "return;".into(),
        },
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            let mut s = format!("if {} {}", pretty_expr(cond, indent), pretty_block(then_block, indent));
            if let Some(e) = else_block {
                s.push_str(" else ");
                s.push_str(&pretty_block(e, indent));
            }
            s
        }
        Stmt::While { cond, body, .. } => {
            format!("while {} {}", pretty_expr(cond, indent), pretty_block(body, indent))
        }
        Stmt::For {
            var, start, end, body, ..
        } => format!(
            "for {} in {}..{} {}",
            var.name,
            pretty_expr(start, indent),
            pretty_expr(end, indent),
            pretty_block(body, indent)
        ),
        Stmt::Break { .. } => "break;".into(),
        Stmt::Continue { .. } => "continue;".into(),
        Stmt::Yield { .. } => "yield;".into(),
        Stmt::Block { block, .. } => pretty_block(block, indent),
    }
}

/// `match` with one arm per line: `Pattern => expr,` when the body is a
/// bare expression, `Pattern => { ... }` otherwise. `if let`'s generated
/// catch-all arm is printed like any other (the source form is a `match`).
fn pretty_match(scrutinee: &Expr, arms: &[crate::ast::MatchArm], indent: usize) -> String {
    let pad = "    ".repeat(indent);
    let inn = "    ".repeat(indent + 1);
    let mut s = format!("match {} {{\n", pretty_expr(scrutinee, indent));
    for arm in arms {
        s.push_str(&inn);
        s.push_str(&pattern_str(&arm.pattern));
        s.push_str(" => ");
        match (&arm.body.stmts.as_slice(), &arm.body.tail) {
            ([], Some(t)) => {
                s.push_str(&pretty_expr(t, indent + 1));
                s.push(',');
            }
            // `{ match .. }` re-parses as an expression arm (the match becomes
            // the block's tail), so print it that way to stay a fixpoint
            ([Stmt::Match { scrutinee, arms: inner, .. }], None) => {
                s.push_str(&pretty_match(scrutinee, inner, indent + 1));
                s.push(',');
            }
            _ => s.push_str(&pretty_block(&arm.body, indent + 1)),
        }
        s.push('\n');
    }
    s.push_str(&pad);
    s.push('}');
    s
}

fn pretty_expr(e: &Expr, indent: usize) -> String {
    match &e.kind {
        ExprKind::Literal(Literal::Float(v)) => float_src(*v),
        ExprKind::Literal(l) => literal_str(l),
        ExprKind::Ident(n) => n.name.clone(),
        ExprKind::Binary { op, lhs, rhs } => {
            format!("({} {} {})", pretty_expr(lhs, indent), op, pretty_expr(rhs, indent))
        }
        ExprKind::Unary { op, expr } => format!("({}{})", op.as_str(), pretty_expr(expr, indent)),
        ExprKind::Call { callee, args } => {
            let a: Vec<String> = args.iter().map(|x| pretty_expr(x, indent)).collect();
            format!("{}({})", pretty_expr(callee, indent), a.join(", "))
        }
        ExprKind::Index { base, index } => {
            format!("{}[{}]", pretty_expr(base, indent), pretty_expr(index, indent))
        }
        ExprKind::Field { base, field } => format!("{}.{}", pretty_expr(base, indent), field.name),
        ExprKind::Cast { expr, ty } => format!("({} as {})", pretty_expr(expr, indent), type_str(ty)),
        ExprKind::Array { elements } => {
            let a: Vec<String> = elements.iter().map(|x| pretty_expr(x, indent)).collect();
            format!("[{}]", a.join(", "))
        }
        ExprKind::StructLit { name, fields } => {
            let fs: Vec<String> = fields
                .iter()
                .map(|(n, e)| format!("{}: {}", n.name, pretty_expr(e, indent)))
                .collect();
            format!("{} {{ {} }}", name.name, fs.join(", "))
        }
        ExprKind::Tuple { elements } => {
            let e: Vec<String> = elements.iter().map(|x| pretty_expr(x, indent)).collect();
            format!("({})", e.join(", "))
        }
        ExprKind::EnumLit {
            enum_name,
            variant,
            args,
        } => {
            if args.is_empty() {
                format!("{}::{}", enum_name.name, variant.name)
            } else {
                let a: Vec<String> = args.iter().map(|x| pretty_expr(x, indent)).collect();
                format!("{}::{}({})", enum_name.name, variant.name, a.join(", "))
            }
        }
        // binary / unary / cast operands print their own parentheses; adding
        // another pair would grow on every `fmt` round trip
        ExprKind::Group(inner)
            if matches!(
                inner.kind,
                ExprKind::Binary { .. }
                    | ExprKind::Unary { .. }
                    | ExprKind::Cast { .. }
                    | ExprKind::Group(_)
            ) =>
        {
            pretty_expr(inner, indent)
        }
        ExprKind::Group(inner) => format!("({})", pretty_expr(inner, indent)),
        ExprKind::Match { scrutinee, arms } => pretty_match(scrutinee, arms, indent),
    }
}

/// A float literal that lexes back as a float: `3.0`, `1e300`, `0.0025`
/// (`{:?}` always keeps a `.` or an exponent; `inf` has no spelling, so it
/// becomes the overflowing literal `1e999`).
fn float_src(v: f64) -> String {
    if v.is_finite() {
        // `{:?}` prints `1e300`; the documented float grammar wants `1.0e300`
        let t = format!("{v:?}");
        match t.find('e') {
            Some(i) if !t[..i].contains('.') => format!("{}.0{}", &t[..i], &t[i..]),
            _ => t,
        }
    } else {
        "1e999".into()
    }
}

fn type_str(t: &crate::ast::TypeExpr) -> String {
    match &t.kind {
        crate::ast::TypeExprKind::Named(n) => n.clone(),
        crate::ast::TypeExprKind::Unit => "()".into(),
        crate::ast::TypeExprKind::Array { elem, len } => format!("[{}; {len}]", type_str(elem)),
        crate::ast::TypeExprKind::Tuple(elems) => {
            let e: Vec<String> = elems.iter().map(type_str).collect();
            format!("({})", e.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::parser::parse;
    use crate::span::FileId;

    #[test]
    fn pretty_keeps_fn_main() {
        let src = "fn main() -> i32 { return 1 + 2; }";
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, _) = parse(toks);
        let out = pretty_program(&prog);
        assert!(out.contains("fn main"));
        assert!(out.contains("return"));
    }

    #[test]
    fn pretty_prints_bitwise_and_desugared_compound_assignment() {
        let src = "fn main() -> i32 { let mut x = 1; x <<= 2; x ^= !x & 3 | 4 >> 1; return x; }";
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, diags) = parse(toks);
        assert!(!diags.has_errors());
        let out = pretty_program(&prog);
        assert!(out.contains("x = (x << 2);"), "{out}");
        assert!(out.contains("x = (x ^ (((!x) & 3) | (4 >> 1)));"), "{out}");
        let dumped = crate::ast::dump_program(&prog);
        assert!(dumped.contains("x = (x << 2);"), "{dumped}");
    }

    fn fmt(src: &str) -> String {
        let (toks, _) = tokenize(FileId(0), src);
        let (prog, diags) = parse(toks);
        assert!(!diags.has_errors(), "{src}");
        pretty_program(&prog)
    }

    #[test]
    fn pretty_keeps_let_annotations_and_float_literals() {
        let out = fmt("fn main() -> i32 { let x: i64 = 5; let f = 2.0; let g = 1.0e300; let h = 1.5e-7; return 0; }");
        assert!(out.contains("let x: i64 = 5;"), "{out}");
        assert!(out.contains("let f = 2.0;"), "{out}");
        assert!(out.contains("let g = 1.0e300;"), "{out}");
        assert!(out.contains("let h = 1.5e-7;"), "{out}");
    }

    #[test]
    fn pretty_is_idempotent_with_match_expressions_and_nested_patterns() {
        let src = "enum E { A(i32), B }
            fn f(e: (E, i32)) -> i32 {
                let (a, (b, _)) = (1, (2, 3));
                let r = match e { (E::A(1), x) => x + a, (E::A(y), _) => { y } (E::B, _) => b };
                match r { 0 => { return 1; } _ => {} }
                match r { 0 => 1, _ => 2 }
            }
            fn main() -> i32 { if let (E::B, 1) = (E::B, 1) { return f((E::B, 1)); } return 0; }";
        let once = fmt(src);
        let twice = fmt(&once);
        assert_eq!(once, twice, "not idempotent:\n{once}");
        assert!(once.contains("(E::A(1), x) => (x + a),"), "{once}");
        assert!(once.contains("let (a, (b, _)) = (1, (2, 3));"), "{once}");
    }
}
