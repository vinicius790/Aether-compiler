//! Type system of Aether.
//!
//! Types are interned by value through [`Type`]. Compatibility and operator
//! rules live here so neither the semantic analyzer nor the backends invent
//! their own conversions.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Unit,
    Bool,
    I32,
    I64,
    F64,
    String,
    Char,
    Array { elem: Box<Type>, len: i64 },
    Struct { name: String, fields: Vec<(String, Type)> },
    /// `(T1, T2, ...)`, at least two elements. Laid out like a struct whose
    /// fields are named `0`, `1`, ...
    Tuple(Vec<Type>),
    /// `enum Name { V1(T, ...), V2, ... }`. Laid out like a struct: field 0
    /// is the variant tag (`i32`), fields `1..=max_payload` the payload
    /// slots (slot `i` holds payload `i-1` of the active variant).
    Enum {
        name: String,
        variants: Vec<(String, Vec<Type>)>,
    },
    Fn { params: Vec<Type>, ret: Box<Type> },
    /// Produced when type checking fails; suppresses cascading errors.
    Error,
}

impl Type {
    pub fn is_numeric(&self) -> bool {
        matches!(self, Type::I32 | Type::I64 | Type::F64)
    }

    pub fn is_integer(&self) -> bool {
        matches!(self, Type::I32 | Type::I64)
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Type::Error)
    }

    pub fn size_bytes(&self) -> u32 {
        match self {
            Type::Unit | Type::Error => 0,
            Type::Bool | Type::Char => 1,
            Type::I32 => 4,
            Type::I64 | Type::F64 => 8,
            Type::String => 8, // pointer + length packed as handle
            Type::Array { elem, len } => elem.size_bytes() * (*len as u32).max(0),
            Type::Struct { fields, .. } => fields.iter().map(|(_, t)| t.size_bytes()).sum(),
            Type::Tuple(elems) => elems.iter().map(|t| t.size_bytes()).sum(),
            Type::Enum { .. } => self
                .layout_fields()
                .map(|f| f.iter().map(|t| t.size_bytes()).sum())
                .unwrap_or(0),
            Type::Fn { .. } => 8,
        }
    }

    pub fn field(&self, name: &str) -> Option<(usize, &Type)> {
        match self {
            Type::Struct { fields, .. } => fields
                .iter()
                .enumerate()
                .find(|(_, (n, _))| n == name)
                .map(|(i, (_, t))| (i, t)),
            Type::Tuple(elems) => {
                if !name.bytes().all(|c| c.is_ascii_digit()) {
                    return None;
                }
                let i: usize = name.parse().ok()?;
                elems.get(i).map(|t| (i, t))
            }
            _ => None,
        }
    }

    /// Field types of an aggregate laid out as an object with indexed
    /// fields: a struct's fields, a tuple's elements, or an enum's tag
    /// followed by its payload slots (slot `i` is typed by the first variant
    /// that uses it).
    pub fn layout_fields(&self) -> Option<Vec<Type>> {
        match self {
            Type::Struct { fields, .. } => Some(fields.iter().map(|(_, t)| t.clone()).collect()),
            Type::Tuple(elems) => Some(elems.clone()),
            Type::Enum { variants, .. } => {
                let slots = variants.iter().map(|(_, p)| p.len()).max().unwrap_or(0);
                let mut out = vec![Type::I32];
                for i in 0..slots {
                    let t = variants
                        .iter()
                        .find_map(|(_, p)| p.get(i).cloned())
                        .unwrap_or(Type::Unit);
                    out.push(t);
                }
                Some(out)
            }
            _ => None,
        }
    }

    /// Index of a variant and its payload types.
    pub fn variant(&self, name: &str) -> Option<(usize, &[Type])> {
        match self {
            Type::Enum { variants, .. } => variants
                .iter()
                .enumerate()
                .find(|(_, (n, _))| n == name)
                .map(|(i, (_, p))| (i, p.as_slice())),
            _ => None,
        }
    }

    /// True when two variants put different types into the same payload
    /// slot. The VM is untyped and does not care; the LLVM backend cannot
    /// give such a slot one static type.
    pub fn enum_has_slot_conflict(&self) -> bool {
        match self {
            Type::Enum { variants, .. } => {
                let slots = variants.iter().map(|(_, p)| p.len()).max().unwrap_or(0);
                (0..slots).any(|i| {
                    let mut tys = variants.iter().filter_map(|(_, p)| p.get(i));
                    match tys.next() {
                        Some(first) => tys.any(|t| t != first),
                        None => false,
                    }
                })
            }
            _ => false,
        }
    }

    /// Whether `==` / `!=` are defined: everything but `unit`, functions and
    /// aggregates containing those.
    pub fn supports_eq(&self) -> bool {
        match self {
            Type::Unit | Type::Fn { .. } => false,
            Type::Error => true,
            Type::Array { elem, .. } => elem.supports_eq(),
            Type::Struct { fields, .. } => fields.iter().all(|(_, t)| t.supports_eq()),
            Type::Tuple(elems) => elems.iter().all(|t| t.supports_eq()),
            Type::Enum { variants, .. } => variants
                .iter()
                .all(|(_, p)| p.iter().all(|t| t.supports_eq())),
            _ => true,
        }
    }

    /// Implicitly compatible (no cast needed).
    pub fn assignable_from(&self, other: &Type) -> bool {
        if self.is_error() || other.is_error() {
            return true;
        }
        self == other
    }

    /// Explicit `as` conversions that are allowed.
    pub fn can_cast_to(&self, target: &Type) -> bool {
        if self == target {
            return true;
        }
        matches!(
            (self, target),
            (Type::I32, Type::I64)
                | (Type::I64, Type::I32)
                | (Type::I32, Type::F64)
                | (Type::I64, Type::F64)
                | (Type::F64, Type::I32)
                | (Type::F64, Type::I64)
                | (Type::Bool, Type::I32)
                | (Type::Bool, Type::I64)
                | (Type::Char, Type::I32)
                | (Type::I32, Type::Char)
        )
    }

    pub fn llvm_name(&self) -> String {
        match self {
            Type::Unit => "void".into(),
            Type::Bool => "i1".into(),
            Type::I32 => "i32".into(),
            Type::I64 => "i64".into(),
            Type::F64 => "double".into(),
            Type::String => "ptr".into(),
            Type::Char => "i8".into(),
            Type::Array { elem, len } => format!("[{len} x {}]", elem.llvm_name()),
            Type::Struct { name, .. } | Type::Enum { name, .. } => format!("%struct.{name}"),
            Type::Tuple(elems) => {
                let p: Vec<_> = elems.iter().map(|t| t.llvm_name()).collect();
                format!("{{ {} }}", p.join(", "))
            }
            Type::Fn { params, ret } => {
                let p: Vec<_> = params.iter().map(|t| t.llvm_name()).collect();
                format!("{} ({})", ret.llvm_name(), p.join(", "))
            }
            Type::Error => "i32".into(),
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Unit => write!(f, "unit"),
            Type::Bool => write!(f, "bool"),
            Type::I32 => write!(f, "i32"),
            Type::I64 => write!(f, "i64"),
            Type::F64 => write!(f, "f64"),
            Type::String => write!(f, "string"),
            Type::Char => write!(f, "char"),
            Type::Array { elem, len } => write!(f, "[{elem}; {len}]"),
            Type::Struct { name, .. } | Type::Enum { name, .. } => write!(f, "{name}"),
            Type::Tuple(elems) => {
                let p: Vec<_> = elems.iter().map(|t| t.to_string()).collect();
                write!(f, "({})", p.join(", "))
            }
            Type::Fn { params, ret } => {
                let p: Vec<_> = params.iter().map(|t| t.to_string()).collect();
                write!(f, "fn({}) -> {ret}", p.join(", "))
            }
            Type::Error => write!(f, "{{error}}"),
        }
    }
}

