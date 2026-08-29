//! Static types — the lattice the type checker (M10) works over.
//!
//! Distinct from runtime `Value`: annotations like `int32` / `float64` keep their
//! width here (so the checker can enforce them and the backend can pick storage),
//! while at runtime every integer is an `i64` and every float an `f64`.

use crate::lexer::Token;
use std::fmt;

/// Integer widths ruspy annotations can name. Runtime storage is always `i64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntWidth {
    /// Platform-default (`int`).
    Default,
    I32,
    I64,
}

/// Float widths. Runtime storage is always `f64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatWidth {
    Default,
    F32,
    F64,
}

/// A static type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Int(IntWidth),
    Float(FloatWidth),
    Bool,
    Str,
    Char,
    Void,
    /// Homogeneous dynamic array (M8).
    Array(Box<Ty>),
    /// Function `(params) -> return` (M6).
    Func(Vec<Ty>, Box<Ty>),
    /// A named record type, resolved to its declaration id (M9).
    Struct(u32),
    /// Gradual-typing wildcard: an unannotated value whose type is unknown. Unifies
    /// with everything so untyped code type-checks (no false positives).
    Dynamic,
    /// Poison — an already-reported error, to stop cascades.
    Error,
}

impl Ty {
    /// Is this any integer type?
    pub fn is_int(&self) -> bool {
        matches!(self, Ty::Int(_))
    }
    /// Is this any float type?
    pub fn is_float(&self) -> bool {
        matches!(self, Ty::Float(_))
    }
    pub fn is_numeric(&self) -> bool {
        self.is_int() || self.is_float()
    }

    /// Can a value of type `self` be used where `expected` is wanted? Same type
    /// always; an integer literal/type may widen into a float target (Go-style),
    /// and `Error` unifies with anything to prevent cascades.
    pub fn assignable_to(&self, expected: &Ty) -> bool {
        if self == expected
            || matches!(self, Ty::Error | Ty::Dynamic)
            || matches!(expected, Ty::Error | Ty::Dynamic)
        {
            return true;
        }
        match (self, expected) {
            // any int width → any int width; int → float (widening)
            (Ty::Int(_), Ty::Int(_)) => true,
            (Ty::Int(_), Ty::Float(_)) => true,
            (Ty::Float(_), Ty::Float(_)) => true,
            (Ty::Array(a), Ty::Array(b)) => a.assignable_to(b),
            _ => false,
        }
    }

    /// The result type of numeric `+ - * / %` on `self` and `other`: float if either
    /// is float, else the wider-looking int. `None` if either is non-numeric.
    pub fn arith_result(&self, other: &Ty) -> Option<Ty> {
        match (self, other) {
            (Ty::Error, _) | (_, Ty::Error) => Some(Ty::Error),
            (Ty::Dynamic, _) | (_, Ty::Dynamic) => Some(Ty::Dynamic),
            (a, b) if a.is_float() || b.is_float() => Some(Ty::Float(FloatWidth::Default)),
            (a, b) if a.is_int() && b.is_int() => Some(Ty::Int(IntWidth::Default)),
            _ => None,
        }
    }

    /// The static type named by a type keyword token (`int`, `float64`, `str`, …).
    /// `str8/str32/str64` are accepted as `Str` here — the checker/parser decide
    /// whether to reject the width-suffixed string forms.
    pub fn from_token(tok: &Token) -> Option<Ty> {
        Some(match tok {
            Token::TypeInt => Ty::Int(IntWidth::Default),
            Token::TypeInt32 => Ty::Int(IntWidth::I32),
            Token::TypeInt64 => Ty::Int(IntWidth::I64),
            Token::TypeFloat => Ty::Float(FloatWidth::Default),
            Token::TypeFloat32 => Ty::Float(FloatWidth::F32),
            Token::TypeFloat64 => Ty::Float(FloatWidth::F64),
            Token::TypeChar => Ty::Char,
            Token::TypeBool => Ty::Bool,
            Token::TypeStr | Token::TypeStr8 | Token::TypeStr32 | Token::TypeStr64 => Ty::Str,
            _ => return None,
        })
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Int(IntWidth::Default) => write!(f, "int"),
            Ty::Int(IntWidth::I32) => write!(f, "int32"),
            Ty::Int(IntWidth::I64) => write!(f, "int64"),
            Ty::Float(FloatWidth::Default) => write!(f, "float"),
            Ty::Float(FloatWidth::F32) => write!(f, "float32"),
            Ty::Float(FloatWidth::F64) => write!(f, "float64"),
            Ty::Bool => write!(f, "bool"),
            Ty::Str => write!(f, "str"),
            Ty::Char => write!(f, "char"),
            Ty::Void => write!(f, "void"),
            Ty::Array(t) => write!(f, "[{t}]"),
            Ty::Func(ps, r) => {
                write!(f, "fn(")?;
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {r}")
            }
            Ty::Struct(id) => write!(f, "struct#{id}"),
            Ty::Dynamic => write!(f, "any"),
            Ty::Error => write!(f, "<error>"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_token_maps_all_type_keywords() {
        assert_eq!(Ty::from_token(&Token::TypeInt64), Some(Ty::Int(IntWidth::I64)));
        assert_eq!(Ty::from_token(&Token::TypeFloat32), Some(Ty::Float(FloatWidth::F32)));
        assert_eq!(Ty::from_token(&Token::TypeStr64), Some(Ty::Str)); // width-suffixed str → Str
        assert_eq!(Ty::from_token(&Token::Plus), None);
    }

    #[test]
    fn assignability_widens_int_to_float_but_not_the_reverse() {
        let i = Ty::Int(IntWidth::Default);
        let i64t = Ty::Int(IntWidth::I64);
        let f = Ty::Float(FloatWidth::F64);
        assert!(i.assignable_to(&i64t), "int widths interassign");
        assert!(i.assignable_to(&f), "int widens to float");
        assert!(!f.assignable_to(&i), "float does NOT narrow to int");
        assert!(!Ty::Bool.assignable_to(&i));
        // Error poison unifies both ways.
        assert!(Ty::Error.assignable_to(&Ty::Bool) && Ty::Bool.assignable_to(&Ty::Error));
    }

    #[test]
    fn arith_result_promotes_to_float() {
        let i = Ty::Int(IntWidth::Default);
        let f = Ty::Float(FloatWidth::Default);
        assert_eq!(i.arith_result(&i), Some(Ty::Int(IntWidth::Default)));
        assert_eq!(i.arith_result(&f), Some(Ty::Float(FloatWidth::Default)));
        assert_eq!(Ty::Bool.arith_result(&i), None);
    }

    #[test]
    fn display_is_readable() {
        assert_eq!(Ty::Array(Box::new(Ty::Float(FloatWidth::F64))).to_string(), "[float64]");
        assert_eq!(Ty::Func(vec![Ty::Int(IntWidth::Default)], Box::new(Ty::Bool)).to_string(), "fn(int) -> bool");
    }
}
