//! Structured diagnostics with source snippets and suggestions.

use crate::span::{Session, Span};
use std::fmt;
use std::io::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warning,
    Note,
    Help,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Note => "note",
            Level::Help => "help",
        }
    }

    fn color(self) -> &'static str {
        match self {
            Level::Error => "\x1b[31;1m",
            Level::Warning => "\x1b[33;1m",
            Level::Note => "\x1b[36;1m",
            Level::Help => "\x1b[32;1m",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub level: Level,
    pub message: String,
    pub span: Span,
    pub notes: Vec<String>,
    pub help: Option<String>,
    pub code: Option<&'static str>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>, span: Span) -> Self {
        Diagnostic {
            level: Level::Error,
            message: message.into(),
            span,
            notes: Vec::new(),
            help: None,
            code: None,
        }
    }

    pub fn warning(message: impl Into<String>, span: Span) -> Self {
        Diagnostic {
            level: Level::Warning,
            message: message.into(),
            span,
            notes: Vec::new(),
            help: None,
            code: None,
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(code);
        self
    }
}

#[derive(Debug, Default)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Diagnostics { items: Vec::new() }
    }

    pub fn push(&mut self, d: Diagnostic) {
        self.items.push(d);
    }

    pub fn error(&mut self, message: impl Into<String>, span: Span) {
        self.items.push(Diagnostic::error(message, span));
    }

    pub fn extend(&mut self, other: Diagnostics) {
        self.items.extend(other.items);
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.level == Level::Error)
    }

    pub fn error_count(&self) -> usize {
        self.items.iter().filter(|d| d.level == Level::Error).count()
    }

    pub fn warning_count(&self) -> usize {
        self.items
            .iter()
            .filter(|d| d.level == Level::Warning)
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.items.iter()
    }

    pub fn emit(&self, session: &Session, color: bool) {
        let mut stderr = io::stderr();
        let _ = self.write_to(&mut stderr, session, color);
    }

    pub fn render(&self, session: &Session, color: bool) -> String {
        let mut buf = Vec::new();
        let _ = self.write_to(&mut buf, session, color);
        String::from_utf8_lossy(&buf).into_owned()
    }

    pub fn write_to<W: Write>(
        &self,
        w: &mut W,
        session: &Session,
        color: bool,
    ) -> io::Result<()> {
        let reset = if color { "\x1b[0m" } else { "" };
        let bold = if color { "\x1b[1m" } else { "" };
        let blue = if color { "\x1b[34;1m" } else { "" };
        for d in &self.items {
            let lvl_col = if color { d.level.color() } else { "" };
            let code = d
                .code
                .map(|c| format!("[{c}] "))
                .unwrap_or_default();
            writeln!(
                w,
                "{lvl_col}{}{reset}{bold}: {code}{}{reset}",
                d.level.label(),
                printable(&d.message)
            )?;

            if !d.span.is_dummy() {
                let file_name = printable(session.file_name(d.span.file));
                writeln!(
                    w,
                    "  {blue}-->{reset} {file_name}:{}:{}",
                    d.span.line, d.span.column
                )?;
                if let Some(file) = session.file(d.span.file) {
                    let line_no = d.span.line;
                    let src_line = file.line_contents(line_no);
                    let width = line_no.to_string().len().max(2);
                    writeln!(w, "  {blue}{:>width$} |{reset}", "")?;
                    // control characters in the source (a stray `\r`, an
                    // ESC inside a string literal) are shown as U+FFFD: printed
                    // raw they would move the cursor or recolour the terminal
                    let shown: String = src_line.chars().map(visible).collect();
                    writeln!(
                        w,
                        "  {blue}{line_no:>width$} |{reset} {shown}"
                    )?;
                    // columns count characters; tabs are copied into the
                    // padding so the carets line up under any tab width, and
                    // wide (CJK, emoji) characters take two cells
                    let col = d.span.column.max(1) as usize;
                    let line_chars = src_line.chars().count();
                    let pad: String = src_line
                        .chars()
                        .take(col - 1)
                        .map(|c| if c == '\t' { "\t".to_string() } else { " ".repeat(cell_width(c)) })
                        .chain(std::iter::repeat(" ".to_string()).take((col - 1).saturating_sub(line_chars)))
                        .collect();
                    let rest: usize = src_line.chars().skip(col - 1).map(cell_width).sum();
                    let span_cells: usize = file
                        .source
                        .get(d.span.start.0 as usize..d.span.end.0 as usize)
                        .unwrap_or("")
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .map(cell_width)
                        .sum();
                    let carets = "^".repeat(span_cells.max(1).min(rest.max(1)));
                    writeln!(
                        w,
                        "  {blue}{:>width$} |{reset} {pad}{lvl_col}{carets}{reset}",
                        ""
                    )?;
                }
            }

            for note in &d.notes {
                let ncol = if color { Level::Note.color() } else { "" };
                writeln!(w, "  {ncol}note{reset}: {}", printable(note))?;
            }
            if let Some(help) = &d.help {
                let hcol = if color { Level::Help.color() } else { "" };
                writeln!(w, "  {hcol}help{reset}: {}", printable(help))?;
            }
            writeln!(w)?;
        }
        Ok(())
    }
}

