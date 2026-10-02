//! Comment preservation for `aether fmt`.
//!
//! The parser drops comments, so the formatter re-lexes the main file with
//! [`tokenize_with_comments`] and the pretty-printer places every comment
//! by position while it walks the syntax tree in source order:
//!
//! * a comment before an item, statement, block tail, match arm, struct
//!   field or enum variant is printed on its own line(s) in front of it
//!   (leading comment);
//! * a comment on the same line after one of those (only `;` / `,` and
//!   other comments in between) stays at the end of that line (trailing);
//!   the same holds right after a block / match / struct / enum `{`;
//! * a comment between the last element of a block and its `}` stays inside
//!   the block; the comments after the last item stay at the end of the file;
//! * a comment inside a construct that has no line of its own (an
//!   expression, a pattern, a type, a parameter list, `} else {`) moves in
//!   front of the closest enclosing element listed above;
//! * one blank line is kept wherever the source had at least one blank line
//!   between two of those elements or comments.
//!
//! Every comment is printed exactly once with its text unchanged, except
//! for line endings: a line comment loses the `\r` of a CRLF line end and a
//! block comment's inner `\r\n` become `\n`.

use crate::ast::Program;
use crate::lexer::{tokenize_with_comments, Comment, CommentKind};
use crate::span::{FileId, Span};
use crate::token::TokenKind;

/// Comments of one source file plus the bookkeeping the printer needs.
pub struct Comments<'a> {
    src: &'a str,
    list: Vec<Comment>,
    done: Vec<bool>,
    /// Start offsets of the `{` tokens, ascending.
    lbraces: Vec<u32>,
    /// End offset of the last element or comment printed on its own line.
    pub(crate) last_end: u32,
    /// Right after an opening `{` (or at the start of the file): no blank
    /// line is printed before the next element.
    pub(crate) at_open: bool,
}

impl<'a> Comments<'a> {
    pub fn new(file: FileId, src: &'a str) -> Self {
        let (tokens, list, _) = tokenize_with_comments(file, src);
        let lbraces = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::LBrace)
            .map(|t| t.span.start.0)
            .collect();
        Comments {
            src,
            done: vec![false; list.len()],
            list,
            lbraces,
            last_end: 0,
            at_open: true,
        }
    }

    /// Number of comments not printed yet.
    pub fn remaining(&self) -> usize {
        self.done.iter().filter(|d| !**d).count()
    }

    /// Start of the first `{` token at or after `pos`.
    pub(crate) fn lbrace_after(&self, pos: u32) -> Option<u32> {
        let i = self.lbraces.partition_point(|&b| b < pos);
        self.lbraces.get(i).copied()
    }

    /// Whether the source byte at `pos` is `{` (a real block, not the
    /// one-statement block that holds an `else if`).
    pub(crate) fn braced(&self, span: Span) -> bool {
        self.src.as_bytes().get(span.start.0 as usize) == Some(&b'{')
    }

    pub(crate) fn start(&self, i: usize) -> u32 {
        self.list[i].span.start.0
    }

    pub(crate) fn end(&self, i: usize) -> u32 {
        self.list[i].span.end.0
    }

    /// The text as printed (see the module docs for line endings).
    pub(crate) fn text(&self, i: usize) -> String {
        let c = &self.list[i];
        match c.kind {
            CommentKind::Line => c.text.trim_end_matches('\r').to_string(),
            CommentKind::Block => c.text.replace("\r\n", "\n"),
        }
    }

    fn take(&mut self, pick: impl Fn(u32) -> bool) -> Vec<usize> {
        let mut out = Vec::new();
        for i in 0..self.list.len() {
            if !self.done[i] && pick(self.start(i)) {
                self.done[i] = true;
                out.push(i);
            }
        }
        out
    }

    /// Comments printed before an element spanning `span`: all earlier ones
    /// not printed yet, and the ones inside it that are not inside one of
    /// `regions` (the parts the element prints on lines of their own).
    pub(crate) fn take_leading(&mut self, span: Span, regions: &[(u32, u32)]) -> Vec<usize> {
        let (s, e) = (span.start.0, span.end.0);
        self.take(|p| p < s || (p < e && !regions.iter().any(|&(a, b)| a <= p && p < b)))
    }

    /// All comments before `pos` not printed yet.
    pub(crate) fn take_before(&mut self, pos: u32) -> Vec<usize> {
        self.take(|p| p < pos)
    }

    /// Comments after `pos` on the same line with only blanks, `;`, `,` and
    /// other such comments in between.
    pub(crate) fn take_trailing(&mut self, pos: u32) -> Vec<usize> {
        let b = self.src.as_bytes();
        let mut p = pos as usize;
        let mut out = Vec::new();
        while p < b.len() {
            match b[p] {
                b' ' | b'\t' | b'\r' | b';' | b',' => p += 1,
                b'/' => {
                    let Some(i) = (0..self.list.len()).find(|&i| self.start(i) as usize == p) else {
                        break;
                    };
                    if self.done[i] {
                        break;
                    }
                    self.done[i] = true;
                    out.push(i);
                    p = self.end(i) as usize;
                    if self.list[i].kind == CommentKind::Line || self.list[i].text.contains('\n') {
                        break;
                    }
                }
                _ => break,
            }
        }
        out
    }

    /// Whether the source has a blank line between offsets `a` and `b`.
    pub(crate) fn blank_between(&self, a: u32, b: u32) -> bool {
        if a >= b {
            return false;
        }
        let gap = &self.src[a as usize..b as usize];
        let lines: Vec<&str> = gap.split('\n').collect();
        lines.len() > 2
            && lines[1..lines.len() - 1]
                .iter()
                .any(|l| l.bytes().all(|c| matches!(c, b' ' | b'\t' | b'\r')))
    }
}

/// Formats `program` (parsed from `src`, file `file`) keeping the comments
/// of `src`. Items from other files must already be removed.
pub fn format_program(program: &Program, file: FileId, src: &str) -> String {
    crate::pretty::pretty_program_with(program, Some(Comments::new(file, src)))
}

/// Lexes, parses and formats one source text. `Err` holds the rendered
/// lexical/syntax errors.
pub fn format_source(src: &str) -> Result<String, String> {
    let (toks, diags) = crate::lexer::tokenize(FileId(0), src);
    let (prog, pdiags) = crate::parser::parse(toks);
    if diags.has_errors() || pdiags.has_errors() {
        let msgs: Vec<String> = diags.iter().chain(pdiags.iter()).map(|d| d.message.clone()).collect();
        return Err(msgs.join("\n"));
    }
    Ok(format_program(&prog, FileId(0), src))
}

/// The comments of `src` as `fmt` prints them (for tests).
pub fn comment_texts(src: &str) -> Vec<String> {
    let c = Comments::new(FileId(0), src);
    (0..c.list.len()).map(|i| c.text(i)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_comments_in_place() {
        let src = "// head\n\nfn main() -> i32 { // open\n    let x = 1; // why\n    /* end */\n}\n// eof";
        let out = format_source(src).unwrap();
        assert_eq!(
            out,
            "// head\n\nfn main() -> i32 { // open\n    let x = 1; // why\n    /* end */\n}\n// eof\n"
        );
        assert_eq!(format_source(&out).unwrap(), out);
    }
}
