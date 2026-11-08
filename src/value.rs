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
    /// The absent value (`null`): "no entity", a missing result.
    Null,
    /// An opaque host object handle (an entity, a resource). The host decides what
    /// `h.field`, `h.field = v` and `h.method(args)` mean. Equality is by id; prints `Node#<id>`.
    Object(u64),
    /// A string-keyed (or int/bool/char-keyed) dictionary with insertion order and
    /// reference semantics, like arrays.
    Dict(DictRef),
    /// 2D / 3D vector and RGBA color value types (copied on assignment, like numbers).
    Vec2([f64; 2]),
    Vec3([f64; 3]),
    Color([f64; 4]),
}

/// Components of a vector-like value: `(kind, components, count)`; kind 2 = Vec2, 3 = Vec3, 4 = Color.
pub fn vec_parts(v: &Value) -> Option<(u8, [f64; 4], usize)> {
    match v {
        Value::Vec2(a) => Some((2, [a[0], a[1], 0.0, 0.0], 2)),
        Value::Vec3(a) => Some((3, [a[0], a[1], a[2], 0.0], 3)),
        Value::Color(a) => Some((4, *a, 4)),
        _ => None,
    }
}

/// Builds a vector-like value of `kind` from components.
pub fn vec_make(kind: u8, c: [f64; 4]) -> Value {
    match kind {
        2 => Value::Vec2([c[0], c[1]]),
        3 => Value::Vec3([c[0], c[1], c[2]]),
        _ => Value::Color(c),
    }
}

/// Shared storage behind `Value::Dict`.
pub type DictRef = Rc<RefCell<Dict>>;

/// A hashable dictionary key (only scalar kinds; floats and null are rejected).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DictKey {
    Int(i64),
    Str(String),
    Bool(bool),
    Char(char),
}

impl DictKey {
    pub fn from_value(v: &Value) -> Option<DictKey> {
        Some(match v {
            Value::Int(i) => DictKey::Int(*i),
            Value::Str(s) => DictKey::Str(s.clone()),
            Value::Bool(b) => DictKey::Bool(*b),
            Value::Char(c) => DictKey::Char(*c),
            _ => return None,
        })
    }
    pub fn to_value(&self) -> Value {
        match self {
            DictKey::Int(i) => Value::Int(*i),
            DictKey::Str(s) => Value::Str(s.clone()),
            DictKey::Bool(b) => Value::Bool(*b),
            DictKey::Char(c) => Value::Char(*c),
        }
    }
}

/// An insertion-ordered map. Lookup is O(1); `remove` is O(n) (reindexes).
#[derive(Debug, Clone, Default)]
pub struct Dict {
    entries: Vec<(DictKey, Value)>,
    index: std::collections::HashMap<DictKey, usize>,
}

impl Dict {
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn get(&self, k: &DictKey) -> Option<&Value> {
        self.index.get(k).map(|&i| &self.entries[i].1)
    }
    pub fn get_str(&self, k: &str) -> Option<&Value> {
        self.get(&DictKey::Str(k.to_string()))
    }
    pub fn contains(&self, k: &DictKey) -> bool {
        self.index.contains_key(k)
    }
    pub fn insert(&mut self, k: DictKey, v: Value) {
        match self.index.get(&k) {
            Some(&i) => self.entries[i].1 = v,
            None => {
                self.index.insert(k.clone(), self.entries.len());
                self.entries.push((k, v));
            }
        }
    }
    pub fn remove(&mut self, k: &DictKey) -> Option<Value> {
        let i = self.index.remove(k)?;
        let (_, v) = self.entries.remove(i);
        for (j, (key, _)) in self.entries.iter().enumerate().skip(i) {
            self.index.insert(key.clone(), j);
        }
        Some(v)
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
    }
    pub fn iter(&self) -> impl Iterator<Item = &(DictKey, Value)> {
        self.entries.iter()
    }
}

impl PartialEq for Dict {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.entries.iter().all(|(k, v)| other.get(k).is_some_and(|o| values_equal(v, o)))
    }
}

