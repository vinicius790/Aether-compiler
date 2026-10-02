//! Item names across files: file-private names and `pub` visibility (E0281).
//!
//! Every file of a program (the main file, its `use` imports and
//! `--include`s) contributes items to one program, but a private item (no
//! `pub`) belongs to the file that defines it:
//!
//! * a name used in file F resolves to F's own item of that name if there is
//!   one, else to the `pub` item of that name (from any file);
//! * two private items of the same name in different files do not clash —
//!   each file sees its own. They get distinct internal names: private items
//!   of files other than the main one are renamed `name$N` (`N` = file id);
//!   `main`, `extern fn`s (the name is the host symbol), `pub` items and
//!   the main file's items keep their name, so two of them still clash;
//! * a `pub` item clashes with every other item of the same name (E0201 /
//!   E0203), as do two items of one file;
//! * naming another file's private item (when no visible item of that name
//!   exists) is E0281. Functions and `extern fn`s share one namespace,
//!   structs and enums another. Built-ins are not subject to the rule.

use crate::ast::{Item, Program};
use crate::diagnostic::Diagnostic;
use crate::span::{FileId, Span};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum VisKind {
    Fn,
    Type,
}

struct Entry {
    /// Index in `Program::items`.
    item: usize,
    internal: String,
    is_pub: bool,
    def: Span,
    what: &'static str,
}

pub(super) struct Visibility {
    /// Non-duplicate items by (namespace, source name), in item order.
    entries: HashMap<(VisKind, String), Vec<Entry>>,
    /// Per item of `Program::items`: its internal name (`None` for `use`)
    /// and whether it clashes with an earlier item.
    items: Vec<(Option<String>, bool)>,
    /// Display names of the session's files, indexed by `FileId`.
    files: Vec<String>,
    reported: HashSet<(FileId, u32)>,
}

fn item_info(item: &Item) -> Option<(VisKind, &str, bool, Span, &'static str)> {
    Some(match item {
        Item::Fn(f) => (VisKind::Fn, &f.name.name, f.is_pub, f.name.span, "function"),
        Item::Extern(e) => (VisKind::Fn, &e.name.name, e.is_pub, e.name.span, "function"),
        Item::Struct(s) => (VisKind::Type, &s.name.name, s.is_pub, s.name.span, "struct"),
        Item::Enum(e) => (VisKind::Type, &e.name.name, e.is_pub, e.name.span, "enum"),
        Item::Use(_) => return None,
    })
}

impl Visibility {
    pub(super) fn new(program: &Program) -> Self {
        // the main file is the first one; its items keep their names
        let main = if program.span.is_dummy() {
            program
                .items
                .first()
                .map(|i| i.span().file)
                .unwrap_or(FileId(0))
        } else {
            program.span.file
        };
        let mut entries: HashMap<(VisKind, String), Vec<Entry>> = HashMap::new();
        let mut items = Vec::with_capacity(program.items.len());
        for (idx, item) in program.items.iter().enumerate() {
            let Some((kind, name, is_pub, def, what)) = item_info(item) else {
                items.push((None, false));
                continue;
            };
            // an `extern fn`'s name is the host symbol it binds to, so it is
            // never renamed (two declarations of it clash, as before)
            let keep = is_pub
                || def.is_dummy()
                || def.file == main
                || name == "main"
                || matches!(item, Item::Extern(_));
            let internal = if keep {
                name.to_string()
            } else {
                format!("{name}${}", def.file.0)
            };
            let list = entries.entry((kind, name.to_string())).or_default();
            let dup = list
                .iter()
                .any(|e| e.internal == internal || e.is_pub || is_pub);
            if !dup {
                list.push(Entry {
                    item: idx,
                    internal: internal.clone(),
                    is_pub,
                    def,
                    what,
                });
            }
            items.push((Some(internal), dup));
        }
        Visibility {
            entries,
            items,
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

    /// Internal name of `Program::items[idx]` and whether it duplicates an
    /// earlier item.
    pub(super) fn item(&self, idx: usize) -> (Option<&str>, bool) {
        match self.items.get(idx) {
            Some((name, dup)) => (name.as_deref(), *dup),
            None => (None, false),
        }
    }

    /// Index in `Program::items` of the item with internal name `internal`.
    pub(super) fn item_of(&self, kind: VisKind, internal: &str) -> Option<usize> {
        let source = crate::ty::source_name(internal);
        self.entries
            .get(&(kind, source.to_string()))?
            .iter()
            .find(|e| e.internal == internal)
            .map(|e| e.item)
    }

    /// Resolves the source name `name` used at `at`: the internal name of
    /// the item it denotes and whether that item is visible there (`false`:
    /// only another file's private item has that name).
    pub(super) fn resolve(&self, kind: VisKind, name: &str, at: Span) -> Option<(String, bool)> {
        let list = self.entries.get(&(kind, name.to_string()))?;
        if at.is_dummy() {
            return list.first().map(|e| (e.internal.clone(), true));
        }
        if let Some(e) = list.iter().find(|e| !e.def.is_dummy() && e.def.file == at.file) {
            return Some((e.internal.clone(), true));
        }
        if let Some(e) = list.iter().find(|e| e.is_pub || e.def.is_dummy()) {
            return Some((e.internal.clone(), true));
        }
        list.first().map(|e| (e.internal.clone(), false))
    }

    /// The E0281 diagnostic for naming another file's private item `name`
    /// (internal name `internal`) at `at`, reported once per use site.
    pub(super) fn private_error(
        &mut self,
        kind: VisKind,
        name: &str,
        internal: &str,
        at: Span,
    ) -> Option<Diagnostic> {
        let (def, what) = self
            .entries
            .get(&(kind, name.to_string()))?
            .iter()
            .find(|e| e.internal == internal)
            .map(|e| (e.def, e.what))?;
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
