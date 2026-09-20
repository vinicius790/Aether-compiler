//! Hand-written lexer. Produces a complete token stream including EOF.
//!
//! Invalid characters become `TokenKind::Invalid` so the parser can recover
//! and the diagnostic system can point at the exact byte.

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::span::{FileId, Span};
use crate::token::{Token, TokenKind};

pub struct Lexer<'src> {
    file: FileId,
    src: &'src str,
    bytes: &'src [u8],
    pos: usize,
    line: u32,
    column: u32,
    diags: Diagnostics,
}

impl<'src> Lexer<'src> {
    pub fn new(file: FileId, src: &'src str) -> Self {
        Lexer {
            file,
            src,
            bytes: src.as_bytes(),
            pos: 0,
            line: 1,
            column: 1,
            diags: Diagnostics::new(),
        }
    }

    pub fn tokenize(mut self) -> (Vec<Token>, Diagnostics) {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token();
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        (tokens, self.diags)
    }

    fn next_token(&mut self) -> Token {
        self.skip_trivia();
        let start = self.pos;
        let line = self.line;
        let column = self.column;

        if self.is_eof() {
            return self.make(TokenKind::Eof, start, line, column);
        }

        let ch = self.peek_char();
        match ch {
            'a'..='z' | 'A'..='Z' | '_' => self.ident_or_keyword(start, line, column),
            '0'..='9' => self.number(start, line, column),
            '"' => self.string(start, line, column),
            '\'' => self.char_lit(start, line, column),
            '(' => self.single(TokenKind::LParen, start, line, column),
            ')' => self.single(TokenKind::RParen, start, line, column),
            '{' => self.single(TokenKind::LBrace, start, line, column),
            '}' => self.single(TokenKind::RBrace, start, line, column),
            '[' => self.single(TokenKind::LBracket, start, line, column),
            ']' => self.single(TokenKind::RBracket, start, line, column),
            ',' => self.single(TokenKind::Comma, start, line, column),
            ';' => self.single(TokenKind::Semicolon, start, line, column),
            ':' => self.single(TokenKind::Colon, start, line, column),
            '+' => self.single(TokenKind::Plus, start, line, column),
            '*' => self.single(TokenKind::Star, start, line, column),
            '%' => self.single(TokenKind::Percent, start, line, column),
            '-' => {
                self.bump();
                if self.peek_char() == '>' {
                    self.bump();
                    self.make(TokenKind::Arrow, start, line, column)
                } else {
                    self.make(TokenKind::Minus, start, line, column)
                }
            }
            '=' => {
                self.bump();
                if self.peek_char() == '=' {
                    self.bump();
                    self.make(TokenKind::EqEq, start, line, column)
                } else {
                    self.make(TokenKind::Eq, start, line, column)
                }
            }
            '!' => {
                self.bump();
                if self.peek_char() == '=' {
                    self.bump();
                    self.make(TokenKind::BangEq, start, line, column)
                } else {
                    self.make(TokenKind::Bang, start, line, column)
                }
            }
            '<' => {
                self.bump();
                if self.peek_char() == '=' {
                    self.bump();
                    self.make(TokenKind::LtEq, start, line, column)
                } else {
                    self.make(TokenKind::Lt, start, line, column)
                }
            }
            '>' => {
                self.bump();
                if self.peek_char() == '=' {
                    self.bump();
                    self.make(TokenKind::GtEq, start, line, column)
                } else {
                    self.make(TokenKind::Gt, start, line, column)
                }
            }
            '&' => {
                self.bump();
                if self.peek_char() == '&' {
                    self.bump();
                    self.make(TokenKind::AmpAmp, start, line, column)
                } else {
                    self.make(TokenKind::Amp, start, line, column)
                }
            }
            '|' => {
                self.bump();
                if self.peek_char() == '|' {
                    self.bump();
                    self.make(TokenKind::PipePipe, start, line, column)
                } else {
                    self.make(TokenKind::Pipe, start, line, column)
                }
            }
            '.' => {
                self.bump();
                if self.peek_char() == '.' {
                    self.bump();
                    self.make(TokenKind::DotDot, start, line, column)
                } else {
                    self.make(TokenKind::Dot, start, line, column)
                }
            }
            '/' => {
                // trivia already consumed comments; a remaining slash is division
                self.single(TokenKind::Slash, start, line, column)
            }
            _ => {
                let bad = ch;
                self.bump();
                let span = self.span_from(start, line, column);
                self.diags.push(
                    Diagnostic::error(format!("unexpected character `{bad}`"), span)
                        .with_code("E0001")
                        .with_help("remove this character or include it inside a string"),
                );
                self.make(TokenKind::Invalid, start, line, column)
            }
        }
    }

