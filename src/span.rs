//! Source locations used by every compiler stage.
//!
//! A [`Span`] is a half-open byte range into a single source file, plus
//! 1-based line/column information computed at lex time so diagnostics
//! never have to rescan the file.

use std::fmt;

/// Identifies a source file in the compiler session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

impl FileId {
    pub const DUMMY: FileId = FileId(u32::MAX);
}

/// Byte offset into a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BytePos(pub u32);

/// A contiguous region of source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: FileId,
    pub start: BytePos,
    pub end: BytePos,
    pub line: u32,
    pub column: u32,
}

impl Span {
    pub const DUMMY: Span = Span {
        file: FileId::DUMMY,
        start: BytePos(0),
        end: BytePos(0),
        line: 0,
        column: 0,
    };

    pub fn new(file: FileId, start: u32, end: u32, line: u32, column: u32) -> Self {
        Span {
            file,
            start: BytePos(start),
            end: BytePos(end),
            line,
            column,
        }
    }

    pub fn merge(self, other: Span) -> Span {
        if self.file != other.file || self.file == FileId::DUMMY {
            return self;
        }
        Span {
            file: self.file,
            start: BytePos(self.start.0.min(other.start.0)),
            end: BytePos(self.end.0.max(other.end.0)),
            line: self.line.min(other.line),
            column: if self.start.0 <= other.start.0 {
                self.column
            } else {
                other.column
            },
        }
    }

    pub fn is_dummy(self) -> bool {
        self.file == FileId::DUMMY
    }

    pub fn len(self) -> u32 {
        self.end.0.saturating_sub(self.start.0)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_dummy() {
            write!(f, "<unknown>")
        } else {
            write!(f, "{}:{}", self.line, self.column)
        }
    }
}

/// A value annotated with a source span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Span) -> Self {
        Spanned { value, span }
    }
}

/// Source file stored by the session.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub id: FileId,
    pub name: String,
    pub source: String,
    /// Byte offset of the first character of each line (0-based).
    pub line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn new(id: FileId, name: String, source: String) -> Self {
        let mut line_starts = vec![0];
        for (i, ch) in source.char_indices() {
            if ch == '\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        SourceFile {
            id,
            name,
            source,
            line_starts,
        }
    }

    pub fn line_contents(&self, line: u32) -> &str {
        if line == 0 {
            return "";
        }
        let idx = (line as usize).saturating_sub(1);
        let start = *self.line_starts.get(idx).unwrap_or(&0) as usize;
        let end = self
            .line_starts
            .get(idx + 1)
            .map(|p| *p as usize)
            .unwrap_or(self.source.len());
        let slice = &self.source[start.min(self.source.len())..end.min(self.source.len())];
        slice.trim_end_matches(['\n', '\r'])
    }

    pub fn snippet(&self, span: Span) -> &str {
        let start = span.start.0 as usize;
        let end = span.end.0 as usize;
        let len = self.source.len();
        if start >= len {
            return "";
        }
        &self.source[start..end.min(len)]
    }
}

/// Compilation session: source files and interned identifiers.
#[derive(Debug, Default)]
pub struct Session {
    files: Vec<SourceFile>,
}

impl Session {
    pub fn new() -> Self {
        Session { files: Vec::new() }
    }

    pub fn add_file(&mut self, name: String, source: String) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.files.push(SourceFile::new(id, name, source));
        id
    }

    pub fn file(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.0 as usize)
    }

    pub fn files(&self) -> &[SourceFile] {
        &self.files
    }

    pub fn file_name(&self, id: FileId) -> &str {
        self.file(id).map(|f| f.name.as_str()).unwrap_or("<unknown>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_track_newlines() {
        let f = SourceFile::new(FileId(0), "t.ae".into(), "a\nbc\n".into());
        assert_eq!(f.line_starts, vec![0, 2, 5]);
        assert_eq!(f.line_contents(1), "a");
        assert_eq!(f.line_contents(2), "bc");
    }

    #[test]
    fn span_merge_takes_extents() {
        let a = Span::new(FileId(0), 0, 3, 1, 1);
        let b = Span::new(FileId(0), 5, 8, 1, 6);
        let m = a.merge(b);
        assert_eq!(m.start.0, 0);
        assert_eq!(m.end.0, 8);
    }
}
