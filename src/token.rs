//! Token kinds produced by the lexer.

use crate::span::Span;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    // identifiers & literals
    Ident,
    Int,
    Float,
    String,
    Char,

    // keywords
    Fn,
    Struct,
    Let,
    Mut,
    If,
    Else,
    While,
    For,
    In,
    Return,
    Break,
    Continue,
    True,
    False,
    As,
    Extern,
    Pub,

    // types (reserved)
    TyI32,
    TyI64,
    TyF64,
    TyBool,
    TyString,
    TyUnit,

    // operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    EqEq,
    Bang,
    BangEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    AmpAmp,
    PipePipe,
    Amp,
    Pipe,

    // delimiters
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon,
    Colon,
    Arrow,
    Dot,
    DotDot,
    Assign,

    Eof,
    Invalid,
}

impl TokenKind {
    pub fn keyword(word: &str) -> Option<TokenKind> {
        Some(match word {
            "fn" => TokenKind::Fn,
            "struct" => TokenKind::Struct,
            "let" => TokenKind::Let,
            "mut" => TokenKind::Mut,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "return" => TokenKind::Return,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "as" => TokenKind::As,
            "extern" => TokenKind::Extern,
            "pub" => TokenKind::Pub,
            "i32" => TokenKind::TyI32,
            "i64" => TokenKind::TyI64,
            "f64" => TokenKind::TyF64,
            "bool" => TokenKind::TyBool,
            "string" => TokenKind::TyString,
            "unit" => TokenKind::TyUnit,
            _ => return None,
        })
    }

    pub fn is_type_keyword(self) -> bool {
        matches!(
            self,
            TokenKind::TyI32
                | TokenKind::TyI64
                | TokenKind::TyF64
                | TokenKind::TyBool
                | TokenKind::TyString
                | TokenKind::TyUnit
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Ident => "identifier",
            TokenKind::Int => "integer literal",
            TokenKind::Float => "float literal",
            TokenKind::String => "string literal",
            TokenKind::Char => "character literal",
            TokenKind::Fn => "fn",
            TokenKind::Struct => "struct",
            TokenKind::Let => "let",
            TokenKind::Mut => "mut",
            TokenKind::If => "if",
            TokenKind::Else => "else",
            TokenKind::While => "while",
            TokenKind::For => "for",
            TokenKind::In => "in",
            TokenKind::Return => "return",
            TokenKind::Break => "break",
            TokenKind::Continue => "continue",
            TokenKind::True => "true",
            TokenKind::False => "false",
            TokenKind::As => "as",
            TokenKind::Extern => "extern",
            TokenKind::Pub => "pub",
            TokenKind::TyI32 => "i32",
            TokenKind::TyI64 => "i64",
            TokenKind::TyF64 => "f64",
            TokenKind::TyBool => "bool",
            TokenKind::TyString => "string",
            TokenKind::TyUnit => "unit",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Star => "*",
            TokenKind::Slash => "/",
            TokenKind::Percent => "%",
            TokenKind::Eq => "=",
            TokenKind::EqEq => "==",
            TokenKind::Bang => "!",
            TokenKind::BangEq => "!=",
            TokenKind::Lt => "<",
            TokenKind::LtEq => "<=",
            TokenKind::Gt => ">",
            TokenKind::GtEq => ">=",
            TokenKind::AmpAmp => "&&",
            TokenKind::PipePipe => "||",
            TokenKind::Amp => "&",
            TokenKind::Pipe => "|",
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::Comma => ",",
            TokenKind::Semicolon => ";",
            TokenKind::Colon => ":",
            TokenKind::Arrow => "->",
            TokenKind::Dot => ".",
            TokenKind::DotDot => "..",
            TokenKind::Assign => "=",
            TokenKind::Eof => "end of file",
            TokenKind::Invalid => "invalid token",
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub lexeme: String,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span, lexeme: impl Into<String>) -> Self {
        Token {
            kind,
            span,
            lexeme: lexeme.into(),
        }
    }

    pub fn is(&self, kind: TokenKind) -> bool {
        self.kind == kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_roundtrip() {
        assert_eq!(TokenKind::keyword("fn"), Some(TokenKind::Fn));
        assert_eq!(TokenKind::keyword("i32"), Some(TokenKind::TyI32));
        assert_eq!(TokenKind::keyword("foo"), None);
    }
}
