//! Source pretty-printer from the AST.

use crate::ast::{
    literal_str, BinOp, pattern_str, variant_str, Block, Expr, ExprKind, Item, Literal, MatchArm, Program,
    Stmt,
};
use crate::comments::Comments;
use crate::span::Span;

/// The source form of `p`, without comments (see [`crate::comments`] for
/// the comment-keeping `aether fmt` entry point).
pub fn pretty_program(p: &Program) -> String {
    pretty_program_with(p, None)
}

/// Prints `p`; with `Some(comments)` every comment of the source is placed
/// as described in [`crate::comments`] and blank lines are kept.
pub(crate) fn pretty_program_with(p: &Program, comments: Option<Comments<'_>>) -> String {
    let mut pr = Printer {
        c: comments,
        force_blank: false,
    };
    pr.program(p)
}

type Regions = Vec<(u32, u32)>;

struct Printer<'a> {
    c: Option<Comments<'a>>,
    /// The next line-level element starts a new group (blank line first).
    force_blank: bool,
}

impl Printer<'_> {
    fn program(&mut self, p: &Program) -> String {
        let mut s = String::new();
        for (i, item) in p.items.iter().enumerate() {
            // consecutive `use` lines stay together; other items are separated
            let uses = matches!(item, Item::Use(_))
                && i > 0
                && matches!(p.items[i - 1], Item::Use(_));
            self.force_blank = i > 0 && !uses;
            let r = self.item_regions(item);
            self.lead(&mut s, "", item.span(), &r);
            let text = self.item(item);
            s.push_str(&text);
            self.trail(&mut s, item.span().end.0);
            s.push('\n');
        }
        self.force_blank = false;
        self.lead_end(&mut s, "", u32::MAX);
        s
    }

    // ---- comment placement -------------------------------------------

    /// A blank line before an element starting at `start` when one is due.
    fn sep(&mut self, out: &mut String, start: u32) {
        let blank = self.force_blank
            || self.c.as_ref().map_or(false, |c| {
                !c.at_open && c.last_end < start && c.blank_between(c.last_end, start)
            });
        if blank {
            out.push('\n');
        }
        self.force_blank = false;
        if let Some(c) = self.c.as_mut() {
            c.at_open = false;
        }
    }

    fn emit_lines(&mut self, out: &mut String, pad: &str, ids: Vec<usize>) {
        for i in ids {
            let (start, end, text) = match self.c.as_ref() {
                Some(c) => (c.start(i), c.end(i), c.text(i)),
                None => return,
            };
            self.sep(out, start);
            out.push_str(pad);
            out.push_str(&text);
            out.push('\n');
            if let Some(c) = self.c.as_mut() {
                c.last_end = c.last_end.max(end);
            }
        }
    }

    /// Leading comments of the element `span` (whose own lines are
    /// `regions`), each on its own line, then the separator before it.
    fn lead(&mut self, out: &mut String, pad: &str, span: Span, regions: &[(u32, u32)]) {
        let ids = match self.c.as_mut() {
            Some(c) => c.take_leading(span, regions),
            None => Vec::new(),
        };
        self.emit_lines(out, pad, ids);
        self.sep(out, span.start.0);
    }

    /// The comments left before `pos` (end of a block / of the file).
    fn lead_end(&mut self, out: &mut String, pad: &str, pos: u32) {
        let ids = match self.c.as_mut() {
            Some(c) => c.take_before(pos),
            None => Vec::new(),
        };
        self.emit_lines(out, pad, ids);
    }

    /// Same-line comments after an element ending at `end`.
    fn trail(&mut self, out: &mut String, end: u32) {
        if let Some(c) = self.c.as_mut() {
            for i in c.take_trailing(end) {
                out.push(' ');
                out.push_str(&c.text(i));
                c.last_end = c.last_end.max(c.end(i));
            }
            c.last_end = c.last_end.max(end);
        }
    }

    /// After a printed `{` (at `brace` in the source): its same-line comments.
    fn open(&mut self, out: &mut String, brace: Option<u32>) {
        if let Some(c) = self.c.as_mut() {
            if let Some(b) = brace {
                for i in c.take_trailing(b + 1) {
                    out.push(' ');
                    out.push_str(&c.text(i));
                    c.last_end = c.last_end.max(c.end(i));
                }
            }
            c.at_open = true;
        }
    }

    fn close(&mut self) {
        if let Some(c) = self.c.as_mut() {
            c.at_open = false;
        }
    }

    fn lbrace_after(&self, pos: u32) -> Option<u32> {
        self.c.as_ref().and_then(|c| c.lbrace_after(pos))
    }

    // ---- regions: the parts of an element printed on lines of their own

    fn item_regions(&self, item: &Item) -> Regions {
        let mut r = Vec::new();
        if self.c.is_none() {
            return r;
        }
        match item {
            Item::Fn(f) => {
                if let Some(b) = &f.body {
                    r.push((b.span.start.0, b.span.end.0));
                }
            }
            Item::Struct(st) => {
                if let Some(b) = self.lbrace_after(st.name.span.end.0) {
                    r.push((b, st.span.end.0));
                }
            }
            Item::Enum(en) => {
                if let Some(b) = self.lbrace_after(en.name.span.end.0) {
                    r.push((b, en.span.end.0));
                }
            }
            Item::Extern(_) | Item::Use(_) => {}
        }
        r
    }

    fn match_regions(&self, scrutinee: &Expr, span: Span, r: &mut Regions) {
        self.expr_regions(scrutinee, r);
        let end = scrutinee.span.end.0;
        let b = self.lbrace_after(end).unwrap_or(end);
        r.push((b, span.end.0));
    }

    fn expr_regions(&self, e: &Expr, r: &mut Regions) {
        if self.c.is_none() {
            return;
        }
        match &e.kind {
            ExprKind::Literal(_) | ExprKind::Ident(_) => {}
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr_regions(lhs, r);
                self.expr_regions(rhs, r);
            }
            ExprKind::Unary { expr, .. } | ExprKind::Cast { expr, .. } | ExprKind::Group(expr) => {
                self.expr_regions(expr, r)
            }
            ExprKind::Call { callee, args } => {
                self.expr_regions(callee, r);
                args.iter().for_each(|a| self.expr_regions(a, r));
            }
            ExprKind::Index { base, index } => {
                self.expr_regions(base, r);
                self.expr_regions(index, r);
            }
            ExprKind::Field { base, .. } => self.expr_regions(base, r),
            ExprKind::ArrayRepeat { value, .. } => self.expr_regions(value, r),
            ExprKind::Array { elements } | ExprKind::Tuple { elements } => {
                elements.iter().for_each(|a| self.expr_regions(a, r))
            }
            ExprKind::EnumLit { args, .. } => args.iter().for_each(|a| self.expr_regions(a, r)),
            ExprKind::StructLit { fields, .. } => {
                fields.iter().for_each(|(_, a)| self.expr_regions(a, r))
            }
            ExprKind::Match { scrutinee, .. } => self.match_regions(scrutinee, e.span, r),
        }
    }

    fn stmt_regions(&self, st: &Stmt) -> Regions {
        let mut r = Vec::new();
        if self.c.is_none() {
            return r;
        }
        let block = |b: &Block, r: &mut Regions| r.push((b.span.start.0, b.span.end.0));
        match st {
            Stmt::Let { init, .. } => {
                if let Some(e) = init {
                    self.expr_regions(e, &mut r);
                }
            }
            Stmt::LetTuple { init, .. } => self.expr_regions(init, &mut r),
            Stmt::Match { arms, .. } if is_if_let(arms) => {
                block(&arms[0].body, &mut r);
                self.else_regions(&arms[1].body, &mut r);
            }
            Stmt::Match { scrutinee, span, .. } => self.match_regions(scrutinee, *span, &mut r),
            Stmt::Assign { target, value, .. } => {
                self.expr_regions(target, &mut r);
                self.expr_regions(value, &mut r);
            }
            Stmt::Expr { expr, .. } => self.expr_regions(expr, &mut r),
            Stmt::Return { value, .. } => {
                if let Some(e) = value {
                    self.expr_regions(e, &mut r);
                }
            }
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                self.expr_regions(cond, &mut r);
                block(then_block, &mut r);
                if let Some(b) = else_block {
                    self.else_regions(b, &mut r);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.expr_regions(cond, &mut r);
                block(body, &mut r);
            }
            Stmt::For {
                start, end, body, ..
            } => {
                self.expr_regions(start, &mut r);
                self.expr_regions(end, &mut r);
                block(body, &mut r);
            }
            Stmt::Block { block: b, .. } => block(b, &mut r),
            Stmt::Break { .. } | Stmt::Continue { .. } | Stmt::Yield { .. } => {}
        }
        r
    }

    /// Mirrors how `match_` prints the arm body.
    fn arm_regions(&self, arm: &MatchArm) -> Regions {
        let mut r = Vec::new();
        if self.c.is_none() {
            return r;
        }
        match (&arm.body.stmts.as_slice(), &arm.body.tail) {
            ([], Some(t)) => self.expr_regions(t, &mut r),
            _ => r.push((arm.body.span.start.0, arm.body.span.end.0)),
        }
        r
    }

    /// The `else` part of an `if` / `if let`: an `else if` continues the
    /// chain on the same line, so its condition's comments lead the chain.
    fn else_regions(&self, b: &Block, r: &mut Regions) {
        match else_if(b) {
            Some(st) => r.extend(self.stmt_regions(st)),
            None => r.push((b.span.start.0, b.span.end.0)),
        }
    }

    // ---- printing ------------------------------------------------------

    fn item(&mut self, item: &Item) -> String {
        match item {
            Item::Fn(f) => {
                let params: Vec<String> = f
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name.name, type_str(&p.ty)))
                    .collect();
                let ret = ret_str(&f.return_ty);
                let body = match &f.body {
                    Some(b) => self.block(b, 0),
                    None => ";".into(),
                };
                format!(
                    "{}fn {}({}){ret} {body}",
                    vis(f.is_pub),
                    f.name.name,
                    params.join(", ")
                )
            }
            Item::Struct(st) => {
                let mut s = format!("{}struct {} {{", vis(st.is_pub), st.name.name);
                let b = self.lbrace_after(st.name.span.end.0);
                self.open(&mut s, b);
                s.push('\n');
                for f in &st.fields {
                    self.lead(&mut s, "    ", f.span, &[]);
                    s.push_str(&format!("    {}: {},", f.name.name, type_str(&f.ty)));
                    self.trail(&mut s, f.span.end.0);
                    s.push('\n');
                }
                self.lead_end(&mut s, "    ", st.span.end.0);
                s.push('}');
                self.close();
                s
            }
            Item::Enum(en) => {
                let mut s = format!("{}enum {} {{", vis(en.is_pub), en.name.name);
                let b = self.lbrace_after(en.name.span.end.0);
                self.open(&mut s, b);
                s.push('\n');
                for v in &en.variants {
                    self.lead(&mut s, "    ", v.span, &[]);
                    s.push_str(&format!("    {},", variant_str(v)));
                    self.trail(&mut s, v.span.end.0);
                    s.push('\n');
                }
                self.lead_end(&mut s, "    ", en.span.end.0);
                s.push('}');
                self.close();
                s
            }
            Item::Extern(e) => {
                let params: Vec<String> = e
                    .params
                    .iter()
                    .map(|p| format!("{}: {}", p.name.name, type_str(&p.ty)))
                    .collect();
                format!(
                    "{}extern fn {}({}){};",
                    vis(e.is_pub),
                    e.name.name,
                    params.join(", "),
                    ret_str(&e.return_ty)
                )
            }
            Item::Use(u) => format!("use {:?};", u.path),
        }
    }

    fn block(&mut self, b: &Block, indent: usize) -> String {
        let pad = "    ".repeat(indent);
        let inn = "    ".repeat(indent + 1);
        let mut s = String::from("{");
        let brace = match self.c.as_ref() {
            Some(c) if c.braced(b.span) => Some(b.span.start.0),
            _ => None,
        };
        self.open(&mut s, brace);
        s.push('\n');
        for (i, st) in b.stmts.iter().enumerate() {
            let r = self.stmt_regions(st);
            self.lead(&mut s, &inn, st.span(), &r);
            s.push_str(&inn);
            let text = self.stmt(st, indent + 1);
            s.push_str(&text);
            // a `match` last in a block would re-parse as the block's tail
            // expression (its value); `;` keeps it a statement
            if i + 1 == b.stmts.len() && b.tail.is_none() {
                if let Stmt::Match { arms, .. } = st {
                    if !is_if_let(arms) {
                        s.push(';');
                    }
                }
            }
            self.trail(&mut s, st.span().end.0);
            s.push('\n');
        }
        if let Some(t) = &b.tail {
            let mut r = Vec::new();
            self.expr_regions(t, &mut r);
            self.lead(&mut s, &inn, t.span, &r);
            s.push_str(&inn);
            let text = self.expr(t, indent + 1);
            s.push_str(&text);
            self.trail(&mut s, t.span.end.0);
            s.push('\n');
        }
        self.lead_end(&mut s, &inn, b.span.end.0);
        s.push_str(&pad);
        s.push('}');
        self.close();
        s
    }

    fn stmt(&mut self, st: &Stmt, indent: usize) -> String {
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
                    let text = self.expr(e, indent);
                    s.push_str(&text);
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
                self.expr(init, indent)
            ),
            Stmt::Match {
                scrutinee, arms, ..
            } if is_if_let(arms) => {
                let mut s = format!(
                    "if let {} = {} ",
                    pattern_str(&arms[0].pattern),
                    self.head(scrutinee, indent)
                );
                let text = self.block(&arms[0].body, indent);
                s.push_str(&text);
                // without `else` the generated arm's empty body has the
                // `then` block's span
                if arms[1].body.span != arms[0].body.span {
                    let text = self.else_part(&arms[1].body, indent);
                    s.push_str(&text);
                }
                s
            }
            Stmt::Match {
                scrutinee,
                arms,
                span,
            } => self.match_(scrutinee, arms, *span, indent),
            Stmt::Assign { target, value, .. } => {
                let t = self.expr(target, indent);
                match compound_rhs(target, value) {
                    Some((op, rhs)) => format!("{t} {op}= {};", self.expr(rhs, indent)),
                    None => format!("{} = {};", t, self.expr(value, indent)),
                }
            }
            Stmt::Expr { expr, .. } => format!("{};", self.expr(expr, indent)),
            Stmt::Return { value, .. } => match value {
                Some(e) => format!("return {};", self.expr(e, indent)),
                None => "return;".into(),
            },
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                let c = self.head(cond, indent);
                let mut s = format!("if {} {}", c, self.block(then_block, indent));
                if let Some(e) = else_block {
                    let text = self.else_part(e, indent);
                    s.push_str(&text);
                }
                s
            }
            Stmt::While { cond, body, .. } => {
                let c = self.head(cond, indent);
                format!("while {} {}", c, self.block(body, indent))
            }
            Stmt::For {
                var, start, end, body, ..
            } => {
                let a = self.head(start, indent);
                let b = self.head(end, indent);
                format!("for {} in {}..{} {}", var.name, a, b, self.block(body, indent))
            }
            Stmt::Break { .. } => "break;".into(),
            Stmt::Continue { .. } => "continue;".into(),
            Stmt::Yield { .. } => "yield;".into(),
            Stmt::Block { block, .. } => self.block(block, indent),
        }
    }

    /// ` else { .. }`, or ` else if ..` for the one-statement block the
    /// parser wraps an `else if` / `else if let` in (printing it as a
    /// block would add a nesting level per link of the chain).
    fn else_part(&mut self, b: &Block, indent: usize) -> String {
        match else_if(b) {
            Some(st) => format!(" else {}", self.stmt(st, indent)),
            None => format!(" else {}", self.block(b, indent)),
        }
    }

    /// `match` with one arm per line: `Pattern => expr,` when the body is a
    /// bare expression, `Pattern => { ... }` otherwise. `if let`'s generated
    /// catch-all arm is printed like any other (the source form is a `match`).
    fn match_(&mut self, scrutinee: &Expr, arms: &[MatchArm], span: Span, indent: usize) -> String {
        let pad = "    ".repeat(indent);
        let inn = "    ".repeat(indent + 1);
        let mut s = format!("match {} {{", self.head(scrutinee, indent));
        let b = self.lbrace_after(scrutinee.span.end.0);
        self.open(&mut s, b);
        s.push('\n');
        for arm in arms {
            let r = self.arm_regions(arm);
            self.lead(&mut s, &inn, arm.span, &r);
            s.push_str(&inn);
            s.push_str(&pattern_str(&arm.pattern));
            s.push_str(" => ");
            match (&arm.body.stmts.as_slice(), &arm.body.tail) {
                ([], Some(t)) => {
                    let text = self.expr(t, indent + 1);
                    s.push_str(&text);
                    s.push(',');
                }
                _ => {
                    let text = self.block(&arm.body, indent + 1);
                    s.push_str(&text);
                }
            }
            self.trail(&mut s, arm.span.end.0);
            s.push('\n');
        }
        self.lead_end(&mut s, &inn, span.end.0);
        s.push_str(&pad);
        s.push('}');
        self.close();
        s
    }

    fn expr(&mut self, e: &Expr, indent: usize) -> String {
        self.expr_at(e, indent, 0)
    }

    /// `e` as an operand that must bind at least as tightly as `min` (a
    /// level of [`prec`]). Parentheses are printed only where the grammar
    /// needs them: every pair costs one level of the parser's nesting
    /// limit (E0101), so a fully parenthesised rendering of a program near
    /// the limit would no longer compile.
    fn expr_at(&mut self, e: &Expr, indent: usize, min: u8) -> String {
        let text = match &e.kind {
            ExprKind::Literal(Literal::Float(v)) => float_src(*v),
            ExprKind::Literal(l) => literal_str(l),
            ExprKind::Ident(n) => n.name.clone(),
            ExprKind::Binary { op, lhs, rhs } => {
                let p = binop_prec(*op);
                let l = self.lhs_operand(lhs, indent, p);
                let r = self.expr_at(rhs, indent, p + 1);
                format!("{l} {op} {r}")
            }
            // `-` `-` stays two tokens (there is no `--` operator), and a
            // negated literal stays a direct negation (one literal)
            ExprKind::Unary { op, expr } => {
                format!("{}{}", op.as_str(), self.expr_at(expr, indent, PREC_UNARY))
            }
            ExprKind::Call { callee, args } => {
                let c = self.postfix_base(callee, indent);
                let a: Vec<String> = args.iter().map(|x| self.expr(x, indent)).collect();
                format!("{c}({})", a.join(", "))
            }
            ExprKind::Index { base, index } => {
                let b = self.postfix_base(base, indent);
                format!("{b}[{}]", self.expr(index, indent))
            }
            ExprKind::Field { base, field } => {
                format!("{}.{}", self.postfix_base(base, indent), field.name)
            }
            ExprKind::Cast { expr, ty } => {
                format!("{} as {}", self.lhs_operand(expr, indent, PREC_CAST), type_str(ty))
            }
            ExprKind::Array { elements } => {
                let a: Vec<String> = elements.iter().map(|x| self.expr(x, indent)).collect();
                format!("[{}]", a.join(", "))
            }
            ExprKind::ArrayRepeat { value, count } => format!("[{}; {count}]", self.expr(value, indent)),
            ExprKind::StructLit { name, fields } => {
                let fs: Vec<String> = fields
                    .iter()
                    .map(|(n, e)| format!("{}: {}", n.name, self.expr(e, indent)))
                    .collect();
                if fs.is_empty() {
                    format!("{} {{}}", name.name)
                } else {
                    format!("{} {{ {} }}", name.name, fs.join(", "))
                }
            }
            ExprKind::Tuple { elements } => {
                let e: Vec<String> = elements.iter().map(|x| self.expr(x, indent)).collect();
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
                    let a: Vec<String> = args.iter().map(|x| self.expr(x, indent)).collect();
                    format!("{}::{}({})", enum_name.name, variant.name, a.join(", "))
                }
            }
            // Parentheses around a literal are meaningful (`-(2147483648)`
            // is not the literal `-2147483648`) and `(match ..)` is an
            // expression where a bare `match` would be a statement; any
            // other group is reprinted with exactly the parentheses its
            // position needs.
            ExprKind::Group(inner) => match &inner.kind {
                ExprKind::Literal(_) | ExprKind::Match { .. } => {
                    format!("({})", self.expr(inner, indent))
                }
                _ => return self.expr_at(inner, indent, min),
            },
            ExprKind::Match { scrutinee, arms } => self.match_(scrutinee, arms, e.span, indent),
        };
        if prec(e) < min {
            format!("({text})")
        } else {
            text
        }
    }

    /// The head of an `if` / `while` / `for` / `match`: parenthesised when a
    /// struct literal without fields would end it (`if (u == U {}) {`).
    fn head(&mut self, e: &Expr, indent: usize) -> String {
        if ends_in_empty_lit(e) {
            format!("({})", self.expr(e, indent))
        } else {
            self.expr(e, indent)
        }
    }

    /// The left operand of a binary operator or `as`: a `match` there is
    /// parenthesised (at the start of a statement it would be a `match`
    /// statement, ending before the operator).
    fn lhs_operand(&mut self, e: &Expr, indent: usize, min: u8) -> String {
        if is_bare(e) {
            format!("({})", self.expr(e, indent))
        } else {
            self.expr_at(e, indent, min)
        }
    }

    /// The base of a call, index or field access: `match` takes no
    /// postfix operator, so it is parenthesised.
    fn postfix_base(&mut self, e: &Expr, indent: usize) -> String {
        if is_bare(e) {
            format!("({})", self.expr(e, indent))
        } else {
            self.expr_at(e, indent, PREC_POSTFIX)
        }
    }
}

