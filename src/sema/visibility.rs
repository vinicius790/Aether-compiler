//! `pub` visibility across files (E0281).
//!
//! The namespace is flat: every file of a program (the main file, its `use`
//! imports and `--include`s) contributes to one item table. An item defined
//! in a file other than the one that names it is accessible only when it is
//! declared `pub`; items of the same file are always accessible. Enums and
//! built-ins are not subject to the rule.

use crate::ast::{Item, Program};
use crate::diagnostic::Diagnostic;
use crate::span::{FileId, Span};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum VisKind {
    Fn,
    Struct,
}

struct Entry {
    is_pub: bool,
    def: Span,
}

pub(super) struct Visibility {
    fns: HashMap<String, Entry>,
    structs: HashMap<String, Entry>,
    /// Display names of the session's files, indexed by `FileId`.
    files: Vec<String>,
    reported: HashSet<(FileId, u32)>,
}

impl Visibility {
    pub(super) fn new(program: &Program) -> Self {
        let mut fns = HashMap::new();
        let mut structs = HashMap::new();
        for item in &program.items {
            match item {
                Item::Fn(f) => {
                    fns.entry(f.name.name.clone()).or_insert(Entry {
                        is_pub: f.is_pub,
                        def: f.name.span,
                    });
                }
                Item::Extern(e) => {
                    fns.entry(e.name.name.clone()).or_insert(Entry {
                        is_pub: e.is_pub,
                        def: e.name.span,
                    });
                }
                Item::Struct(s) => {
                    structs.entry(s.name.name.clone()).or_insert(Entry {
                        is_pub: s.is_pub,
                        def: s.name.span,
                    });
                }
                Item::Enum(_) | Item::Use(_) => {}
            }
        }
        Visibility {
            fns,
            structs,
            files: Vec::new(),
            reported: HashSet::new(),
        }
    }

    pub(super) fn with_file_names(mut self, files: Vec<String>) -> Self {
        self.files = files;
        self
    }

    fn file_name(&self, id: FileId) -> String {
        self.files
            .get(id.0 as usize)
            .cloned()
            .unwrap_or_else(|| format!("<file {}>", id.0))
    }

    /// The E0281 diagnostic for naming `name` at `at`, if it is private to
    /// another file (reported once per use site).
    pub(super) fn check(&mut self, kind: VisKind, name: &str, at: Span) -> Option<Diagnostic> {
        let table = match kind {
            VisKind::Fn => &self.fns,
            VisKind::Struct => &self.structs,
        };
        let entry = table.get(name)?;
        if entry.is_pub || at.is_dummy() || entry.def.is_dummy() || entry.def.file == at.file {
            return None;
        }
        let (def, what) = (
            entry.def,
            match kind {
                VisKind::Fn => "function",
                VisKind::Struct => "struct",
            },
        );
        if !self.reported.insert((at.file, at.start.0)) {
            return None;
        }
        let file = self.file_name(def.file);
        Some(
            Diagnostic::error(format!("`{name}` is private to `{file}`"), at)
                .with_code("E0281")
                .with_note(format!(
                    "{what} `{name}` is defined at {file}:{}:{} without `pub`",
                    def.line, def.column
                ))
                .with_help(format!("mark it `pub` in {file}")),
        )
    }
}