    fn ident_or_keyword(&mut self, start: usize, line: u32, column: u32) -> Token {
        while matches!(self.peek_char(), 'a'..='z' | 'A'..='Z' | '0'..='9' | '_') {
            self.bump();
        }
        let lexeme = &self.src[start..self.pos];
        let kind = TokenKind::keyword(lexeme).unwrap_or(TokenKind::Ident);
        self.make(kind, start, line, column)
    }

    fn number(&mut self, start: usize, line: u32, column: u32) -> Token {
        while self.peek_char().is_ascii_digit() {
            self.bump();
        }
        let mut is_float = false;
        if self.peek_char() == '.' && self.peek_char_at(1).is_ascii_digit() {
            is_float = true;
            self.bump();
            while self.peek_char().is_ascii_digit() {
                self.bump();
            }
        }
        if matches!(self.peek_char(), 'e' | 'E') {
            let next = self.peek_char_at(1);
            if next.is_ascii_digit() || ((next == '+' || next == '-') && self.peek_char_at(2).is_ascii_digit())
            {
                is_float = true;
                self.bump();
                if matches!(self.peek_char(), '+' | '-') {
                    self.bump();
                }
                while self.peek_char().is_ascii_digit() {
                    self.bump();
                }
            }
        }
        let kind = if is_float {
            TokenKind::Float
        } else {
            TokenKind::Int
        };
        self.make(kind, start, line, column)
    }

    fn string(&mut self, start: usize, line: u32, column: u32) -> Token {
        self.bump(); // opening "
        let mut closed = false;
        while !self.is_eof() {
            let ch = self.peek_char();
            if ch == '"' {
                self.bump();
                closed = true;
                break;
            }
            if ch == '\\' {
                self.bump();
                if !self.is_eof() {
                    self.bump();
                }
                continue;
            }
            if ch == '\n' {
                break;
            }
            self.bump();
        }
        if !closed {
            let span = self.span_from(start, line, column);
            self.diags.push(
                Diagnostic::error("unterminated string literal", span)
                    .with_code("E0002")
                    .with_help("add a closing `\"` before the end of the line"),
            );
        }
        self.make(TokenKind::String, start, line, column)
    }

    fn char_lit(&mut self, start: usize, line: u32, column: u32) -> Token {
        self.bump();
        if self.peek_char() == '\\' {
            self.bump();
            if !self.is_eof() {
                self.bump();
            }
        } else if !self.is_eof() && self.peek_char() != '\'' {
            self.bump();
        }
        if self.peek_char() == '\'' {
            self.bump();
        } else {
            let span = self.span_from(start, line, column);
            self.diags.push(
                Diagnostic::error("unterminated character literal", span)
                    .with_code("E0003")
                    .with_help("character literals look like `'a'` or `'\\n'`"),
            );
        }
        self.make(TokenKind::Char, start, line, column)
    }

    fn single(&mut self, kind: TokenKind, start: usize, line: u32, column: u32) -> Token {
        self.bump();
        self.make(kind, start, line, column)
    }

