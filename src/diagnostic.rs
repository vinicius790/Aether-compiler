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
                d.message
            )?;

            if !d.span.is_dummy() {
                let file_name = session.file_name(d.span.file);
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
                    writeln!(
                        w,
                        "  {blue}{line_no:>width$} |{reset} {src_line}"
                    )?;
                    let col = d.span.column.max(1) as usize;
                    let caret_len = (d.span.len() as usize).max(1).min(src_line.len().saturating_sub(col.saturating_sub(1)).max(1));
                    let pad = " ".repeat(col.saturating_sub(1));
                    let carets = "^".repeat(caret_len);
                    writeln!(
                        w,
                        "  {blue}{:>width$} |{reset} {pad}{lvl_col}{carets}{reset}",
                        ""
                    )?;
                }
            }

            for note in &d.notes {
                let ncol = if color { Level::Note.color() } else { "" };
                writeln!(w, "  {ncol}note{reset}: {note}")?;
            }
            if let Some(help) = &d.help {
                let hcol = if color { Level::Help.color() } else { "" };
                writeln!(w, "  {hcol}help{reset}: {help}")?;
            }
            writeln!(w)?;
        }
        Ok(())
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
}