impl Value {
    /// Construct an array value from a vector of elements.
    pub fn array(items: Vec<Value>) -> Value {
        Value::Array(Rc::new(RefCell::new(items)))
    }
    /// Construct an object handle.
    pub fn object(id: u64) -> Value {
        Value::Object(id)
    }
    /// Construct a dictionary from key/value pairs (insertion order kept). Keys that are not
    /// str/int/bool/char are skipped.
    pub fn dict(pairs: Vec<(Value, Value)>) -> Value {
        let mut d = Dict::default();
        for (k, v) in pairs {
            if let Some(k) = DictKey::from_value(&k) {
                d.insert(k, v);
            }
        }
        Value::Dict(Rc::new(RefCell::new(d)))
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
            Value::Null => "null",
            Value::Object(_) => "object",
            Value::Dict(_) => "dict",
            Value::Vec2(_) => "Vec2",
            Value::Vec3(_) => "Vec3",
            Value::Color(_) => "Color",
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

    // `str + scalar` / `scalar + str` concatenates (the scalar prints as `tostr` would).
    if op == Add {
        match (l, r) {
            (Value::Str(a), b @ (Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Char(_))) => {
                return Ok(Value::Str(format!("{a}{b}")))
            }
            (a @ (Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Char(_)), Value::Str(b)) => {
                return Ok(Value::Str(format!("{a}{b}")))
            }
            _ => {}
        }
    }

    // Vectors: `+ -` componentwise, `* /` by a scalar or componentwise, `%` formats strings.
    if let Some(res) = vec_binop(op, l, r) {
        return res.ok_or_else(|| type_err(op, l, r, span));
    }
    if op == Rem {
        if let Value::Str(f) = l {
            return Ok(Value::Str(format_percent(f, r)));
        }
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

/// `Some(Some(v))` result, `Some(None)` vector operands that don't fit, `None` not a vector op.
fn vec_binop(op: BinOp, l: &Value, r: &Value) -> Option<Option<Value>> {
    use BinOp::*;
    let (lv, rv) = (vec_parts(l), vec_parts(r));
    if lv.is_none() && rv.is_none() {
        return None;
    }
    if !matches!(op, Add | Sub | Mul | Div) {
        return Some(None);
    }
    match (lv, rv) {
        (Some((k, a, n)), Some((k2, b, _))) => {
            if k != k2 {
                return Some(None);
            }
            let mut o = [0.0; 4];
            for i in 0..n {
                o[i] = match op {
                    Add => a[i] + b[i],
                    Sub => a[i] - b[i],
                    Mul => a[i] * b[i],
                    _ => a[i] / b[i],
                };
            }
            Some(Some(vec_make(k, o)))
        }
        (Some((k, a, n)), None) => {
            let s = r.as_f64()?;
            let mut o = [0.0; 4];
            for i in 0..n {
                o[i] = match op {
                    Mul => a[i] * s,
                    Div => a[i] / s,
                    _ => return Some(None),
                };
            }
            Some(Some(vec_make(k, o)))
        }
        (None, Some((k, b, n))) => {
            let s = l.as_f64()?;
            let mut o = [0.0; 4];
            for i in 0..n {
                o[i] = match op {
                    Mul => s * b[i],
                    _ => return Some(None),
                };
            }
            Some(Some(vec_make(k, o)))
        }
        (None, None) => None,
    }
}

/// `"%s has %d (%.2f)" % [a, b, c]` (a single non-array value is one argument; `%%` is `%`).
fn format_percent(fmt: &str, args: &Value) -> String {
    let items: Vec<Value> = match args {
        Value::Array(a) => a.borrow().clone(),
        other => vec![other.clone()],
    };
    let mut it = items.into_iter();
    let mut out = String::new();
    let cs: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        if cs[i] != '%' {
            out.push(cs[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i < cs.len() && cs[i] == '%' {
            out.push('%');
            i += 1;
            continue;
        }
        let mut prec: Option<usize> = None;
        if i < cs.len() && cs[i] == '.' {
            i += 1;
            let mut p = 0;
            while i < cs.len() && cs[i].is_ascii_digit() {
                p = p * 10 + cs[i] as usize - '0' as usize;
                i += 1;
            }
            prec = Some(p);
        }
        let Some(&c) = cs.get(i) else { break };
        i += 1;
        let v = it.next().unwrap_or(Value::Null);
        match c {
            'd' => out.push_str(&v.as_f64().map_or_else(|| v.to_string(), |x| format!("{}", x.trunc() as i64))),
            'f' => out.push_str(&v.as_f64().map_or_else(|| v.to_string(), |x| format!("{:.*}", prec.unwrap_or(6), x))),
            _ => out.push_str(&v.to_string()),
        }
    }
    out
}

/// Unary negation (`-x`), promoting nothing.
pub fn neg(v: &Value, span: Span) -> Result<Value, Diag> {
    if let Some((k, a, n)) = vec_parts(v) {
        let mut o = [0.0; 4];
        for i in 0..n {
            o[i] = -a[i];
        }
        return Ok(vec_make(k, o));
    }
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
pub fn values_equal(l: &Value, r: &Value) -> bool {
    match (l, r) {
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Char(a), Value::Char(b)) => a == b,
        (Value::Array(a), Value::Array(b)) => *a.borrow() == *b.borrow(),
        (Value::Struct(na, a), Value::Struct(nb, b)) => na == nb && *a.borrow() == *b.borrow(),
        (Value::Object(a), Value::Object(b)) => a == b,
        (Value::Vec2(a), Value::Vec2(b)) => a == b,
        (Value::Vec3(a), Value::Vec3(b)) => a == b,
        (Value::Color(a), Value::Color(b)) => a == b,
        (Value::Dict(a), Value::Dict(b)) => Rc::ptr_eq(a, b) || *a.borrow() == *b.borrow(),
        (Value::Void, Value::Void) => true,
        (Value::Null, Value::Null) => true,
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
            Value::Null => write!(f, "null"),
            Value::Vec2(a) => write!(f, "({}, {})", a[0], a[1]),
            Value::Vec3(a) => write!(f, "({}, {}, {})", a[0], a[1], a[2]),
            Value::Color(a) => write!(f, "Color({}, {}, {}, {})", a[0], a[1], a[2], a[3]),
            Value::Object(id) => write!(f, "Node#{id}"),
            Value::Dict(d) => {
                write!(f, "{{")?;
                for (i, (k, v)) in d.borrow().iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write_quoted(f, &k.to_value())?;
                    write!(f, ": ")?;
                    write_quoted(f, v)?;
                }
                write!(f, "}}")
            }
        }
    }
}

fn write_quoted(f: &mut fmt::Formatter<'_>, v: &Value) -> fmt::Result {
    match v {
        Value::Str(s) => write!(f, "\"{s}\""),
        other => write!(f, "{other}"),
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
