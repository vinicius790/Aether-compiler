//! Recursive-descent parser with a Pratt parser for expressions.
//!
//! Statement and item grammar is LL(1)-friendly; operator precedence and
//! associativity live in the Pratt table so they cannot drift from the spec.

use crate::ast::*;
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::span::Span;
use crate::token::{Token, TokenKind};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    diags: Diagnostics,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            pos: 0,
            diags: Diagnostics::new(),
        }
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
                            TokenKind::Fn | TokenKind::Struct | TokenKind::Extern | TokenKind::Eof
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
        match self.peek_kind() {
            TokenKind::Fn => self.parse_fn().map(Item::Fn),
            TokenKind::Struct => self.parse_struct().map(Item::Struct),
            TokenKind::Extern => self.parse_extern().map(Item::Extern),
            TokenKind::Eof => None,
            _ => {
                let tok = self.peek().clone();
                self.error_at(
                    format!("expected item, found `{}`", tok.lexeme),
                    tok.span,
                    Some("items start with `fn`, `struct` or `extern`"),
                );
                None
            }
        }
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
            name,
            fields,
            span: start.merge(end),
        })
    }

    fn parse_block(&mut self) -> Option<Block> {
        let start = self.expect(TokenKind::LBrace)?.span;
        let mut stmts = Vec::new();
        let mut tail = None;
        while !self.check(TokenKind::RBrace) && !self.is_eof() {
            if self.is_stmt_start() {
                if let Some(stmt) = self.parse_stmt() {
                    stmts.push(stmt);
                } else {
                    self.synchronize_stmt();
                }
            } else {
                // expression statement, assignment, or block tail
                match self.parse_expr() {
                    Some(expr) => {
                        if self.eat(TokenKind::Eq) {
                            let value = match self.parse_expr() {
                                Some(v) => v,
                                None => {
                                    self.synchronize_stmt();
                                    continue;
                                }
                            };
                            self.expect(TokenKind::Semicolon);
                            let span = expr.span.merge(self.prev_span());
                            stmts.push(Stmt::Assign {
                                target: expr,
                                value,
                                span,
                            });
                        } else if self.check(TokenKind::RBrace) {
                            tail = Some(Box::new(expr));
                            break;
                        } else if self.eat(TokenKind::Semicolon) {
                            let span = expr.span;
                            stmts.push(Stmt::Expr { expr, span });
                        } else {
                            tail = Some(Box::new(expr));
                            break;
                        }
                    }
                    None => {
                        self.synchronize_stmt();
                    }
                }
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
                | TokenKind::LBrace
        )
    }

    fn parse_stmt(&mut self) -> Option<Stmt> {
        match self.peek_kind() {
            TokenKind::Let => self.parse_let(),
            TokenKind::If => self.parse_if(),
            TokenKind::While => self.parse_while(),
            TokenKind::For => self.parse_for(),
            TokenKind::Return => {
                let start = self.bump().span;
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
            TokenKind::Break => {
                let start = self.bump().span;
                self.expect(TokenKind::Semicolon)?;
                Some(Stmt::Break {
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::Continue => {
                let start = self.bump().span;
                self.expect(TokenKind::Semicolon)?;
                Some(Stmt::Continue {
                    span: start.merge(self.prev_span()),
                })
            }
            TokenKind::LBrace => {
                let block = self.parse_block()?;
                let span = block.span;
                Some(Stmt::Block { block, span })
            }
            _ => {
                let expr = self.parse_expr()?;
                if self.eat(TokenKind::Eq) {
                    let value = self.parse_expr()?;
                    self.expect(TokenKind::Semicolon)?;
                    let span = expr.span.merge(self.prev_span());
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
        }
    }

    fn parse_let(&mut self) -> Option<Stmt> {
        let start = self.expect(TokenKind::Let)?.span;
        let mutable = self.eat(TokenKind::Mut);
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

    fn parse_if(&mut self) -> Option<Stmt> {
        let start = self.expect(TokenKind::If)?.span;
        let cond = self.parse_expr()?;
        let then_block = self.parse_block()?;
        let else_block = if self.eat(TokenKind::Else) {
            if self.check(TokenKind::If) {
                // else if → wrap in a block containing a single if
                let inner = self.parse_if()?;
                let span = inner.span();
                Some(Block {
                    stmts: vec![inner],
                    tail: None,
                    span,
                })
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };
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
        let mut lhs = self.parse_prefix()?;
        loop {
            let kind = self.peek_kind();
            if kind == TokenKind::As {
                if min_prec > 9 {
                    break;
                }
                self.bump();
                let ty = self.parse_type()?;
                let span = lhs.span.merge(ty.span);
                lhs = Expr {
                    kind: ExprKind::Cast {
                        expr: Box::new(lhs),
                        ty,
                    },
                    span,
                };
                continue;
            }
            if let Some((prec, right_assoc, op)) = infix_info(kind) {
                if prec < min_prec {
                    break;
                }
                self.bump();
                let next_min = if right_assoc { prec } else { prec + 1 };
                let rhs = self.parse_prec(next_min)?;
                let span = lhs.span.merge(rhs.span);
                lhs = Expr {
                    kind: ExprKind::Binary {
                        op,
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    },
                    span,
                };
                continue;
            }
            break;
        }
        Some(lhs)
    }

    fn parse_prefix(&mut self) -> Option<Expr> {
        match self.peek_kind() {
            TokenKind::Int => {
                let tok = self.bump();
                let value = parse_int(&tok.lexeme);
                if value.is_none() {
                    self.error_at("invalid integer literal", tok.span, None);
                }
                Some(Expr {
                    kind: ExprKind::Literal(Literal::Int(value.unwrap_or(0))),
                    span: tok.span,
                })
            }
            TokenKind::Float => {
                let tok = self.bump();
                let value = tok.lexeme.parse::<f64>().unwrap_or(0.0);
                Some(Expr {
                    kind: ExprKind::Literal(Literal::Float(value)),
                    span: tok.span,
                })
            }
            TokenKind::True => {
                let tok = self.bump();
                Some(Expr {
                    kind: ExprKind::Literal(Literal::Bool(true)),
                    span: tok.span,
                })
            }
            TokenKind::False => {
                let tok = self.bump();
                Some(Expr {
                    kind: ExprKind::Literal(Literal::Bool(false)),
                    span: tok.span,
                })
            }
            TokenKind::String => {
                let tok = self.bump();
                let value = unescape_string(&tok.lexeme);
                Some(Expr {
                    kind: ExprKind::Literal(Literal::String(value)),
                    span: tok.span,
                })
            }
            TokenKind::Char => {
                let tok = self.bump();
                let value = unescape_char(&tok.lexeme);
                Some(Expr {
                    kind: ExprKind::Literal(Literal::Char(value)),
                    span: tok.span,
                })
            }
            TokenKind::Ident => {
                let id = self.parse_ident()?;
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
                Some(self.parse_postfix(expr)?)
            }
            TokenKind::LParen => {
                let start = self.bump().span;
                if self.check(TokenKind::RParen) {
                    let end = self.bump().span;
                    return Some(Expr {
                        kind: ExprKind::Literal(Literal::Unit),
                        span: start.merge(end),
                    });
                }
                let inner = self.parse_expr()?;
                let end = self.expect(TokenKind::RParen)?.span;
                let expr = Expr {
                    kind: ExprKind::Group(Box::new(inner)),
                    span: start.merge(end),
                };
                Some(self.parse_postfix(expr)?)
            }
            TokenKind::LBracket => {
                let start = self.bump().span;
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
                Some(self.parse_postfix(expr)?)
            }
            TokenKind::Minus => {
                let tok = self.bump();
                let expr = self.parse_prec(12)?;
                let span = tok.span.merge(expr.span);
                Some(Expr {
                    kind: ExprKind::Unary {
                        op: UnOp::Neg,
                        expr: Box::new(expr),
                    },
                    span,
                })
            }
            TokenKind::Bang => {
                let tok = self.bump();
                let expr = self.parse_prec(12)?;
                let span = tok.span.merge(expr.span);
                Some(Expr {
                    kind: ExprKind::Unary {
                        op: UnOp::Not,
                        expr: Box::new(expr),
                    },
                    span,
                })
            }
            TokenKind::TyI32
            | TokenKind::TyI64
            | TokenKind::TyF64
            | TokenKind::TyBool
            | TokenKind::TyString
            | TokenKind::TyUnit => {
                // type names used as identifiers should not appear in expr
                let tok = self.bump();
                self.error_at(
                    format!("type name `{}` is not a valid expression", tok.lexeme),
                    tok.span,
                    Some("use a variable or literal here"),
                );
                None
            }
            _ => {
                let tok = self.peek().clone();
                self.error_at(
                    format!("expected expression, found `{}`", tok.lexeme),
                    tok.span,
                    None,
                );
                None
            }
        }
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
                    let field = self.parse_ident()?;
                    let span = expr.span.merge(field.span);
                    expr = Expr {
                        kind: ExprKind::Field {
                            base: Box::new(expr),
                            field,
                        },
                        span,
                    };
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
                self.expect(TokenKind::RParen)?;
                Some(TypeExpr::unit(start.merge(self.prev_span())))
            }
            _ => {
                let tok = self.peek().clone();
                self.error_at(
                    format!("expected type, found `{}`", tok.lexeme),
                    tok.span,
                    Some("valid types: i32, i64, f64, bool, string, unit, [T; N], or a struct name"),
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
                    | TokenKind::Fn
                    | TokenKind::Struct
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
}

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
        TokenKind::Plus => Some((5, false, BinOp::Add)),
        TokenKind::Minus => Some((5, false, BinOp::Sub)),
        TokenKind::Star => Some((6, false, BinOp::Mul)),
        TokenKind::Slash => Some((6, false, BinOp::Div)),
        TokenKind::Percent => Some((6, false, BinOp::Rem)),
        _ => None,
    }
}

fn parse_int(lexeme: &str) -> Option<i64> {
    lexeme.parse::<i64>().ok()
}

fn unescape_string(lexeme: &str) -> String {
    let inner = if lexeme.len() >= 2 && lexeme.starts_with('"') && lexeme.ends_with('"') {
        &lexeme[1..lexeme.len() - 1]
    } else if lexeme.starts_with('"') {
        &lexeme[1..]
    } else {
        lexeme
    };
    unescape(inner)
}

fn unescape_char(lexeme: &str) -> char {
    let inner = if lexeme.len() >= 2 && lexeme.starts_with('\'') && lexeme.ends_with('\'') {
        &lexeme[1..lexeme.len() - 1]
    } else {
        lexeme
    };
    unescape(inner).chars().next().unwrap_or('\0')
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
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
    out
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
