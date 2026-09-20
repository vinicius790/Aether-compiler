//! Source pretty-printer from the AST.

use crate::ast::{Block, Expr, ExprKind, Item, Program, Stmt};

pub fn pretty_program(p: &Program) -> String {
    let mut s = String::new();
    for (i, item) in p.items.iter().enumerate() {
        if i > 0 {
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
            format!("fn {}({}) -> {ret} {body}", f.name.name, params.join(", "))
        }
        Item::Struct(st) => {
            let fields: Vec<String> = st
                .fields
                .iter()
                .map(|f| format!("    {}: {},", f.name.name, type_str(&f.ty)))
                .collect();
            format!("struct {} {{\n{}\n}}", st.name.name, fields.join("\n"))
        }
        Item::Extern(e) => {
            let params: Vec<String> = e
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name.name, type_str(&p.ty)))
                .collect();
            format!(
                "extern fn {}({}) -> {};",
                e.name.name,
                params.join(", "),
                type_str(&e.return_ty)
            )
        }
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
        s.push_str(&pretty_expr(t));
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
            init,
            ..
        } => {
            let mut s = String::from("let ");
            if *mutable {
                s.push_str("mut ");
            }
            s.push_str(&name.name);
            if let Some(e) = init {
                s.push_str(" = ");
                s.push_str(&pretty_expr(e));
            }
            s.push(';');
            s
        }
        Stmt::Assign { target, value, .. } => {
            format!("{} = {};", pretty_expr(target), pretty_expr(value))
        }
        Stmt::Expr { expr, .. } => format!("{};", pretty_expr(expr)),
        Stmt::Return { value, .. } => match value {
            Some(e) => format!("return {};", pretty_expr(e)),
            None => "return;".into(),
        },
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            let mut s = format!("if {} {}", pretty_expr(cond), pretty_block(then_block, indent));
            if let Some(e) = else_block {
                s.push_str(" else ");
                s.push_str(&pretty_block(e, indent));
            }
            s
        }
        Stmt::While { cond, body, .. } => {
            format!("while {} {}", pretty_expr(cond), pretty_block(body, indent))
        }
        Stmt::For {
            var, start, end, body, ..
        } => format!(
            "for {} in {}..{} {}",
            var.name,
            pretty_expr(start),
            pretty_expr(end),
            pretty_block(body, indent)
        ),
        Stmt::Break { .. } => "break;".into(),
        Stmt::Continue { .. } => "continue;".into(),
        Stmt::Block { block, .. } => pretty_block(block, indent),
    }
}

fn pretty_expr(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Literal(l) => match l {
            crate::ast::Literal::Int(n) => n.to_string(),
            crate::ast::Literal::Float(n) => format!("{n}"),
            crate::ast::Literal::Bool(b) => b.to_string(),
            crate::ast::Literal::String(s) => format!("{s:?}"),
            crate::ast::Literal::Char(c) => format!("{c:?}"),
            crate::ast::Literal::Unit => "()".into(),
        },
        ExprKind::Ident(n) => n.name.clone(),
        ExprKind::Binary { op, lhs, rhs } => {
            format!("({} {} {})", pretty_expr(lhs), op, pretty_expr(rhs))
        }
        ExprKind::Unary { op, expr } => format!("({}{})", op.as_str(), pretty_expr(expr)),
        ExprKind::Call { callee, args } => {
            let a: Vec<String> = args.iter().map(pretty_expr).collect();
            format!("{}({})", pretty_expr(callee), a.join(", "))
        }
        ExprKind::Index { base, index } => {
            format!("{}[{}]", pretty_expr(base), pretty_expr(index))
        }
        ExprKind::Field { base, field } => format!("{}.{}", pretty_expr(base), field.name),
        ExprKind::Cast { expr, ty } => format!("({} as {})", pretty_expr(expr), type_str(ty)),
        ExprKind::Array { elements } => {
            let a: Vec<String> = elements.iter().map(pretty_expr).collect();
            format!("[{}]", a.join(", "))
        }
        ExprKind::StructLit { name, fields } => {
            let fs: Vec<String> = fields
                .iter()
                .map(|(n, e)| format!("{}: {}", n.name, pretty_expr(e)))
                .collect();
            format!("{} {{ {} }}", name.name, fs.join(", "))
        }
        ExprKind::Group(inner) => format!("({})", pretty_expr(inner)),
    }
}

fn type_str(t: &crate::ast::TypeExpr) -> String {
    match &t.kind {
        crate::ast::TypeExprKind::Named(n) => n.clone(),
        crate::ast::TypeExprKind::Unit => "()".into(),
        crate::ast::TypeExprKind::Array { elem, len } => format!("[{}; {len}]", type_str(elem)),
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
}