const PREC_CAST: u8 = 11;
const PREC_UNARY: u8 = 12;
const PREC_POSTFIX: u8 = 13;

/// Binary operator precedence, the parser's table (`docs/language.md`).
fn binop_prec(op: BinOp) -> u8 {
    match op {
        BinOp::Or => 1,
        BinOp::And => 2,
        BinOp::Eq | BinOp::Ne => 3,
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 4,
        BinOp::BitOr => 5,
        BinOp::BitXor => 6,
        BinOp::BitAnd => 7,
        BinOp::Shl | BinOp::Shr => 8,
        BinOp::Add | BinOp::Sub => 9,
        BinOp::Mul | BinOp::Div | BinOp::Rem => 10,
    }
}

/// How tightly the printed form of `e` binds. A literal `i64::MIN` prints
/// with its `-` and so binds like a prefix operator.
fn prec(e: &Expr) -> u8 {
    match &e.kind {
        ExprKind::Binary { op, .. } => binop_prec(*op),
        ExprKind::Cast { .. } => PREC_CAST,
        ExprKind::Unary { .. } => PREC_UNARY,
        ExprKind::Literal(Literal::Int(v)) if *v < 0 => PREC_UNARY,
        ExprKind::Group(inner) if !matches!(inner.kind, ExprKind::Literal(_) | ExprKind::Match { .. }) => {
            prec(inner)
        }
        _ => PREC_POSTFIX,
    }
}

