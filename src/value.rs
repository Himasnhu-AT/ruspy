//! Runtime values + the numeric tower.
//!
//! The old `RuspyType` conflated the runtime value with the static type and its
//! `Add`/`Sub`/`Mul`/`Div` impls **panicked** on a type mismatch or divide-by-zero
//! (RUSPY_SCHEMA.md §5). Here the value domain collapses integer/float widths
//! (widths live in `Ty`), and every operation returns `Result<Value, Diag>` so a
//! bad program yields a diagnostic, never a crash.

use crate::diagnostics::{Diag, Span};
use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

/// A shared, mutable array — the storage behind `Value::Array`. `Rc<RefCell<..>>`
/// gives reference semantics: passing an array to a function and pushing to it
/// mutates the caller's array.
pub type ArrayRef = Rc<RefCell<Vec<Value>>>;

/// A runtime value. Integer widths collapse to `Int(i64)` and float widths to
/// `Float(f64)`; the static width is tracked separately by the type checker.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Char(char),
    Array(ArrayRef),
    /// A record instance: the struct's name + its named fields (reference semantics).
    Struct(String, Rc<RefCell<std::collections::HashMap<String, Value>>>),
    Void,
}

impl Value {
    /// Construct an array value from a vector of elements.
    pub fn array(items: Vec<Value>) -> Value {
        Value::Array(Rc::new(RefCell::new(items)))
    }
    /// Construct a struct value from a name + field map.
    pub fn strukt(name: impl Into<String>, fields: std::collections::HashMap<String, Value>) -> Value {
        Value::Struct(name.into(), Rc::new(RefCell::new(fields)))
    }
}

/// Binary operators over values (arithmetic + comparison). Kept small and explicit
/// so the tree-walker, the VM, and Cranelift all lower the same set identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Bool(_) => "bool",
            Value::Str(_) => "str",
            Value::Char(_) => "char",
            Value::Array(_) => "array",
            Value::Struct(_, _) => "struct",
            Value::Void => "void",
        }
    }

    /// Numeric coercion to f64 for mixed arithmetic + comparison. `None` for
    /// non-numeric values. Public so hosts (e.g. `ruspy_host`) can read numeric
    /// arguments passed from a strategy.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Truthiness for conditions — strictly `Bool` (there is no implicit int→bool;
    /// the checker enforces this statically, this is the runtime backstop).
    pub fn as_bool(&self, span: Span) -> Result<bool, Diag> {
        match self {
            Value::Bool(b) => Ok(*b),
            other => Err(Diag::error(
                span,
                "E0201",
                format!("condition must be `bool`, found `{}`", other.type_name()),
            )
            .with_help("use a comparison, e.g. `x == 1`")),
        }
    }
}

/// Apply a binary operator, promoting `int`↔`float` for mixed arithmetic.
///
/// Integer `/` and `%` by zero are diagnostics (Rust would panic); float `/` by
/// zero follows IEEE-754 (→ `inf`/`nan`) to match the native Cranelift backend.
pub fn binop(op: BinOp, l: &Value, r: &Value, span: Span) -> Result<Value, Diag> {
    use BinOp::*;

    // Equality/inequality work structurally across all value kinds.
    match op {
        Eq => return Ok(Value::Bool(values_equal(l, r))),
        Ne => return Ok(Value::Bool(!values_equal(l, r))),
        _ => {}
    }

    // String `+` concatenates; any other string op is a type error.
    if let (Value::Str(a), Value::Str(b)) = (l, r) {
        return match op {
            Add => Ok(Value::Str(format!("{a}{b}"))),
            Lt => Ok(Value::Bool(a < b)),
            Le => Ok(Value::Bool(a <= b)),
            Gt => Ok(Value::Bool(a > b)),
            Ge => Ok(Value::Bool(a >= b)),
            _ => Err(type_err(op, l, r, span)),
        };
    }

    // Numeric path.
    match (l, r) {
        // Both integers → stay integer (exact), except a division that must guard 0.
        (Value::Int(a), Value::Int(b)) => {
            let (a, b) = (*a, *b);
            match op {
                Add => Ok(Value::Int(a.wrapping_add(b))),
                Sub => Ok(Value::Int(a.wrapping_sub(b))),
                Mul => Ok(Value::Int(a.wrapping_mul(b))),
                Div if b == 0 => Err(div_zero(span)),
                Div => Ok(Value::Int(a.wrapping_div(b))),
                Rem if b == 0 => Err(div_zero(span)),
                Rem => Ok(Value::Int(a.wrapping_rem(b))),
                Lt => Ok(Value::Bool(a < b)),
                Le => Ok(Value::Bool(a <= b)),
                Gt => Ok(Value::Bool(a > b)),
                Ge => Ok(Value::Bool(a >= b)),
                Eq | Ne => unreachable!("handled above"),
            }
        }
        // At least one float (and both numeric) → promote to f64 (IEEE).
        _ => match (l.as_f64(), r.as_f64()) {
            (Some(a), Some(b)) => match op {
                Add => Ok(Value::Float(a + b)),
                Sub => Ok(Value::Float(a - b)),
                Mul => Ok(Value::Float(a * b)),
                Div => Ok(Value::Float(a / b)), // IEEE: /0.0 → inf/nan
                Rem => Ok(Value::Float(a % b)),
                Lt => Ok(Value::Bool(a < b)),
                Le => Ok(Value::Bool(a <= b)),
                Gt => Ok(Value::Bool(a > b)),
                Ge => Ok(Value::Bool(a >= b)),
                Eq | Ne => unreachable!("handled above"),
            },
            _ => Err(type_err(op, l, r, span)),
        },
    }
}