pub fn parse_named_type(name: &str) -> Option<Type> {
    Some(match name {
        "i32" => Type::I32,
        "i64" => Type::I64,
        "f64" => Type::F64,
        "bool" => Type::Bool,
        "string" => Type::String,
        "char" => Type::Char,
        "unit" | "()" => Type::Unit,
        _ => return None,
    })
}

/// Result type of a binary operator given operand types.
pub fn binop_result(op: crate::ast::BinOp, lhs: &Type, rhs: &Type) -> Option<Type> {
    use crate::ast::BinOp::*;
    if lhs.is_error() || rhs.is_error() {
        return Some(Type::Error);
    }
    match op {
        Add | Sub | Mul | Div | Rem => {
            if lhs == rhs && lhs.is_numeric() {
                if op == Rem && matches!(lhs, Type::F64) {
                    return None;
                }
                Some(lhs.clone())
            } else if op == Add && *lhs == Type::String && *rhs == Type::String {
                Some(Type::String)
            } else {
                None
            }
        }
        Eq | Ne => {
            if lhs == rhs && lhs.supports_eq() {
                Some(Type::Bool)
            } else {
                None
            }
        }
        Lt | Le | Gt | Ge => {
            if lhs == rhs && (lhs.is_numeric() || *lhs == Type::Char) {
                Some(Type::Bool)
            } else {
                None
            }
        }
        And | Or => {
            if *lhs == Type::Bool && *rhs == Type::Bool {
                Some(Type::Bool)
            } else {
                None
            }
        }
        // Same-type integers only; the shift amount has the type of the
        // shifted value and the result keeps that type.
        BitAnd | BitOr | BitXor | Shl | Shr => {
            if lhs == rhs && lhs.is_integer() {
                Some(lhs.clone())
            } else {
                None
            }
        }
    }
}