    fn skip_trivia(&mut self) {
        loop {
            if self.is_eof() {
                return;
            }
            match self.peek_char() {
                ' ' | '\t' | '\r' => {
                    self.bump();
                }
                '\n' => {
                    self.bump();
                }
                '/' if self.peek_char_at(1) == '/' => {
                    while !self.is_eof() && self.peek_char() != '\n' {
                        self.bump();
                    }
                }
                '/' if self.peek_char_at(1) == '*' => {
                    let start = self.pos;
                    let line = self.line;
                    let column = self.column;
                    self.bump();
                    self.bump();
                    let mut closed = false;
                    while !self.is_eof() {
                        if self.peek_char() == '*' && self.peek_char_at(1) == '/' {
                            self.bump();
                            self.bump();
                            closed = true;
                            break;
                        }
                        self.bump();
                    }
                    if !closed {
                        self.diags.push(
                            Diagnostic::error(
                                "unterminated block comment",
                                self.span_from(start, line, column),
                            )
                            .with_code("E0004")
                            .with_help("close the comment with `*/`"),
                        );
                    }
                }
                _ => return,
            }
        }
    }

    fn make(&self, kind: TokenKind, start: usize, line: u32, column: u32) -> Token {
        Token::new(
            kind,
            self.span_from(start, line, column),
            self.src[start..self.pos].to_string(),
        )
    }

    fn span_from(&self, start: usize, line: u32, column: u32) -> Span {
        Span::new(self.file, start as u32, self.pos as u32, line, column)
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.bytes.len()
    }

    fn peek_char(&self) -> char {
        if self.is_eof() {
            '\0'
        } else {
            self.src[self.pos..].chars().next().unwrap_or('\0')
        }
    }

    fn peek_char_at(&self, n: usize) -> char {
        self.src[self.pos..].chars().nth(n).unwrap_or('\0')
    }

    fn bump(&mut self) {
        if self.is_eof() {
            return;
        }
        let ch = self.peek_char();
        self.pos += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
    }
}

pub fn tokenize(file: FileId, src: &str) -> (Vec<Token>, Diagnostics) {
    Lexer::new(file, src).tokenize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::FileId;

    fn kinds(src: &str) -> Vec<TokenKind> {
        let (toks, _) = tokenize(FileId(0), src);
        toks.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_hello_program() {
        let src = r#"fn main() -> i32 { return 0; }"#;
        let k = kinds(src);
        assert_eq!(
            k,
            vec![
                TokenKind::Fn,
                TokenKind::Ident,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::Arrow,
                TokenKind::TyI32,
                TokenKind::LBrace,
                TokenKind::Return,
                TokenKind::Int,
                TokenKind::Semicolon,
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_operators_and_comments() {
        let src = "a += 1; // ignore\n/* block */ 1.5 != 2 && true";
        // `+=` is `+` then `=`
        let k = kinds(src);
        assert!(k.contains(&TokenKind::Plus));
        assert!(k.contains(&TokenKind::Eq));
        assert!(k.contains(&TokenKind::Float));
        assert!(k.contains(&TokenKind::BangEq));
        assert!(k.contains(&TokenKind::AmpAmp));
        assert!(k.contains(&TokenKind::True));
        assert!(!k.contains(&TokenKind::Invalid));
    }

    #[test]
    fn unterminated_string_is_invalid_but_emits_token() {
        let (toks, diags) = tokenize(FileId(0), "\"abc");
        assert!(diags.has_errors());
        assert!(toks.iter().any(|t| t.kind == TokenKind::String));
    }

    #[test]
    fn unexpected_character_reported() {
        let (_, diags) = tokenize(FileId(0), "@");
        assert!(diags.has_errors());
    }

    #[test]
    fn tracks_line_and_column() {
        let (toks, _) = tokenize(FileId(0), "a\n  b");
        let b = toks.iter().find(|t| t.lexeme == "b").unwrap();
        assert_eq!(b.span.line, 2);
        assert_eq!(b.span.column, 3);
    }
}