/// Unary negation (`-x`), promoting nothing.
pub fn neg(v: &Value, span: Span) -> Result<Value, Diag> {
    match v {
        Value::Int(i) => Ok(Value::Int(i.wrapping_neg())),
        Value::Float(f) => Ok(Value::Float(-f)),
        other => Err(Diag::error(span, "E0202", format!("cannot negate `{}`", other.type_name()))),
    }
}

/// Logical not (`!x`) — strictly on `bool`.
pub fn not(v: &Value, span: Span) -> Result<Value, Diag> {
    match v {
        Value::Bool(b) => Ok(Value::Bool(!b)),
        other => Err(Diag::error(span, "E0203", format!("`!` expects `bool`, found `{}`", other.type_name()))),
    }
}

/// Structural equality with numeric cross-type comparison (`1 == 1.0` is true).
fn values_equal(l: &Value, r: &Value) -> bool {
    match (l, r) {
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Char(a), Value::Char(b)) => a == b,
        (Value::Array(a), Value::Array(b)) => *a.borrow() == *b.borrow(),
        (Value::Struct(na, a), Value::Struct(nb, b)) => na == nb && *a.borrow() == *b.borrow(),
        (Value::Void, Value::Void) => true,
        _ => match (l.as_f64(), r.as_f64()) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
    }
}

fn type_err(op: BinOp, l: &Value, r: &Value, span: Span) -> Diag {
    Diag::error(
        span,
        "E0204",
        format!("operator `{}` cannot apply to `{}` and `{}`", op_symbol(op), l.type_name(), r.type_name()),
    )
}

fn div_zero(span: Span) -> Diag {
    Diag::error(span, "E0205", "division by zero").with_help("guard the divisor, e.g. `if d != 0 { .. }`")
}

fn op_symbol(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Str(s) => write!(f, "{s}"),
            Value::Char(c) => write!(f, "{c}"),
            Value::Array(a) => {
                write!(f, "[")?;
                for (i, v) in a.borrow().iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            Value::Struct(name, fields) => {
                write!(f, "{name} {{ ")?;
                let map = fields.borrow();
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {}", map[*k])?;
                }
                write!(f, " }}")
            }
            Value::Void => write!(f, "void"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sp() -> Span {
        Span::new(0, 1)
    }

    #[test]
    fn integer_arithmetic_stays_integer() {
        assert_eq!(binop(BinOp::Add, &Value::Int(5), &Value::Int(3), sp()).unwrap(), Value::Int(8));
        assert_eq!(binop(BinOp::Div, &Value::Int(7), &Value::Int(2), sp()).unwrap(), Value::Int(3));
        assert_eq!(binop(BinOp::Rem, &Value::Int(7), &Value::Int(2), sp()).unwrap(), Value::Int(1));
    }

    #[test]
    fn int_float_mixing_promotes_to_float() {
        // The old RuspyType panicked here ("Incompatible types for addition").
        assert_eq!(binop(BinOp::Add, &Value::Int(1), &Value::Float(0.5), sp()).unwrap(), Value::Float(1.5));
        assert_eq!(binop(BinOp::Mul, &Value::Float(2.0), &Value::Int(3), sp()).unwrap(), Value::Float(6.0));
    }

    #[test]
    fn integer_div_or_rem_by_zero_is_a_diag_not_a_panic() {
        let e = binop(BinOp::Div, &Value::Int(5), &Value::Int(0), sp()).unwrap_err();
        assert_eq!(e.code, "E0205");
        assert!(binop(BinOp::Rem, &Value::Int(5), &Value::Int(0), sp()).is_err());
        // float /0.0 follows IEEE (inf), matching the native backend — not an error.
        assert_eq!(binop(BinOp::Div, &Value::Float(1.0), &Value::Float(0.0), sp()).unwrap(), Value::Float(f64::INFINITY));
    }

    #[test]
    fn comparisons_and_equality() {
        assert_eq!(binop(BinOp::Lt, &Value::Int(1), &Value::Int(2), sp()).unwrap(), Value::Bool(true));
        assert_eq!(binop(BinOp::Eq, &Value::Int(1), &Value::Float(1.0), sp()).unwrap(), Value::Bool(true));
        assert_eq!(binop(BinOp::Ne, &Value::Str("a".into()), &Value::Str("b".into()), sp()).unwrap(), Value::Bool(true));
    }

    #[test]
    fn strings_concatenate_but_dont_multiply() {
        assert_eq!(binop(BinOp::Add, &Value::Str("Hello, ".into()), &Value::Str("World!".into()), sp()).unwrap(), Value::Str("Hello, World!".into()));
        assert_eq!(binop(BinOp::Mul, &Value::Str("a".into()), &Value::Str("b".into()), sp()).unwrap_err().code, "E0204");
    }

    #[test]
    fn type_mismatch_is_a_diag() {
        let e = binop(BinOp::Add, &Value::Bool(true), &Value::Int(1), sp()).unwrap_err();
        assert_eq!(e.code, "E0204");
    }

    #[test]
    fn unary_and_bool_ops() {
        assert_eq!(neg(&Value::Int(5), sp()).unwrap(), Value::Int(-5));
        assert_eq!(neg(&Value::Float(2.5), sp()).unwrap(), Value::Float(-2.5));
        assert_eq!(not(&Value::Bool(true), sp()).unwrap(), Value::Bool(false));
        assert_eq!(not(&Value::Int(1), sp()).unwrap_err().code, "E0203");
        assert_eq!(Value::Bool(true).as_bool(sp()).unwrap(), true);
        assert_eq!(Value::Int(1).as_bool(sp()).unwrap_err().code, "E0201");
    }
}
