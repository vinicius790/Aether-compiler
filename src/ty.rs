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
            _ => None,
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
            Type::Struct { name, .. } => format!("%struct.{name}"),
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
            Type::Struct { name, .. } => write!(f, "{name}"),
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
            if lhs == rhs && !matches!(lhs, Type::Unit | Type::Fn { .. }) {
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
    }
}

pub fn unop_result(op: crate::ast::UnOp, inner: &Type) -> Option<Type> {
    use crate::ast::UnOp::*;
    if inner.is_error() {
        return Some(Type::Error);
    }
    match op {
        Neg if inner.is_numeric() => Some(inner.clone()),
        Not if *inner == Type::Bool => Some(Type::Bool),
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
    fn casts() {
        assert!(Type::I32.can_cast_to(&Type::F64));
        assert!(!Type::String.can_cast_to(&Type::I32));
    }
}