/// `e` prints as a bare `match`, looking through the groups the printer
/// drops: the parser takes no postfix or infix operator after a `match`
/// that starts a statement.
fn is_bare(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Match { .. } => true,
        ExprKind::Group(inner) if !matches!(inner.kind, ExprKind::Literal(_) | ExprKind::Match { .. }) => {
            is_bare(inner)
        }
        _ => false,
    }
}

/// `e` prints with a struct literal without fields outside any brackets
/// (the parser reads `U {}` there as `U` and a block when in a head).
fn ends_in_empty_lit(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::StructLit { fields, .. } => fields.is_empty(),
        ExprKind::Binary { lhs, rhs, .. } => ends_in_empty_lit(lhs) || ends_in_empty_lit(rhs),
        ExprKind::Unary { expr, .. } | ExprKind::Cast { expr, .. } => ends_in_empty_lit(expr),
        ExprKind::Field { base, .. } | ExprKind::Index { base, .. } => ends_in_empty_lit(base),
        ExprKind::Call { callee, .. } => ends_in_empty_lit(callee),
        ExprKind::Group(inner) => ends_in_empty_lit(inner),
        _ => false,
    }
}

/// `target = target op rhs` as written by `target op= rhs` (the parser's
/// desugaring), so `fmt` prints the compound form back: it is shorter and
/// one nesting level shallower. The two `target`s are compared as printed
/// (`x = (x) * 2` is `x *= 2` too, or a second `fmt` would change it).
fn compound_rhs<'e>(target: &Expr, value: &'e Expr) -> Option<(BinOp, &'e Expr)> {
    let mut v = value;
    while let ExprKind::Group(inner) = &v.kind {
        v = inner;
    }
    match &v.kind {
        ExprKind::Binary { op, lhs, rhs }
            if !op.is_cmp() && !op.is_logical() && plain(lhs) == plain(target) =>
        {
            Some((*op, rhs))
        }
        _ => None,
    }
}