/// How a source character is shown in a diagnostic: control characters
/// other than tab become U+FFFD (one cell, like the character it replaces
/// in the column count).
fn visible(c: char) -> char {
    if c.is_control() && c != '\t' {
        '\u{FFFD}'
    } else {
        c
    }
}

/// A message, note, help or file name as printed: control characters
/// other than newline are escaped (`\u{1b}`), so text taken from the
/// source (an `unexpected character` that is an ESC or NUL, a `use` path)
/// cannot drive the terminal.
fn printable(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() && c != '\n' {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// Terminal cells a source character takes in a diagnostic: 0 for
/// combining marks and zero-width characters, 2 for East Asian wide and
/// emoji characters, else 1 (an approximation of `wcwidth` without tables).
fn cell_width(c: char) -> usize {
    let u = c as u32;
    match u {
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200B..=0x200F | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F => 0,
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.level.label(), self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Session;

    #[test]
    fn renders_error_with_caret() {
        let mut sess = Session::new();
        let id = sess.add_file("t.ae".into(), "let x = ;\n".into());
        let mut diags = Diagnostics::new();
        diags.push(
            Diagnostic::error("expected expression", Span::new(id, 8, 9, 1, 9))
                .with_help("add a value after `=`"),
        );
        let out = diags.render(&sess, false);
        assert!(out.contains("error: expected expression"));
        assert!(out.contains("t.ae:1:9"));
        assert!(out.contains("help: add a value after `=`"));
    }

    #[test]
    fn control_characters_are_not_printed_raw() {
        let mut sess = Session::new();
        let src = "let s = \"\u{1b}[31m\rX\"; bad\n";
        let id = sess.add_file("t.ae".into(), src.into());
        let start = src.find("bad").unwrap() as u32;
        let col = src[..start as usize].chars().count() as u32 + 1;
        let mut diags = Diagnostics::new();
        diags.push(Diagnostic::error("e", Span::new(id, start, start + 3, 1, col)));
        let out = diags.render(&sess, false);
        assert!(!out.contains('\u{1b}') && !out.contains('\r'), "{out:?}");
        let lines: Vec<&str> = out.lines().collect();
        let src_line = lines.iter().find(|l| l.contains("bad")).unwrap();
        let caret_line = lines.iter().find(|l| l.contains('^')).unwrap();
        // the carets sit under `bad`: same char offset in both lines
        assert_eq!(
            src_line.chars().position(|c| c == 'b'),
            caret_line.chars().position(|c| c == '^'),
            "{out}"
        );
    }

    #[test]
    fn messages_escape_control_characters() {
        let mut sess = Session::new();
        let src = "x\u{1b}y\n";
        let id = sess.add_file("t\u{7}.ae".into(), src.into());
        let mut diags = Diagnostics::new();
        diags.push(
            Diagnostic::error("unexpected character `\u{1b}`", Span::new(id, 1, 2, 1, 2))
                .with_note("n\u{0}")
                .with_help("h\u{9b}"),
        );
        let out = diags.render(&sess, false);
        assert!(!out.chars().any(|c| c.is_control() && c != '\n'), "{out:?}");
        assert!(out.contains("unexpected character `\\u{1b}`"), "{out}");
        assert!(out.contains("t\\u{7}.ae:1:2"), "{out}");
    }

    #[test]
    fn carets_account_for_wide_characters() {
        let mut sess = Session::new();
        let src = "let s = \"日本\"; bad\n";
        let id = sess.add_file("t.ae".into(), src.into());
        let start = src.find("bad").unwrap() as u32;
        let col = src[..start as usize].chars().count() as u32 + 1;
        let mut diags = Diagnostics::new();
        diags.push(Diagnostic::error("e", Span::new(id, start, start + 3, 1, col)));
        let out = diags.render(&sess, false);
        let caret_line = out.lines().find(|l| l.contains('^')).unwrap();
        // the two wide chars take 4 cells, so the carets start 2 cells
        // further right than the char count before `bad`
        let caret_cells = caret_line.chars().take_while(|c| *c != '^').count();
        let plain = out.lines().find(|l| l.contains("bad")).unwrap();
        let chars_before = plain.chars().take_while(|c| *c != 'b').count();
        assert_eq!(caret_cells, chars_before + 2, "{out}");
    }
}
