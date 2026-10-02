//! Recursive-descent parser with a Pratt parser for expressions.
//!
//! Statement and item grammar is LL(1)-friendly; operator precedence and
//! associativity live in the Pratt table so they cannot drift from the spec.

use crate::ast::*;
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::span::Span;
use crate::token::{Token, TokenKind};

/// Maximum depth of syntactic nesting (parenthesised expressions, blocks,
/// array types, `else if` chains) before the parser gives up on a subtree.
/// The parser, sema and lowering all recurse once per level, so without a
/// bound a few hundred thousand `(` would overflow the host stack.
pub const MAX_NESTING: usize = 256;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    diags: Diagnostics,
    depth: usize,
    depth_reported: bool,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            pos: 0,
            diags: Diagnostics::new(),
            depth: 0,
            depth_reported: false,
        }
    }

    /// Enter one level of nesting. Returns `None` (after reporting E0101
    /// once) when `MAX_NESTING` would be exceeded, so callers can bail out
    /// with `?` and let statement-level recovery skip the rest.
    fn enter_nesting(&mut self) -> Option<()> {
        if self.depth >= MAX_NESTING {
            if !self.depth_reported {
                self.depth_reported = true;
                let span = self.peek_span();
                self.diags.push(
                    Diagnostic::error(
                        format!("nesting too deep (limit {MAX_NESTING})"),
                        span,
                    )
                    .with_code("E0101")
                    .with_help("split the expression or block into smaller pieces"),
                );
            }
            return None;
        }
        self.depth += 1;
        Some(())
    }

    fn leave_nesting(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    pub fn parse_program(mut self) -> (Program, Diagnostics) {
        let start = self.peek_span();
        let mut items = Vec::new();
        while !self.is_eof() {
            match self.parse_item() {
                Some(item) => items.push(item),
                None => {
                    // panic-mode: skip until the next top-level keyword
                    if self.is_eof() {
                        break;
                    }
                    self.bump();
                    while !self.is_eof()
                        && !matches!(
                            self.peek_kind(),
                            TokenKind::Fn
                                | TokenKind::Struct
                                | TokenKind::Enum
                                | TokenKind::Extern
                                | TokenKind::Use
                                | TokenKind::Pub
                                | TokenKind::Eof
                        )
                    {
                        self.bump();
                    }
                }
            }
        }
        let end = self.peek_span();
        (
            Program {
                items,
                span: start.merge(end),
            },
            self.diags,
        )
    }

    fn parse_item(&mut self) -> Option<Item> {
        // `pub` is accepted and recorded; visibility is not enforced in 0.3.
        let is_pub = self.eat(TokenKind::Pub);
        match self.peek_kind() {
            TokenKind::Fn => self.parse_fn().map(|mut f| {
                f.is_pub = is_pub;
                Item::Fn(f)
            }),
            TokenKind::Struct => self.parse_struct().map(|mut s| {
                s.is_pub = is_pub;
                Item::Struct(s)
            }),
            TokenKind::Enum => self.parse_enum().map(Item::Enum),
            TokenKind::Extern => self.parse_extern().map(|mut e| {
                e.is_pub = is_pub;
                Item::Extern(e)
            }),
            TokenKind::Use if !is_pub => self.parse_use().map(Item::Use),
            TokenKind::Eof if !is_pub => None,
            _ => {
                let tok = self.peek().clone();
                self.error_at(
                    format!("expected item, found `{}`", tok.lexeme),
                    tok.span,
                    Some("items start with `fn`, `struct`, `enum`, `extern` or `use`; `pub` may precede fn/struct/enum/extern"),
                );
                None
            }
        }
    }

    /// `use "relative/path.ae";` — resolved by the driver, not here.
    fn parse_use(&mut self) -> Option<UseDecl> {
        let start = self.expect(TokenKind::Use)?.span;
        let tok = self.expect(TokenKind::String)?;
        let (path, bad) = unescape_string(&tok.lexeme);
        if bad {
            self.bad_unicode_escape(tok.span);
        }
        let end = self.expect(TokenKind::Semicolon)?.span;
        Some(UseDecl {
            path,
            span: start.merge(end),
        })
    }

    fn parse_fn(&mut self) -> Option<FnDecl> {
        let start = self.expect(TokenKind::Fn)?.span;
        let name = self.parse_ident()?;
        self.expect(TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(TokenKind::RParen)?;
        let return_ty = if self.eat(TokenKind::Arrow) {
            self.parse_type()?
        } else {
            TypeExpr::unit(self.peek_span())
        };
        let body = if self.check(TokenKind::LBrace) {
            Some(self.parse_block()?)
        } else {
            self.expect(TokenKind::Semicolon)?;
            None
        };
        let end = body.as_ref().map(|b| b.span).unwrap_or(self.prev_span());
        Some(FnDecl {
            is_pub: false,
            name,
            params,
            return_ty,
            body,
            span: start.merge(end),
        })
    }

    fn parse_extern(&mut self) -> Option<ExternDecl> {
        let start = self.expect(TokenKind::Extern)?.span;
        self.expect(TokenKind::Fn)?;
        let name = self.parse_ident()?;
        self.expect(TokenKind::LParen)?;
        let params = self.parse_params()?;
        self.expect(TokenKind::RParen)?;
        let return_ty = if self.eat(TokenKind::Arrow) {
            self.parse_type()?
        } else {
            TypeExpr::unit(self.peek_span())
        };
        self.expect(TokenKind::Semicolon)?;
        Some(ExternDecl {
            is_pub: false,
            name,
            params,
            return_ty,
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_params(&mut self) -> Option<Vec<Param>> {
        let mut params = Vec::new();
        if self.check(TokenKind::RParen) {
            return Some(params);
        }
        loop {
            let name = self.parse_ident()?;
            self.expect(TokenKind::Colon)?;
            let ty = self.parse_type()?;
            let span = name.span.merge(ty.span);
            params.push(Param { name, ty, span });
            if self.eat(TokenKind::Comma) {
                if self.check(TokenKind::RParen) {
                    break;
                }
                continue;
            }
            break;
        }
        Some(params)
    }

    fn parse_struct(&mut self) -> Option<StructDecl> {
        let start = self.expect(TokenKind::Struct)?.span;
        let name = self.parse_ident()?;
        self.expect(TokenKind::LBrace)?;
        let mut fields = Vec::new();
        while !self.check(TokenKind::RBrace) && !self.is_eof() {
            let fname = self.parse_ident()?;
            self.expect(TokenKind::Colon)?;
            let ty = self.parse_type()?;
            let span = fname.span.merge(ty.span);
            fields.push(FieldDecl {
                name: fname,
                ty,
                span,
            });
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(TokenKind::RBrace)?.span;
        Some(StructDecl {
            is_pub: false,
            name,
            fields,
            span: start.merge(end),
        })
    }

    /// `enum Name { Variant(T, ...), Unit, ... }`
    fn parse_enum(&mut self) -> Option<EnumDecl> {
        let start = self.expect(TokenKind::Enum)?.span;
        let name = self.parse_ident()?;
        self.expect(TokenKind::LBrace)?;
        let mut variants = Vec::new();
        while !self.check(TokenKind::RBrace) && !self.is_eof() {
            let vname = self.parse_ident()?;
            let mut payload = Vec::new();
            if self.eat(TokenKind::LParen) {
                while !self.check(TokenKind::RParen) && !self.is_eof() {
                    payload.push(self.parse_type()?);
                    if !self.eat(TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(TokenKind::RParen)?;
            }
            let span = vname.span.merge(self.prev_span());
            variants.push(VariantDecl {
                name: vname,
                payload,
                span,
            });
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(TokenKind::RBrace)?.span;
        Some(EnumDecl {
            name,
            variants,
            span: start.merge(end),
        })
    }

    fn parse_block(&mut self) -> Option<Block> {
        self.enter_nesting()?;
        let result = self.parse_block_inner();
        self.leave_nesting();
        result
    }

    // The recursive entry points (`parse_block_inner`, `parse_stmt`,
    // `parse_prec_inner`, `parse_prefix`) are kept to small dispatchers that
    // call out-of-line helpers. Debug builds reserve stack for every arm of
    // a `match` up front, so one fat function per nesting level would eat a
    // 2 MiB thread stack well before `MAX_NESTING`.

    fn parse_block_inner(&mut self) -> Option<Block> {
        let start = self.expect(TokenKind::LBrace)?.span;
        let mut stmts = Vec::new();
        let mut tail = None;
        while !self.check(TokenKind::RBrace) && !self.is_eof() {
            let before = self.pos;
            if self.is_stmt_start() {
                if let Some(stmt) = self.parse_stmt() {
                    stmts.push(stmt);
                } else {
                    self.synchronize_stmt();
                }
            } else if self.parse_expr_stmt_or_tail(&mut stmts, &mut tail) {
                break;
            }
            if self.pos == before {
                // Recovery made no progress: `synchronize_stmt` stops in
                // front of `fn` / `struct` without consuming them. An item
                // keyword inside a block means the block is unterminated, so
                // end it here and let the item parse at top level; anything
                // else is skipped so the loop always advances.
                if matches!(
                    self.peek_kind(),
                    TokenKind::Fn | TokenKind::Struct | TokenKind::Enum | TokenKind::Extern
                ) {
                    break;
                }
                self.bump();
            }
        }
        let end = match self.expect(TokenKind::RBrace) {
            Some(t) => t.span,
            None => self.peek_span(),
        };
        Some(Block {
            stmts,
            tail,
            span: start.merge(end),
        })
    }

    /// Expression statement, assignment, or block tail. Returns `true` when
    /// the block tail was consumed and the block body is complete.
    fn parse_expr_stmt_or_tail(
        &mut self,
        stmts: &mut Vec<Stmt>,
        tail: &mut Option<Box<Expr>>,
    ) -> bool {
        let expr = match self.parse_expr() {
            Some(e) => e,
            None => {
                self.synchronize_stmt();
                return false;
            }
        };
        let compound = compound_assign_op(self.peek_kind());
        if compound.is_some() || self.check(TokenKind::Eq) {
            self.bump();
            let value = match self.parse_expr() {
                Some(v) => v,
                None => {
                    self.synchronize_stmt();
                    return false;
                }
            };
            self.expect(TokenKind::Semicolon);
            let span = expr.span.merge(self.prev_span());
            let value = match compound {
                Some(op) => desugar_compound(&expr, op, value),
                None => value,
            };
            stmts.push(Stmt::Assign {
                target: expr,
                value,
                span,
            });
            false
        } else if self.check(TokenKind::RBrace) {
            *tail = Some(Box::new(expr));
            true
        } else if self.eat(TokenKind::Semicolon) {
            let span = expr.span;
            stmts.push(Stmt::Expr { expr, span });
            false
        } else {
            *tail = Some(Box::new(expr));
            true
        }
    }

    fn is_stmt_start(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Let
                | TokenKind::If
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Return
                | TokenKind::Break
                | TokenKind::Continue
                | TokenKind::Yield
                | TokenKind::Match
                | TokenKind::LBrace
        )
    }

    fn parse_stmt(&mut self) -> Option<Stmt> {
        match self.peek_kind() {
            TokenKind::Let => self.parse_let(),
            TokenKind::If => self.parse_if(),
            TokenKind::While => self.parse_while(),
            TokenKind::For => self.parse_for(),
            TokenKind::Return => self.parse_return(),
            TokenKind::Break | TokenKind::Continue | TokenKind::Yield => self.parse_jump(),
            TokenKind::Match => self.parse_match(),
            TokenKind::LBrace => self.parse_block_stmt(),
            _ => self.parse_expr_stmt(),
        }
    }

    fn parse_return(&mut self) -> Option<Stmt> {
        let start = self.expect(TokenKind::Return)?.span;
        let value = if self.check(TokenKind::Semicolon) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::Semicolon)?;
        Some(Stmt::Return {
            value,
            span: start.merge(self.prev_span()),
        })
    }

    fn parse_jump(&mut self) -> Option<Stmt> {
        let tok = self.bump();
        self.expect(TokenKind::Semicolon)?;
        let span = tok.span.merge(self.prev_span());
        Some(match tok.kind {
            TokenKind::Break => Stmt::Break { span },
            TokenKind::Yield => Stmt::Yield { span },
            _ => Stmt::Continue { span },
        })
    }

    fn parse_block_stmt(&mut self) -> Option<Stmt> {
        let block = self.parse_block()?;
        let span = block.span;
        Some(Stmt::Block { block, span })
    }

    fn parse_expr_stmt(&mut self) -> Option<Stmt> {
        let expr = self.parse_expr()?;
        let compound = compound_assign_op(self.peek_kind());
        if compound.is_some() || self.check(TokenKind::Eq) {
            self.bump();
            let value = self.parse_expr()?;
            self.expect(TokenKind::Semicolon)?;
            let span = expr.span.merge(self.prev_span());
            let value = match compound {
                Some(op) => desugar_compound(&expr, op, value),
                None => value,
            };
            Some(Stmt::Assign {
                target: expr,
                value,
                span,
            })
        } else {
            self.expect(TokenKind::Semicolon)?;
            let span = expr.span;
            Some(Stmt::Expr { expr, span })
        }
    }

    fn parse_let(&mut self) -> Option<Stmt> {
        let start = self.expect(TokenKind::Let)?.span;
        let mutable = self.eat(TokenKind::Mut);
        if self.check(TokenKind::LParen) {
            return self.parse_let_tuple(start, mutable);
        }
        let name = self.parse_ident()?;
        let ty = if self.eat(TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        let init = if self.eat(TokenKind::Eq) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::Semicolon)?;
        Some(Stmt::Let {
            mutable,
            name,
            ty,
            init,
            span: start.merge(self.prev_span()),
        })
    }

    /// `let (a, b, ...) = expr;`
    fn parse_let_tuple(&mut self, start: Span, mutable: bool) -> Option<Stmt> {
        let open = self.expect(TokenKind::LParen)?.span;
        let mut names = Vec::new();
        while !self.check(TokenKind::RParen) && !self.is_eof() {
            names.push(self.parse_ident()?);
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        let close = self.expect(TokenKind::RParen)?.span;
        if names.len() < 2 {
            self.error_at(
                "tuple patterns need at least two names",
                open.merge(close),
                Some("write `let (a, b) = t;`"),
            );
        }
        self.expect(TokenKind::Eq)?;
        let init = self.parse_expr()?;
        self.expect(TokenKind::Semicolon)?;
        Some(Stmt::LetTuple {
            mutable,
            names,
            init,
            span: start.merge(self.prev_span()),
        })
    }

    /// `match expr { Pattern => Block ,? ... }`
    fn parse_match(&mut self) -> Option<Stmt> {
        self.enter_nesting()?;
        let result = self.parse_match_inner();
        self.leave_nesting();
        result
    }

    fn parse_match_inner(&mut self) -> Option<Stmt> {
        let start = self.expect(TokenKind::Match)?.span;
        let scrutinee = self.parse_expr()?;
        self.expect(TokenKind::LBrace)?;
        let mut arms = Vec::new();
        while !self.check(TokenKind::RBrace) && !self.is_eof() {
            let pattern = self.parse_pattern()?;
            self.expect(TokenKind::FatArrow)?;
            let body = self.parse_block()?;
            let span = pattern.span.merge(body.span);
            arms.push(MatchArm {
                pattern,
                body,
                span,
            });
            self.eat(TokenKind::Comma);
        }
        let end = self.expect(TokenKind::RBrace)?.span;
        Some(Stmt::Match {
            scrutinee,
            arms,
            span: start.merge(end),
        })
    }

    /// Pattern ::= "_" | Ident | Ident "::" Ident ("(" Pattern,* ")")? | Literal
    fn parse_pattern(&mut self) -> Option<Pattern> {
        self.enter_nesting()?;
        let result = self.parse_pattern_inner();
        self.leave_nesting();
        result
    }

    fn parse_pattern_inner(&mut self) -> Option<Pattern> {
        match self.peek_kind() {
            TokenKind::Ident => {
                let id = self.parse_ident()?;
                if id.name == "_" {
                    return Some(Pattern {
                        kind: PatternKind::Wildcard,
                        span: id.span,
                    });
                }
                if self.eat(TokenKind::ColonColon) {
                    let variant = self.parse_ident()?;
                    let mut fields = Vec::new();
                    if self.eat(TokenKind::LParen) {
                        while !self.check(TokenKind::RParen) && !self.is_eof() {
                            fields.push(self.parse_pattern()?);
                            if !self.eat(TokenKind::Comma) {
                                break;
                            }
                        }
                        self.expect(TokenKind::RParen)?;
                    }
                    let span = id.span.merge(self.prev_span());
                    return Some(Pattern {
                        kind: PatternKind::Variant {
                            enum_name: id,
                            variant,
                            fields,
                        },
                        span,
                    });
                }
                Some(Pattern {
                    span: id.span,
                    kind: PatternKind::Binding(id),
                })
            }
            TokenKind::Int
            | TokenKind::Float
            | TokenKind::True
            | TokenKind::False
            | TokenKind::String
            | TokenKind::Char => {
                let e = self.parse_literal()?;
                match e.kind {
                    ExprKind::Literal(lit) => Some(Pattern {
                        kind: PatternKind::Literal(lit),
                        span: e.span,
                    }),
                    _ => None,
                }
            }
            TokenKind::Minus => {
                let start = self.bump().span;
                let e = self.parse_literal()?;
                let lit = match e.kind {
                    ExprKind::Literal(Literal::Int(v)) => Literal::Int(v.wrapping_neg()),
                    ExprKind::Literal(Literal::Float(v)) => Literal::Float(-v),
                    _ => {
                        self.error_at("expected a number after `-` in pattern", e.span, None);
                        return None;
                    }
                };
                Some(Pattern {
                    kind: PatternKind::Literal(lit),
                    span: start.merge(e.span),
                })
            }
            _ => {
                let tok = self.peek().clone();
                self.error_at(
                    format!("expected pattern, found `{}`", tok.lexeme),
                    tok.span,
                    Some("patterns: `_`, a name, a literal, or `Enum::Variant(a, _)`"),
                );
                None
            }
        }
    }

    fn parse_if(&mut self) -> Option<Stmt> {
        // `else if` chains recurse here without going through a block, so
        // they count towards the nesting limit as well.
        self.enter_nesting()?;
        let result = self.parse_if_inner();
        self.leave_nesting();
        result
    }

    fn parse_if_inner(&mut self) -> Option<Stmt> {
        let start = self.expect(TokenKind::If)?.span;
        if self.check(TokenKind::Let) {
            return self.parse_if_let(start);
        }
        let cond = self.parse_expr()?;
        let then_block = self.parse_block()?;
        let else_block = self.parse_else()?;
        let end = else_block
            .as_ref()
            .map(|b| b.span)
            .unwrap_or(then_block.span);
        Some(Stmt::If {
            cond,
            then_block,
            else_block,
            span: start.merge(end),
        })
    }

    /// Optional `else Block` / `else if ...` (the latter wrapped in a block).
    fn parse_else(&mut self) -> Option<Option<Block>> {
        if !self.eat(TokenKind::Else) {
            return Some(None);
        }
        if self.check(TokenKind::If) {
            let inner = self.parse_if()?;
            let span = inner.span();
            return Some(Some(Block {
                stmts: vec![inner],
                tail: None,
                span,
            }));
        }
        Some(Some(self.parse_block()?))
    }

    /// `if let Pattern = expr Block (else Block)?` → a two-arm `match`
    /// whose second arm is `_` (an empty block when there is no `else`).
    fn parse_if_let(&mut self, start: Span) -> Option<Stmt> {
        self.expect(TokenKind::Let)?;
        let pattern = self.parse_pattern()?;
        self.expect(TokenKind::Eq)?;
        let scrutinee = self.parse_expr()?;
        let then_block = self.parse_block()?;
        let else_block = self.parse_else()?;
        let end = else_block
            .as_ref()
            .map(|b| b.span)
            .unwrap_or(then_block.span);
        let else_body = else_block.unwrap_or(Block {
            stmts: Vec::new(),
            tail: None,
            span: end,
        });
        let arms = vec![
            MatchArm {
                span: pattern.span.merge(then_block.span),
                pattern,
                body: then_block,
            },
            MatchArm {
                pattern: Pattern {
                    kind: PatternKind::Wildcard,
                    span: else_body.span,
                },
                span: else_body.span,
                body: else_body,
            },
        ];
        Some(Stmt::Match {
            scrutinee,
            arms,
            span: start.merge(end),
        })
    }

    fn parse_while(&mut self) -> Option<Stmt> {
        let start = self.expect(TokenKind::While)?.span;
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        let span = start.merge(body.span);
        Some(Stmt::While { cond, body, span })
    }

    fn parse_for(&mut self) -> Option<Stmt> {
        let start = self.expect(TokenKind::For)?.span;
        let var = self.parse_ident()?;
        self.expect(TokenKind::In)?;
        let start_e = self.parse_expr()?;
        self.expect(TokenKind::DotDot)?;
        let end_e = self.parse_expr()?;
        let body = self.parse_block()?;
        let span = start.merge(body.span);
        Some(Stmt::For {
            var,
            start: start_e,
            end: end_e,
            body,
            span,
        })
    }

    // ----- Pratt expression parser -----

    fn parse_expr(&mut self) -> Option<Expr> {
        self.parse_prec(0)
    }

    fn parse_prec(&mut self, min_prec: u8) -> Option<Expr> {
        self.enter_nesting()?;
        let result = self.parse_prec_inner(min_prec);
        self.leave_nesting();
        result
    }

    fn parse_prec_inner(&mut self, min_prec: u8) -> Option<Expr> {
        let lhs = self.parse_prefix()?;
        self.parse_infix(lhs, min_prec)
    }

    fn parse_infix(&mut self, mut lhs: Expr, min_prec: u8) -> Option<Expr> {
        loop {
            let kind = self.peek_kind();
            if kind == TokenKind::As {
                if min_prec > PREC_CAST {
                    break;
                }
                lhs = self.parse_cast(lhs)?;
                continue;
            }
            if let Some((prec, right_assoc, op)) = infix_info(kind) {
                if prec < min_prec {
                    break;
                }
                self.bump();
                let next_min = if right_assoc { prec } else { prec + 1 };
                lhs = self.parse_binary_rhs(lhs, op, next_min)?;
                continue;
            }
            break;
        }
        Some(lhs)
    }

    fn parse_cast(&mut self, lhs: Expr) -> Option<Expr> {
        self.expect(TokenKind::As)?;
        let ty = self.parse_type()?;
        let span = lhs.span.merge(ty.span);
        Some(Expr {
            kind: ExprKind::Cast {
                expr: Box::new(lhs),
                ty,
            },
            span,
        })
    }

    fn parse_binary_rhs(&mut self, lhs: Expr, op: BinOp, next_min: u8) -> Option<Expr> {
        let rhs = self.parse_prec(next_min)?;
        let span = lhs.span.merge(rhs.span);
        Some(Expr {
            kind: ExprKind::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            span,
        })
    }

    fn parse_prefix(&mut self) -> Option<Expr> {
        match self.peek_kind() {
            TokenKind::Int
            | TokenKind::Float
            | TokenKind::True
            | TokenKind::False
            | TokenKind::String
            | TokenKind::Char => self.parse_literal(),
            TokenKind::Ident => self.parse_ident_expr(),
            TokenKind::LParen => self.parse_paren(),
            TokenKind::LBracket => self.parse_array_lit(),
            TokenKind::Minus => self.parse_unary(UnOp::Neg),
            TokenKind::Bang => self.parse_unary(UnOp::Not),
            _ => self.parse_prefix_error(),
        }
    }

    fn parse_literal(&mut self) -> Option<Expr> {
        let tok = self.bump();
        let lit = match tok.kind {
            TokenKind::Int => {
                let value = parse_int(&tok.lexeme);
                if value.is_none() {
                    self.error_at(
                        "invalid integer literal",
                        tok.span,
                        Some("integer literals must fit in 64 bits; forms: 255, 1_000, 0xFF, 0b1010, 0o17"),
                    );
                }
                Literal::Int(value.unwrap_or(0))
            }
            TokenKind::Float => Literal::Float(parse_float(&tok.lexeme).unwrap_or(0.0)),
            TokenKind::True => Literal::Bool(true),
            TokenKind::False => Literal::Bool(false),
            TokenKind::String => {
                let (s, bad) = unescape_string(&tok.lexeme);
                if bad {
                    self.bad_unicode_escape(tok.span);
                }
                Literal::String(s)
            }
            _ => {
                let (c, bad) = unescape_char(&tok.lexeme);
                if bad {
                    self.bad_unicode_escape(tok.span);
                }
                Literal::Char(c)
            }
        };
        Some(Expr {
            kind: ExprKind::Literal(lit),
            span: tok.span,
        })
    }

    fn parse_ident_expr(&mut self) -> Option<Expr> {
        let id = self.parse_ident()?;
        // enum variant: Ident '::' Ident ('(' args ')')?
        if self.eat(TokenKind::ColonColon) {
            let variant = self.parse_ident()?;
            let mut args = Vec::new();
            if self.eat(TokenKind::LParen) {
                while !self.check(TokenKind::RParen) && !self.is_eof() {
                    args.push(self.parse_expr()?);
                    if !self.eat(TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(TokenKind::RParen)?;
            }
            let expr = Expr {
                span: id.span.merge(self.prev_span()),
                kind: ExprKind::EnumLit {
                    enum_name: id,
                    variant,
                    args,
                },
            };
            return self.parse_postfix(expr);
        }
        // struct literal: Ident '{' field: expr, ... '}'
        if self.check(TokenKind::LBrace) {
            // Ambiguous with block after `if cond`. We only parse a
            // struct literal when the next token after `{` looks like
            // `ident :`.
            if self.looks_like_struct_lit() {
                return self.finish_struct_lit(id);
            }
        }
        let expr = Expr {
            span: id.span,
            kind: ExprKind::Ident(id),
        };
        self.parse_postfix(expr)
    }

    fn parse_paren(&mut self) -> Option<Expr> {
        let start = self.expect(TokenKind::LParen)?.span;
        if self.check(TokenKind::RParen) {
            let end = self.bump().span;
            return Some(Expr {
                kind: ExprKind::Literal(Literal::Unit),
                span: start.merge(end),
            });
        }
        let inner = self.parse_expr()?;
        if self.check(TokenKind::Comma) {
            // tuple: (a, b, ...) — a trailing comma is allowed after two elements
            let mut elements = vec![inner];
            while self.eat(TokenKind::Comma) {
                if self.check(TokenKind::RParen) {
                    break;
                }
                elements.push(self.parse_expr()?);
            }
            let end = self.expect(TokenKind::RParen)?.span;
            let span = start.merge(end);
            if elements.len() < 2 {
                self.error_at(
                    "tuples need at least two elements",
                    span,
                    Some("write `(a, b)`; `(a)` is just `a`"),
                );
            }
            let expr = Expr {
                kind: ExprKind::Tuple { elements },
                span,
            };
            return self.parse_postfix(expr);
        }
        let end = self.expect(TokenKind::RParen)?.span;
        let expr = Expr {
            kind: ExprKind::Group(Box::new(inner)),
            span: start.merge(end),
        };
        self.parse_postfix(expr)
    }

    fn parse_array_lit(&mut self) -> Option<Expr> {
        let start = self.expect(TokenKind::LBracket)?.span;
        let mut elements = Vec::new();
        if !self.check(TokenKind::RBracket) {
            loop {
                elements.push(self.parse_expr()?);
                if !self.eat(TokenKind::Comma) {
                    break;
                }
                if self.check(TokenKind::RBracket) {
                    break;
                }
            }
        }
        let end = self.expect(TokenKind::RBracket)?.span;
        let expr = Expr {
            kind: ExprKind::Array { elements },
            span: start.merge(end),
        };
        self.parse_postfix(expr)
    }

    fn parse_unary(&mut self, op: UnOp) -> Option<Expr> {
        let start = self.bump().span;
        let expr = self.parse_prec(PREC_UNARY)?;
        let span = start.merge(expr.span);
        Some(Expr {
            kind: ExprKind::Unary {
                op,
                expr: Box::new(expr),
            },
            span,
        })
    }

    fn parse_prefix_error(&mut self) -> Option<Expr> {
        let tok = self.peek().clone();
        if matches!(
            tok.kind,
            TokenKind::TyI32
                | TokenKind::TyI64
                | TokenKind::TyF64
                | TokenKind::TyBool
                | TokenKind::TyString
                | TokenKind::TyUnit
        ) {
            // type names used as identifiers should not appear in expr
            self.bump();
            self.error_at(
                format!("type name `{}` is not a valid expression", tok.lexeme),
                tok.span,
                Some("use a variable or literal here"),
            );
        } else {
            self.error_at(
                format!("expected expression, found `{}`", tok.lexeme),
                tok.span,
                None,
            );
        }
        None
    }

    fn parse_postfix(&mut self, mut expr: Expr) -> Option<Expr> {
        loop {
            match self.peek_kind() {
                TokenKind::LParen => {
                    self.bump();
                    let mut args = Vec::new();
                    if !self.check(TokenKind::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if !self.eat(TokenKind::Comma) {
                                break;
                            }
                            if self.check(TokenKind::RParen) {
                                break;
                            }
                        }
                    }
                    let end = self.expect(TokenKind::RParen)?.span;
                    let span = expr.span.merge(end);
                    expr = Expr {
                        kind: ExprKind::Call {
                            callee: Box::new(expr),
                            args,
                        },
                        span,
                    };
                }
                TokenKind::LBracket => {
                    self.bump();
                    let index = self.parse_expr()?;
                    let end = self.expect(TokenKind::RBracket)?.span;
                    let span = expr.span.merge(end);
                    expr = Expr {
                        kind: ExprKind::Index {
                            base: Box::new(expr),
                            index: Box::new(index),
                        },
                        span,
                    };
                }
                TokenKind::Dot => {
                    self.bump();
                    // `t.0`: the lexer yields an integer after the dot;
                    // `t.0.1` lexes as the float `0.1`, split here.
                    let fields: Vec<Ident> = match self.peek_kind() {
                        TokenKind::Int | TokenKind::Float => {
                            let tok = self.bump();
                            let mut parts = Vec::new();
                            for part in tok.lexeme.split('.') {
                                match part.parse::<usize>() {
                                    Ok(i) if !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()) => {
                                        parts.push(Ident::new(i.to_string(), tok.span));
                                    }
                                    _ => {
                                        self.error_at(
                                            format!("invalid tuple index `{}`", tok.lexeme),
                                            tok.span,
                                            Some("tuple fields are accessed as `t.0`, `t.1`, ..."),
                                        );
                                        return None;
                                    }
                                }
                            }
                            parts
                        }
                        _ => vec![self.parse_ident()?],
                    };
                    for field in fields {
                        let span = expr.span.merge(field.span);
                        expr = Expr {
                            kind: ExprKind::Field {
                                base: Box::new(expr),
                                field,
                            },
                            span,
                        };
                    }
                }
                _ => break,
            }
        }
        Some(expr)
    }

    fn looks_like_struct_lit(&self) -> bool {
        // `{` then `ident` then `:`
        if self.pos + 2 >= self.tokens.len() {
            return false;
        }
        self.tokens[self.pos].kind == TokenKind::LBrace
            && self.tokens[self.pos + 1].kind == TokenKind::Ident
            && self.tokens[self.pos + 2].kind == TokenKind::Colon
    }

    fn finish_struct_lit(&mut self, name: Ident) -> Option<Expr> {
        self.expect(TokenKind::LBrace)?;
        let mut fields = Vec::new();
        while !self.check(TokenKind::RBrace) && !self.is_eof() {
            let fname = self.parse_ident()?;
            self.expect(TokenKind::Colon)?;
            let value = self.parse_expr()?;
            fields.push((fname, value));
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        let end = self.expect(TokenKind::RBrace)?.span;
        Some(Expr {
            span: name.span.merge(end),
            kind: ExprKind::StructLit { name, fields },
        })
    }

    fn parse_type(&mut self) -> Option<TypeExpr> {
        // `[[[[...` array types recurse too.
        self.enter_nesting()?;
        let result = self.parse_type_inner();
        self.leave_nesting();
        result
    }

    fn parse_type_inner(&mut self) -> Option<TypeExpr> {
        match self.peek_kind() {
            TokenKind::TyI32
            | TokenKind::TyI64
            | TokenKind::TyF64
            | TokenKind::TyBool
            | TokenKind::TyString
            | TokenKind::TyUnit
            | TokenKind::Ident => {
                let tok = self.bump();
                let kind = if tok.kind == TokenKind::TyUnit {
                    TypeExprKind::Unit
                } else {
                    TypeExprKind::Named(tok.lexeme)
                };
                Some(TypeExpr {
                    kind,
                    span: tok.span,
                })
            }
            TokenKind::LBracket => {
                let start = self.bump().span;
                let elem = Box::new(self.parse_type()?);
                self.expect(TokenKind::Semicolon)?;
                let len_tok = self.expect(TokenKind::Int)?;
                let len = parse_int(&len_tok.lexeme).unwrap_or(0);
                let end = self.expect(TokenKind::RBracket)?.span;
                Some(TypeExpr {
                    kind: TypeExprKind::Array { elem, len },
                    span: start.merge(end),
                })
            }
            TokenKind::LParen => {
                let start = self.bump().span;
                if self.eat(TokenKind::RParen) {
                    return Some(TypeExpr::unit(start.merge(self.prev_span())));
                }
                let mut elems = Vec::new();
                while !self.check(TokenKind::RParen) && !self.is_eof() {
                    elems.push(self.parse_type()?);
                    if !self.eat(TokenKind::Comma) {
                        break;
                    }
                }
                let end = self.expect(TokenKind::RParen)?.span;
                let span = start.merge(end);
                if elems.len() < 2 {
                    self.error_at(
                        "tuple types need at least two elements",
                        span,
                        Some("write `(T1, T2)`; `()` is the unit type"),
                    );
                    return None;
                }
                Some(TypeExpr {
                    kind: TypeExprKind::Tuple(elems),
                    span,
                })
            }
            _ => {
                let tok = self.peek().clone();
                self.error_at(
                    format!("expected type, found `{}`", tok.lexeme),
                    tok.span,
                    Some("valid types: i32, i64, f64, bool, string, unit, [T; N], (T1, T2), or a struct/enum name"),
                );
                None
            }
        }
    }

    fn parse_ident(&mut self) -> Option<Ident> {
        if self.check(TokenKind::Ident) {
            let tok = self.bump();
            Some(Ident::new(tok.lexeme, tok.span))
        } else {
            let tok = self.peek().clone();
            self.error_at(
                format!("expected identifier, found `{}`", tok.lexeme),
                tok.span,
                None,
            );
            None
        }
    }

    // ----- helpers -----

    fn expect(&mut self, kind: TokenKind) -> Option<Token> {
        if self.check(kind) {
            Some(self.bump())
        } else {
            let tok = self.peek().clone();
            self.error_at(
                format!("expected {}, found `{}`", kind.as_str(), tok.lexeme),
                tok.span,
                None,
            );
            None
        }
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.check(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn check(&self, kind: TokenKind) -> bool {
        self.peek_kind() == kind
    }

    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .or_else(|| self.tokens.last())
            .expect("token stream missing EOF")
    }

    fn peek_kind(&self) -> TokenKind {
        self.peek().kind
    }

    fn peek_span(&self) -> Span {
        self.peek().span
    }

    fn prev_span(&self) -> Span {
        if self.pos == 0 {
            self.peek_span()
        } else {
            self.tokens[self.pos - 1].span
        }
    }

    fn bump(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.pos < self.tokens.len() && self.tokens[self.pos].kind != TokenKind::Eof {
            self.pos += 1;
        }
        tok
    }

    fn is_eof(&self) -> bool {
        self.peek_kind() == TokenKind::Eof
    }

    fn synchronize_stmt(&mut self) {
        while !self.is_eof() {
            if self.eat(TokenKind::Semicolon) {
                return;
            }
            if matches!(
                self.peek_kind(),
                TokenKind::RBrace
                    | TokenKind::Let
                    | TokenKind::If
                    | TokenKind::While
                    | TokenKind::For
                    | TokenKind::Return
                    | TokenKind::Match
                    | TokenKind::Fn
                    | TokenKind::Struct
                    | TokenKind::Enum
            ) {
                return;
            }
            self.bump();
        }
    }

    fn error_at(&mut self, message: impl Into<String>, span: Span, help: Option<&str>) {
        let mut d = Diagnostic::error(message, span).with_code("E0100");
        if let Some(h) = help {
            d = d.with_help(h);
        }
        self.diags.push(d);
    }

    fn bad_unicode_escape(&mut self, span: Span) {
        self.diags.push(
            Diagnostic::error("invalid unicode escape", span)
                .with_code("E0005")
                .with_help("write `\\u{XXXX}` with 1 to 6 hex digits naming a Unicode scalar value"),
        );
    }
}

/// Binding power of `as`: above arithmetic, below prefix operators.
const PREC_CAST: u8 = 11;
/// Binding power of the prefix operators `-` and `!`.
const PREC_UNARY: u8 = 12;

/// Precedence table (`docs/language.md`), loosest first:
/// `||` 1, `&&` 2, `== !=` 3, `< <= > >=` 4, `|` 5, `^` 6, `&` 7, `<< >>` 8,
/// `+ -` 9, `* / %` 10, `as` 11, prefix 12. All infix operators are
/// left-associative.
fn infix_info(kind: TokenKind) -> Option<(u8, bool, BinOp)> {
    // precedence, right-associative, op
    match kind {
        TokenKind::PipePipe => Some((1, false, BinOp::Or)),
        TokenKind::AmpAmp => Some((2, false, BinOp::And)),
        TokenKind::EqEq => Some((3, false, BinOp::Eq)),
        TokenKind::BangEq => Some((3, false, BinOp::Ne)),
        TokenKind::Lt => Some((4, false, BinOp::Lt)),
        TokenKind::LtEq => Some((4, false, BinOp::Le)),
        TokenKind::Gt => Some((4, false, BinOp::Gt)),
        TokenKind::GtEq => Some((4, false, BinOp::Ge)),
        TokenKind::Pipe => Some((5, false, BinOp::BitOr)),
        TokenKind::Caret => Some((6, false, BinOp::BitXor)),
        TokenKind::Amp => Some((7, false, BinOp::BitAnd)),
        TokenKind::Shl => Some((8, false, BinOp::Shl)),
        TokenKind::Shr => Some((8, false, BinOp::Shr)),
        TokenKind::Plus => Some((9, false, BinOp::Add)),
        TokenKind::Minus => Some((9, false, BinOp::Sub)),
        TokenKind::Star => Some((10, false, BinOp::Mul)),
        TokenKind::Slash => Some((10, false, BinOp::Div)),
        TokenKind::Percent => Some((10, false, BinOp::Rem)),
        _ => None,
    }
}

/// The binary operator behind a compound-assignment token (`+=` → `+`).
fn compound_assign_op(kind: TokenKind) -> Option<BinOp> {
    Some(match kind {
        TokenKind::PlusEq => BinOp::Add,
        TokenKind::MinusEq => BinOp::Sub,
        TokenKind::StarEq => BinOp::Mul,
        TokenKind::SlashEq => BinOp::Div,
        TokenKind::PercentEq => BinOp::Rem,
        TokenKind::AmpEq => BinOp::BitAnd,
        TokenKind::PipeEq => BinOp::BitOr,
        TokenKind::CaretEq => BinOp::BitXor,
        TokenKind::ShlEq => BinOp::Shl,
        TokenKind::ShrEq => BinOp::Shr,
        _ => return None,
    })
}

/// `target op= value` is sugar for `target = target op value`. The target
/// is evaluated twice (once as a value, once as a place), so a call inside
/// an index expression (`a[f()] += 1`) runs twice; documented in
/// `docs/language.md`.
fn desugar_compound(target: &Expr, op: BinOp, value: Expr) -> Expr {
    let span = target.span.merge(value.span);
    Expr {
        kind: ExprKind::Binary {
            op,
            lhs: Box::new(target.clone()),
            rhs: Box::new(value),
        },
        span,
    }
}

/// Decimal, `0x`, `0b` or `0o` with optional `_` separators; the value must
/// fit in `i64` (hex and friends denote values, not bit patterns, so
/// `0xFFFF_FFFF_FFFF_FFFF` is rejected).
fn parse_int(lexeme: &str) -> Option<i64> {
    let digits: String = lexeme.chars().filter(|c| *c != '_').collect();
    let (radix, body) = match digits.get(..2) {
        Some("0x") => (16, &digits[2..]),
        Some("0b") => (2, &digits[2..]),
        Some("0o") => (8, &digits[2..]),
        _ => (10, digits.as_str()),
    };
    i64::from_str_radix(body, radix).ok()
}

fn parse_float(lexeme: &str) -> Option<f64> {
    let digits: String = lexeme.chars().filter(|c| *c != '_').collect();
    digits.parse::<f64>().ok()
}

/// Returns the decoded string and whether an invalid `\u{...}` was seen.
fn unescape_string(lexeme: &str) -> (String, bool) {
    let inner = if lexeme.len() >= 2 && lexeme.starts_with('"') && lexeme.ends_with('"') {
        &lexeme[1..lexeme.len() - 1]
    } else if lexeme.starts_with('"') {
        &lexeme[1..]
    } else {
        lexeme
    };
    unescape(inner)
}

fn unescape_char(lexeme: &str) -> (char, bool) {
    let inner = if lexeme.len() >= 2 && lexeme.starts_with('\'') && lexeme.ends_with('\'') {
        &lexeme[1..lexeme.len() - 1]
    } else {
        lexeme
    };
    let (s, bad) = unescape(inner);
    (s.chars().next().unwrap_or('\0'), bad)
}

/// Escapes: `\n \t \r \0 \\ \" \'` and `\u{XXXX}` (1–6 hex digits naming a
/// Unicode scalar value). A malformed `\u{...}` decodes to U+FFFD and sets
/// the flag so the parser can report E0005 and carry on. Unknown escapes
/// are kept verbatim.
fn unescape(s: &str) -> (String, bool) {
    let mut out = String::new();
    let mut bad = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('0') => out.push('\0'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('\'') => out.push('\''),
                Some('u') if chars.peek() == Some(&'{') => {
                    chars.next();
                    let mut hex = String::new();
                    let mut closed = false;
                    while let Some(&h) = chars.peek() {
                        chars.next();
                        if h == '}' {
                            closed = true;
                            break;
                        }
                        hex.push(h);
                    }
                    let decoded = if closed && !hex.is_empty() && hex.len() <= 6 {
                        u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32)
                    } else {
                        None
                    };
                    match decoded {
                        Some(ch) => out.push(ch),
                        None => {
                            bad = true;
                            out.push('\u{FFFD}');
                        }
                    }
                }
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    (out, bad)
}

pub fn parse(tokens: Vec<Token>) -> (Program, Diagnostics) {
    Parser::new(tokens).parse_program()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;
    use crate::span::FileId;

    fn parse_src(src: &str) -> (Program, Diagnostics) {
        let (toks, lex_diags) = tokenize(FileId(0), src);
        let (prog, mut diags) = parse(toks);
        diags.extend(lex_diags);
        (prog, diags)
    }

    #[test]
    fn parses_function_and_let() {
        let src = r#"
            fn add(a: i32, b: i32) -> i32 {
                let c = a + b * 2;
                return c;
            }
        "#;
        let (prog, diags) = parse_src(src);
        assert!(!diags.has_errors(), "{}", diags.render(&crate::span::Session::new(), false));
        assert_eq!(prog.items.len(), 1);
        match &prog.items[0] {
            Item::Fn(f) => {
                assert_eq!(f.name.name, "add");
                assert_eq!(f.params.len(), 2);
            }
            _ => panic!("expected fn"),
        }
    }

    #[test]
    fn precedence_mul_over_add() {
        let src = "fn main() -> i32 { return 1 + 2 * 3; }";
        let (prog, diags) = parse_src(src);
        assert!(!diags.has_errors());
        let Item::Fn(f) = &prog.items[0] else { panic!() };
        let body = f.body.as_ref().unwrap();
        let Stmt::Return { value: Some(e), .. } = &body.stmts[0] else { panic!() };
        match &e.kind {
            ExprKind::Binary { op: BinOp::Add, rhs, .. } => match &rhs.kind {
                ExprKind::Binary { op: BinOp::Mul, .. } => {}
                other => panic!("expected mul on rhs, got {other:?}"),
            },
            other => panic!("expected add, got {other:?}"),
        }
    }

    #[test]
    fn reports_missing_paren() {
        let src = "fn main( -> i32 { return 0; }";
        let (_, diags) = parse_src(src);
        assert!(diags.has_errors());
    }

    fn nested_parens(n: usize) -> String {
        format!(
            "fn main() -> i32 {{ let x = {}1{}; return x; }}",
            "(".repeat(n),
            ")".repeat(n)
        )
    }

    fn render(diags: &Diagnostics) -> String {
        diags.render(&crate::span::Session::new(), false)
    }

    #[test]
    fn nesting_100_parens_is_fine() {
        let (prog, diags) = parse_src(&nested_parens(100));
        assert!(!diags.has_errors(), "{}", render(&diags));
        assert_eq!(prog.items.len(), 1);
    }

    #[test]
    fn nesting_100k_parens_reports_e0101_once() {
        let (_, diags) = parse_src(&nested_parens(100_000));
        assert!(diags.has_errors());
        let e0101 = diags.iter().filter(|d| d.code == Some("E0101")).count();
        assert_eq!(e0101, 1, "{}", render(&diags));
        let out = render(&diags);
        assert!(out.contains("nesting too deep (limit 256)"), "{out}");
    }

    #[test]
    fn nesting_100k_blocks_reports_e0101_once() {
        let src = format!(
            "fn main() -> i32 {{ {} return 0; {} }}",
            "{".repeat(100_000),
            "}".repeat(100_000)
        );
        let (_, diags) = parse_src(&src);
        assert!(diags.has_errors());
        let e0101 = diags.iter().filter(|d| d.code == Some("E0101")).count();
        assert_eq!(e0101, 1, "{}", render(&diags));
    }

    #[test]
    fn nesting_100k_else_if_and_array_types_do_not_crash() {
        let mut src = String::from("fn main() -> i32 { ");
        for _ in 0..100_000 {
            src.push_str("if true { } else ");
        }
        src.push_str("{ } return 0; }");
        let (_, diags) = parse_src(&src);
        assert!(diags.iter().any(|d| d.code == Some("E0101")));

        let src = format!(
            "fn main() -> i32 {{ let a: {}i32{}; return 0; }}",
            "[".repeat(100_000),
            "; 1]".repeat(100_000)
        );
        let (_, diags) = parse_src(&src);
        assert!(diags.iter().any(|d| d.code == Some("E0101")));
    }

    #[test]
    fn nesting_near_limit_end_to_end() {
        // The parser itself fits 256 levels in a 2 MiB test thread, but
        // sema's `check_expr` frame is several KiB in debug builds and
        // overflows 2 MiB somewhere between 128 and 256 `Group` levels, so
        // the full pipeline runs on an explicit 32 MiB stack here (the CLI's
        // main thread has 8 MiB and release frames are far smaller).
        let handle = std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let src = nested_parens(MAX_NESTING - 8);
                let (v, _, _) = crate::run_source("deep.ae", &src, 0).expect("-O0");
                assert_eq!(v, crate::vm::Value::I32(1));
                let (v, _, _) = crate::run_source("deep.ae", &src, 2).expect("-O2");
                assert_eq!(v, crate::vm::Value::I32(1));
                let (prog, diags) = parse_src(&src);
                assert!(!diags.has_errors());
                let pretty = crate::pretty::pretty_program(&prog);
                assert!(pretty.contains("fn main"));
                let dumped = crate::ast::dump_program(&prog);
                assert!(dumped.contains("main"));
            })
            .unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn unterminated_block_before_next_item_terminates() {
        // `fn` inside a block used to make statement recovery spin forever
        // (and allocate a diagnostic per spin). The block must end, the
        // next item must still parse, and diagnostics must stay bounded.
        let src = "fn h0(p0: i32) -> i32 {\n    return 8;\n\nfn main() -> i32 {\n    let mut acc = 0;\n    acc = h0(acc);\n   \u{fffd}return acc;\n}\n";
        let (prog, diags) = parse_src(src);
        assert!(diags.has_errors());
        assert!(diags.error_count() < 16, "{}", render(&diags));
        assert_eq!(prog.items.len(), 2, "{}", render(&diags));
        for src in [
            "fn main() -> i32 { struct S { x: i32 }",
            "fn main() -> i32 { extern fn f();",
            "fn main() -> i32 { let x = 1; } }",
            "fn main() -> i32 { else else else }",
        ] {
            let (_, diags) = parse_src(src);
            assert!(diags.error_count() < 16, "{src}: {}", render(&diags));
        }
    }

    #[test]
    fn nesting_limit_parses_on_default_test_stack() {
        // Exactly at the limit the parser must neither crash nor report.
        // The fn body block and the `let` initializer each take one level.
        let (prog, diags) = parse_src(&nested_parens(MAX_NESTING - 2));
        assert!(!diags.has_errors(), "{}", render(&diags));
        assert_eq!(prog.items.len(), 1);
        let (_, diags) = parse_src(&nested_parens(MAX_NESTING - 1));
        assert!(diags.iter().any(|d| d.code == Some("E0101")));
    }

    fn first_stmt(src: &str) -> Stmt {
        let (prog, diags) = parse_src(src);
        assert!(!diags.has_errors(), "{}", render(&diags));
        let Item::Fn(f) = &prog.items[0] else { panic!() };
        f.body.as_ref().unwrap().stmts[0].clone()
    }

    fn return_expr(src: &str) -> Expr {
        let Stmt::Return { value: Some(e), .. } = first_stmt(src) else { panic!() };
        e
    }

    #[test]
    fn compound_assignment_desugars_to_assign_with_binary() {
        for (src, op) in [
            ("fn main() -> i32 { x += 1; }", BinOp::Add),
            ("fn main() -> i32 { x -= 1; }", BinOp::Sub),
            ("fn main() -> i32 { x *= 1; }", BinOp::Mul),
            ("fn main() -> i32 { x /= 1; }", BinOp::Div),
            ("fn main() -> i32 { x %= 1; }", BinOp::Rem),
            ("fn main() -> i32 { x &= 1; }", BinOp::BitAnd),
            ("fn main() -> i32 { x |= 1; }", BinOp::BitOr),
            ("fn main() -> i32 { x ^= 1; }", BinOp::BitXor),
            ("fn main() -> i32 { x <<= 1; }", BinOp::Shl),
            ("fn main() -> i32 { x >>= 1; }", BinOp::Shr),
        ] {
            let Stmt::Assign { target, value, .. } = first_stmt(src) else {
                panic!("{src}")
            };
            assert!(matches!(target.kind, ExprKind::Ident(_)), "{src}");
            let ExprKind::Binary { op: got, lhs, .. } = &value.kind else {
                panic!("{src}")
            };
            assert_eq!(*got, op, "{src}");
            assert_eq!(**lhs, target, "{src}");
        }
        // `a[i] += v` and `p.x -= v` keep the full place expression as lhs;
        // the right operand binds the whole expression (`x += 1 + 2`).
        let Stmt::Assign { target, value, .. } =
            first_stmt("fn main() -> i32 { a[i] += 1 + 2; }")
        else {
            panic!()
        };
        assert!(matches!(target.kind, ExprKind::Index { .. }));
        let ExprKind::Binary { op: BinOp::Add, lhs, rhs } = &value.kind else { panic!() };
        assert_eq!(**lhs, target);
        assert!(matches!(rhs.kind, ExprKind::Binary { op: BinOp::Add, .. }));
        let Stmt::Assign { target, .. } = first_stmt("fn main() -> i32 { p.x -= 2; }") else {
            panic!()
        };
        assert!(matches!(target.kind, ExprKind::Field { .. }));
        // also when the statement is the last thing in a block
        let (prog, diags) = parse_src("fn main() -> i32 { let mut x = 1; x += 2; }");
        assert!(!diags.has_errors(), "{}", render(&diags));
        let Item::Fn(f) = &prog.items[0] else { panic!() };
        assert_eq!(f.body.as_ref().unwrap().stmts.len(), 2);
    }

    #[test]
    fn bitwise_precedence_is_rust_like() {
        // `|` 5 < `^` 6 < `&` 7 < `<< >>` 8 < `+ -` 9, all tighter than `==`.
        let e = return_expr("fn main() -> i32 { return 1 | 2 & 3 == 3; }");
        let ExprKind::Binary { op: BinOp::Eq, lhs, .. } = &e.kind else { panic!("{e:?}") };
        let ExprKind::Binary { op: BinOp::BitOr, rhs, .. } = &lhs.kind else { panic!("{e:?}") };
        assert!(matches!(rhs.kind, ExprKind::Binary { op: BinOp::BitAnd, .. }));

        let e = return_expr("fn main() -> i32 { return a ^ b & c | d; }");
        let ExprKind::Binary { op: BinOp::BitOr, lhs, .. } = &e.kind else { panic!("{e:?}") };
        let ExprKind::Binary { op: BinOp::BitXor, rhs, .. } = &lhs.kind else { panic!("{e:?}") };
        assert!(matches!(rhs.kind, ExprKind::Binary { op: BinOp::BitAnd, .. }));

        // `1 << 2 + 3` is `1 << (2 + 3)`; `a & b << 1` is `a & (b << 1)`
        let e = return_expr("fn main() -> i32 { return 1 << 2 + 3; }");
        let ExprKind::Binary { op: BinOp::Shl, rhs, .. } = &e.kind else { panic!("{e:?}") };
        assert!(matches!(rhs.kind, ExprKind::Binary { op: BinOp::Add, .. }));
        let e = return_expr("fn main() -> i32 { return a & b << 1; }");
        let ExprKind::Binary { op: BinOp::BitAnd, rhs, .. } = &e.kind else { panic!("{e:?}") };
        assert!(matches!(rhs.kind, ExprKind::Binary { op: BinOp::Shl, .. }));

        // shifts are left-associative; `as` binds tighter than `<<`; `!` tighter than `as`
        let e = return_expr("fn main() -> i32 { return a >> 1 >> 2; }");
        let ExprKind::Binary { op: BinOp::Shr, lhs, .. } = &e.kind else { panic!("{e:?}") };
        assert!(matches!(lhs.kind, ExprKind::Binary { op: BinOp::Shr, .. }));
        let e = return_expr("fn main() -> i32 { return a << b as i32; }");
        let ExprKind::Binary { op: BinOp::Shl, rhs, .. } = &e.kind else { panic!("{e:?}") };
        assert!(matches!(rhs.kind, ExprKind::Cast { .. }));
        let e = return_expr("fn main() -> i32 { return !a as i64; }");
        let ExprKind::Cast { expr, .. } = &e.kind else { panic!("{e:?}") };
        assert!(matches!(expr.kind, ExprKind::Unary { op: UnOp::Not, .. }));
        // `&&`/`||` still loosest
        let e = return_expr("fn main() -> i32 { return a & b && c | d; }");
        let ExprKind::Binary { op: BinOp::And, lhs, rhs } = &e.kind else { panic!("{e:?}") };
        assert!(matches!(lhs.kind, ExprKind::Binary { op: BinOp::BitAnd, .. }));
        assert!(matches!(rhs.kind, ExprKind::Binary { op: BinOp::BitOr, .. }));
    }

    #[test]
    fn integer_literal_forms_and_range() {
        for (src, want) in [
            ("0xFF", 255),
            ("0xff", 255),
            ("0b1010", 10),
            ("0o17", 15),
            ("1_000_000", 1_000_000),
            ("0x7FFF_FFFF_FFFF_FFFF", i64::MAX),
            ("0", 0),
        ] {
            let e = return_expr(&format!("fn main() -> i32 {{ return {src}; }}"));
            assert_eq!(e.kind, ExprKind::Literal(Literal::Int(want)), "{src}");
        }
        let e = return_expr("fn main() -> f64 { return 1_000.000_5; }");
        assert_eq!(e.kind, ExprKind::Literal(Literal::Float(1000.0005)));
        for src in ["0x", "0b", "0o8", "0xFFFF_FFFF_FFFF_FFFF", "99999999999999999999"] {
            let (_, diags) = parse_src(&format!("fn main() -> i32 {{ return {src}; }}"));
            let out = render(&diags);
            assert!(out.contains("invalid integer literal"), "{src}: {out}");
        }
    }

    #[test]
    fn unicode_escapes_in_char_and_string() {
        let e = return_expr(r"fn main() -> i32 { return '\u{41}'; }");
        assert_eq!(e.kind, ExprKind::Literal(Literal::Char('A')));
        let e = return_expr(r#"fn main() -> i32 { return "a\u{1F600}b\u{0}"; }"#);
        assert_eq!(
            e.kind,
            ExprKind::Literal(Literal::String("a\u{1F600}b\0".into()))
        );
        // invalid escapes: E0005, but the literal still parses (U+FFFD)
        for src in [
            r#"fn main() -> i32 { return "\u{}"; }"#,
            r#"fn main() -> i32 { return "\u{110000}"; }"#,
            r#"fn main() -> i32 { return "\u{D800}"; }"#,
            r#"fn main() -> i32 { return "\u{1234567}"; }"#,
            r#"fn main() -> i32 { return "\u{zz}"; }"#,
            r#"fn main() -> i32 { return "\u{41"; }"#,
        ] {
            let (prog, diags) = parse_src(src);
            assert_eq!(
                diags.iter().filter(|d| d.code == Some("E0005")).count(),
                1,
                "{src}: {}",
                render(&diags)
            );
            assert_eq!(prog.items.len(), 1, "{src}");
        }
        // `\u` without a brace is an unknown escape, kept verbatim as before
        let e = return_expr(r#"fn main() -> i32 { return "\u41"; }"#);
        assert_eq!(e.kind, ExprKind::Literal(Literal::String("\\u41".into())));
    }

    #[test]
    fn parses_enum_match_if_let_and_tuples() {
        let src = "
            enum Shape { Circle(f64), Rect(i32, i32), Empty }
            fn main() -> i32 {
                let t = (1, (2, 3));
                let (a, b) = t.1;
                let s = Shape::Rect(t.0, t.1.1);
                match s {
                    Shape::Circle(r) => { }
                    Shape::Rect(w, _) => { }
                    -1 => { }
                    'c' => { }
                    _ => { }
                }
                if let Shape::Empty = s { } else { }
                let p: (i32, bool) = (1, true);
                return 0;
            }";
        let (prog, diags) = parse_src(src);
        assert!(!diags.has_errors(), "{}", render(&diags));
        assert!(matches!(prog.items[0], Item::Enum(ref e) if e.variants.len() == 3
            && e.variants[1].payload.len() == 2 && e.variants[2].payload.is_empty()));
        let Item::Fn(f) = &prog.items[1] else { panic!() };
        let stmts = &f.body.as_ref().unwrap().stmts;
        assert!(matches!(&stmts[0], Stmt::Let { init: Some(Expr { kind: ExprKind::Tuple { elements }, .. }), .. } if elements.len() == 2));
        assert!(matches!(&stmts[1], Stmt::LetTuple { names, .. } if names.len() == 2));
        // `t.1.1` lexes as the float `1.1` and is split into two field accesses
        let Stmt::Let { init: Some(Expr { kind: ExprKind::EnumLit { args, .. }, .. }), .. } = &stmts[2] else { panic!() };
        assert!(matches!(&args[1].kind, ExprKind::Field { base, field } if field.name == "1"
            && matches!(&base.kind, ExprKind::Field { field, .. } if field.name == "1")));
        let Stmt::Match { arms, .. } = &stmts[3] else { panic!() };
        assert_eq!(arms.len(), 5);
        assert!(matches!(&arms[1].pattern.kind, PatternKind::Variant { fields, .. }
            if matches!(fields[1].kind, PatternKind::Wildcard)));
        assert!(matches!(&arms[2].pattern.kind, PatternKind::Literal(Literal::Int(-1))));
        assert!(matches!(&arms[4].pattern.kind, PatternKind::Wildcard));
        // `if let` desugars to a two-arm match ending in `_`
        let Stmt::Match { arms, .. } = &stmts[4] else { panic!() };
        assert_eq!(arms.len(), 2);
        assert!(matches!(&arms[1].pattern.kind, PatternKind::Wildcard));
        assert!(matches!(&stmts[5], Stmt::Let { ty: Some(TypeExpr { kind: TypeExprKind::Tuple(ts), .. }), .. } if ts.len() == 2));
        // round trip through the pretty-printer and the AST dump
        let pretty = crate::pretty::pretty_program(&prog);
        assert!(pretty.contains("Shape::Rect(w, _) => {"), "{pretty}");
        assert!(pretty.contains("let (a, b) = t.1;"), "{pretty}");
        assert!(crate::ast::dump_program(&prog).contains("enum Shape {"));
        // float literals still lex as floats; `(a)` is a group, `(a,)` is not a tuple
        let e = return_expr("fn main() -> f64 { return 1.5; }");
        assert!(matches!(e.kind, ExprKind::Literal(Literal::Float(_))));
        let (_, diags) = parse_src("fn main() -> i32 { let t = (1,); return 0; }");
        assert!(diags.has_errors());
    }

    #[test]
    fn parses_for_and_struct() {
        let src = r#"
            struct Point { x: i32, y: i32 }
            fn main() -> i32 {
                let p = Point { x: 1, y: 2 };
                for i in 0..10 { p.x = p.x + i; }
                return p.x;
            }
        "#;
        let (prog, diags) = parse_src(src);
        assert!(!diags.has_errors());
        assert_eq!(prog.items.len(), 2);
    }
}