/// `e` printed without comments.
fn plain(e: &Expr) -> String {
    Printer {
        c: None,
        force_blank: false,
    }
    .expr(e, 0)
}

/// The `match` an `if let` is parsed into: its second arm is the generated
/// catch-all holding the `else` block.
fn is_if_let(arms: &[MatchArm]) -> bool {
    arms.len() == 2 && !arms[0].generated && arms[1].generated
}

/// The `if` / `if let` of an `else if`: the parser wraps it in a block with
/// the statement's own span (a written `{ .. }` also spans its braces).
fn else_if(b: &Block) -> Option<&Stmt> {
    match (b.stmts.as_slice(), &b.tail) {
        ([st @ Stmt::If { .. }], None) if st.span() == b.span => Some(st),
        ([st @ Stmt::Match { arms, .. }], None) if st.span() == b.span && is_if_let(arms) => Some(st),
        _ => None,
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

/// ` -> T`, or nothing for `unit` (`fn main() {`, as usually written).
fn ret_str(t: &crate::ast::TypeExpr) -> String {
    match t.kind {
        crate::ast::TypeExprKind::Unit => String::new(),
        _ => format!(" -> {}", type_str(t)),
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
        // the compound form comes back, with only the parentheses needed
        assert!(out.contains("x <<= 2;"), "{out}");
        assert!(out.contains("x ^= !x & 3 | 4 >> 1;"), "{out}");
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
        assert!(once.contains("(E::A(1), x) => x + a,"), "{once}");
        assert!(once.contains("let (a, (b, _)) = (1, (2, 3));"), "{once}");
    }
}
