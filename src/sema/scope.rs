use crate::span::Span;
use crate::ty::Type;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefKind {
    Local,
    Function,
    Struct,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub ty: Type,
    pub mutable: bool,
    pub kind: DefKind,
    pub span: Span,
}

#[derive(Debug, Default)]
pub struct ScopeStack {
    frames: Vec<HashMap<String, Symbol>>,
}

impl ScopeStack {
    pub fn new() -> Self {
        ScopeStack {
            frames: vec![HashMap::new()],
        }
    }

    pub fn push(&mut self) {
        self.frames.push(HashMap::new());
    }

    pub fn pop(&mut self) {
        if self.frames.len() > 1 {
            self.frames.pop();
        }
    }

    /// Returns `false` if the name already existed in an outer scope (shadowing).
    pub fn define(&mut self, name: String, sym: Symbol) -> bool {
        let shadowed = self.lookup(&name).is_some();
        if let Some(top) = self.frames.last_mut() {
            top.insert(name, sym);
        }
        !shadowed
    }

    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        for frame in self.frames.iter().rev() {
            if let Some(s) = frame.get(name) {
                return Some(s);
            }
        }
        None
    }

    pub fn lookup_mut(&mut self, name: &str) -> Option<&mut Symbol> {
        for frame in self.frames.iter_mut().rev() {
            if frame.contains_key(name) {
                return frame.get_mut(name);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::Type;

    fn sym(name: &str) -> Symbol {
        Symbol {
            name: name.into(),
            ty: Type::I32,
            mutable: false,
            kind: DefKind::Local,
            span: Span::DUMMY,
        }
    }

    #[test]
    fn nested_lookup_and_shadow() {
        let mut s = ScopeStack::new();
        s.define("x".into(), sym("x"));
        s.push();
        let first = s.define("x".into(), sym("x"));
        assert!(!first);
        assert!(s.lookup("x").is_some());
        s.pop();
        assert!(s.lookup("x").is_some());
        assert!(s.lookup("y").is_none());
    }
}