/// `-` on numbers; `!` is logical not on `bool` and bitwise not on integers.
pub fn unop_result(op: crate::ast::UnOp, inner: &Type) -> Option<Type> {
    use crate::ast::UnOp::*;
    if inner.is_error() {
        return Some(Type::Error);
    }
    match op {
        Neg if inner.is_numeric() => Some(inner.clone()),
        Not if *inner == Type::Bool => Some(Type::Bool),
        Not if inner.is_integer() => Some(inner.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::BinOp;

    #[test]
    fn numeric_add() {
        assert_eq!(
            binop_result(BinOp::Add, &Type::I32, &Type::I32),
            Some(Type::I32)
        );
        assert!(binop_result(BinOp::Add, &Type::I32, &Type::I64).is_none());
    }

    #[test]
    fn bitwise_is_integer_only_and_same_typed() {
        use crate::ast::UnOp;
        for op in [BinOp::BitAnd, BinOp::BitOr, BinOp::BitXor, BinOp::Shl, BinOp::Shr] {
            assert_eq!(binop_result(op, &Type::I32, &Type::I32), Some(Type::I32));
            assert_eq!(binop_result(op, &Type::I64, &Type::I64), Some(Type::I64));
            assert!(binop_result(op, &Type::I32, &Type::I64).is_none());
            assert!(binop_result(op, &Type::I64, &Type::I32).is_none());
            assert!(binop_result(op, &Type::Bool, &Type::Bool).is_none());
            assert!(binop_result(op, &Type::F64, &Type::F64).is_none());
        }
        assert_eq!(unop_result(UnOp::Not, &Type::Bool), Some(Type::Bool));
        assert_eq!(unop_result(UnOp::Not, &Type::I32), Some(Type::I32));
        assert_eq!(unop_result(UnOp::Not, &Type::I64), Some(Type::I64));
        assert!(unop_result(UnOp::Not, &Type::F64).is_none());
        assert!(unop_result(UnOp::Not, &Type::String).is_none());
    }

    #[test]
    fn tuples_and_enums() {
        let t = Type::Tuple(vec![Type::I32, Type::Bool]);
        assert_eq!(t.to_string(), "(i32, bool)");
        assert_eq!(t.field("1"), Some((1, &Type::Bool)));
        assert!(t.field("2").is_none());
        assert!(t.field("x").is_none());
        assert_eq!(binop_result(BinOp::Eq, &t, &t), Some(Type::Bool));
        assert!(binop_result(BinOp::Lt, &t, &t).is_none());
        let e = Type::Enum {
            name: "Shape".into(),
            variants: vec![
                ("Circle".into(), vec![Type::F64]),
                ("Rect".into(), vec![Type::I32, Type::I32]),
                ("Empty".into(), vec![]),
            ],
        };
        assert_eq!(e.to_string(), "Shape");
        assert_eq!(
            e.layout_fields(),
            Some(vec![Type::I32, Type::F64, Type::I32])
        );
        assert_eq!(e.variant("Rect").map(|(i, p)| (i, p.len())), Some((1, 2)));
        assert!(e.enum_has_slot_conflict());
        assert_eq!(binop_result(BinOp::Ne, &e, &e), Some(Type::Bool));
        assert!(binop_result(BinOp::Le, &e, &e).is_none());
        assert!(!e.can_cast_to(&Type::I32));
        let unit_tuple = Type::Tuple(vec![Type::Unit, Type::I32]);
        assert!(binop_result(BinOp::Eq, &unit_tuple, &unit_tuple).is_none());
    }

    #[test]
    fn casts() {
        assert!(Type::I32.can_cast_to(&Type::F64));
        assert!(!Type::String.can_cast_to(&Type::I32));
    }
}
