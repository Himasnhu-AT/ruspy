//! Methods and members of the built-in value types: vectors/colors and strings.

use crate::diagnostics::{Diag, Span};
use crate::value::{vec_make, vec_parts, Value};

fn err(span: Span, code: &'static str, msg: String) -> Diag {
    Diag::error(span, code, msg)
}

fn arity(name: &str, args: &[Value], want: usize, span: Span) -> Result<(), Diag> {
    if args.len() == want {
        Ok(())
    } else {
        Err(err(span, "E0317", format!("`{name}` takes {want} argument(s), got {}", args.len())))
    }
}

fn component_index(v: &Value, name: &str) -> Option<usize> {
    let (kind, _, n) = vec_parts(v)?;
    let i = match (kind, name) {
        (2 | 3, "x") | (4, "r") => 0,
        (2 | 3, "y") | (4, "g") => 1,
        (3, "z") | (4, "b") => 2,
        (4, "a") => 3,
        _ => return None,
    };
    (i < n).then_some(i)
}

/// `v.x` / `c.r` ...
pub fn vec_get(v: &Value, name: &str) -> Option<Value> {
    let i = component_index(v, name)?;
    vec_parts(v).map(|(_, c, _)| Value::Float(c[i]))
}

/// A copy of `v` with component `name` set (for `v.x = n`).
pub fn vec_set(v: &Value, name: &str, val: f64) -> Option<Value> {
    let i = component_index(v, name)?;
    let (k, mut c, _) = vec_parts(v)?;
    c[i] = val;
    Some(vec_make(k, c))
}

/// Component `i` of a vector (for `v[i]`).
pub fn vec_index(v: &Value, i: i64) -> Option<Value> {
    let (_, c, n) = vec_parts(v)?;
    (i >= 0 && (i as usize) < n).then(|| Value::Float(c[i as usize]))
}

pub fn vec_with_index(v: &Value, i: i64, val: f64) -> Option<Value> {
    let (k, mut c, n) = vec_parts(v)?;
    if i < 0 || i as usize >= n {
        return None;
    }
    c[i as usize] = val;
    Some(vec_make(k, c))
}

fn same_kind(a: &Value, b: &Value, name: &str, span: Span) -> Result<(u8, [f64; 4], [f64; 4], usize), Diag> {
    match (vec_parts(a), vec_parts(b)) {
        (Some((k, x, n)), Some((k2, y, _))) if k == k2 => Ok((k, x, y, n)),
        _ => Err(err(span, "E0350", format!("`{name}` needs two vectors of the same type, got `{}` and `{}`", a.type_name(), b.type_name()))),
    }
}

fn scalar(v: &Value, name: &str, span: Span) -> Result<f64, Diag> {
    v.as_f64().ok_or_else(|| err(span, "E0351", format!("`{name}` expects a number, found `{}`", v.type_name())))
}

pub fn vec_method(recv: &Value, name: &str, args: &[Value], span: Span) -> Result<Value, Diag> {
    let (k, a, n) = vec_parts(recv).expect("vector receiver");
    let len = |c: &[f64; 4]| c[..n].iter().map(|x| x * x).sum::<f64>();
    match name {
        "length" => {
            arity(name, args, 0, span)?;
            Ok(Value::Float(len(&a).sqrt()))
        }
        "length_squared" => {
            arity(name, args, 0, span)?;
            Ok(Value::Float(len(&a)))
        }
        "normalized" => {
            arity(name, args, 0, span)?;
            let l = len(&a).sqrt();
            let mut o = [0.0; 4];
            if l > 0.0 {
                for i in 0..n {
                    o[i] = a[i] / l;
                }
            }
            Ok(vec_make(k, o))
        }
        "dot" => {
            arity(name, args, 1, span)?;
            let (_, x, y, n) = same_kind(recv, &args[0], name, span)?;
            Ok(Value::Float((0..n).map(|i| x[i] * y[i]).sum()))
        }
        "cross" => {
            arity(name, args, 1, span)?;
            match (recv, &args[0]) {
                (Value::Vec3(x), Value::Vec3(y)) => {
                    Ok(Value::Vec3([x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]]))
                }
                (Value::Vec2(x), Value::Vec2(y)) => Ok(Value::Float(x[0] * y[1] - x[1] * y[0])),
                _ => Err(err(span, "E0350", "`cross` needs two Vec3 (or two Vec2) values".into())),
            }
        }
        "distance_to" | "distance_squared_to" => {
            arity(name, args, 1, span)?;
            let (_, x, y, n) = same_kind(recv, &args[0], name, span)?;
            let d: f64 = (0..n).map(|i| (x[i] - y[i]).powi(2)).sum();
            Ok(Value::Float(if name == "distance_to" { d.sqrt() } else { d }))
        }
        "lerp" => {
            arity(name, args, 2, span)?;
            let (k, x, y, n) = same_kind(recv, &args[0], name, span)?;
            let t = scalar(&args[1], name, span)?;
            let mut o = [0.0; 4];
            for i in 0..n {
                o[i] = x[i] + (y[i] - x[i]) * t;
            }
            Ok(vec_make(k, o))
        }
        "abs" => {
            arity(name, args, 0, span)?;
            let mut o = [0.0; 4];
            for i in 0..n {
                o[i] = a[i].abs();
            }
            Ok(vec_make(k, o))
        }
        "to_array" => {
            arity(name, args, 0, span)?;
            Ok(Value::array(a[..n].iter().map(|x| Value::Float(*x)).collect()))
        }
        _ => Err(err(span, "E0352", format!("unknown {} method `{name}`", recv.type_name()))
            .with_help("try length/length_squared/normalized/dot/cross/distance_to/lerp/abs/to_array")),
    }
}

fn str_arg<'a>(v: &'a Value, name: &str, span: Span) -> Result<&'a str, Diag> {
    match v {
        Value::Str(s) => Ok(s),
        other => Err(err(span, "E0353", format!("`{name}` expects a str, found `{}`", other.type_name()))),
    }
}

fn int_arg(v: &Value, name: &str, span: Span) -> Result<i64, Diag> {
    match v {
        Value::Int(i) => Ok(*i),
        other => Err(err(span, "E0353", format!("`{name}` expects an int, found `{}`", other.type_name()))),
    }
}

pub fn str_method(s: &str, name: &str, args: &[Value], span: Span) -> Result<Value, Diag> {
    match name {
        "len" => {
            arity(name, args, 0, span)?;
            Ok(Value::Int(s.chars().count() as i64))
        }
        "upper" | "to_upper" => {
            arity(name, args, 0, span)?;
            Ok(Value::Str(s.to_uppercase()))
        }
        "lower" | "to_lower" => {
            arity(name, args, 0, span)?;
            Ok(Value::Str(s.to_lowercase()))
        }
        "strip" | "trim" => {
            arity(name, args, 0, span)?;
            Ok(Value::Str(s.trim().to_string()))
        }
        "split" => {
            if args.len() > 1 {
                return Err(err(span, "E0317", "`split` takes 0 or 1 argument(s)".into()));
            }
            let parts: Vec<Value> = match args.first() {
                None => s.split_whitespace().map(|p| Value::Str(p.to_string())).collect(),
                Some(sep) => {
                    let sep = str_arg(sep, name, span)?;
                    if sep.is_empty() {
                        s.chars().map(|c| Value::Str(c.to_string())).collect()
                    } else {
                        s.split(sep).map(|p| Value::Str(p.to_string())).collect()
                    }
                }
            };
            Ok(Value::array(parts))
        }
        "join" => {
            // `",".join(["a", "b"])`
            arity(name, args, 1, span)?;
            match &args[0] {
                Value::Array(a) => Ok(Value::Str(a.borrow().iter().map(|v| v.to_string()).collect::<Vec<_>>().join(s))),
                other => Err(err(span, "E0353", format!("`join` expects an array, found `{}`", other.type_name()))),
            }
        }
        "replace" => {
            arity(name, args, 2, span)?;
            Ok(Value::Str(s.replace(str_arg(&args[0], name, span)?, str_arg(&args[1], name, span)?)))
        }
        "find" => {
            arity(name, args, 1, span)?;
            let needle = str_arg(&args[0], name, span)?;
            Ok(Value::Int(s.find(needle).map_or(-1, |b| s[..b].chars().count() as i64)))
        }
        "contains" | "has" => {
            arity(name, args, 1, span)?;
            Ok(Value::Bool(s.contains(str_arg(&args[0], name, span)?)))
        }
        "begins_with" | "starts_with" => {
            arity(name, args, 1, span)?;
            Ok(Value::Bool(s.starts_with(str_arg(&args[0], name, span)?)))
        }
        "ends_with" => {
            arity(name, args, 1, span)?;
            Ok(Value::Bool(s.ends_with(str_arg(&args[0], name, span)?)))
        }
        "substr" | "slice" => {
            // substr(from [, len]) counts chars; slice(from, to) is exclusive. Out of range clamps.
            if args.is_empty() || args.len() > 2 {
                return Err(err(span, "E0317", format!("`{name}` takes 1 or 2 arguments")));
            }
            let total = s.chars().count() as i64;
            let from = int_arg(&args[0], name, span)?.clamp(0, total);
            let end = match args.get(1) {
                None => total,
                Some(v) if name == "substr" => (from + int_arg(v, name, span)?.max(0)).min(total),
                Some(v) => int_arg(v, name, span)?.clamp(from, total),
            };
            Ok(Value::Str(s.chars().skip(from as usize).take((end - from) as usize).collect()))
        }
        "to_int" => {
            arity(name, args, 0, span)?;
            Ok(Value::Int(s.trim().parse::<i64>().or_else(|_| s.trim().parse::<f64>().map(|f| f as i64)).unwrap_or(0)))
        }
        "to_float" => {
            arity(name, args, 0, span)?;
            Ok(Value::Float(s.trim().parse::<f64>().unwrap_or(0.0)))
        }
        _ => Err(err(span, "E0354", format!("unknown str method `{name}`"))
            .with_help("try len/split/join/strip/upper/lower/replace/find/contains/begins_with/ends_with/substr/to_int/to_float")),
    }
}

/// The `fmt("a {} b {}", x, y)` builtin: each `{}` takes the next argument; `{{` / `}}` escape.
pub fn fmt(args: &[Value], span: Span) -> Result<Value, Diag> {
    let Some(Value::Str(f)) = args.first() else {
        return Err(err(span, "E0500", "`fmt` expects a format string first".into()));
    };
    let mut rest = args[1..].iter();
    let mut out = String::new();
    let cs: Vec<char> = f.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        match (cs[i], cs.get(i + 1)) {
            ('{', Some('{')) => {
                out.push('{');
                i += 2;
            }
            ('}', Some('}')) => {
                out.push('}');
                i += 2;
            }
            ('{', Some('}')) => {
                out.push_str(&rest.next().map_or_else(String::new, |v| v.to_string()));
                i += 2;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(Value::Str(out))
}

/// Vector/color constructors: `vec2(x,y)`, `vec3(x,y,z)`, `Color(r,g,b[,a])` (also from one array).
pub fn construct(name: &str, args: &[Value], span: Span) -> Result<Value, Diag> {
    let mut nums = Vec::new();
    let flat: Vec<Value> = match args {
        [Value::Array(a)] => a.borrow().clone(),
        [v] if vec_parts(v).is_some() => {
            let (_, c, n) = vec_parts(v).unwrap();
            c[..n].iter().map(|x| Value::Float(*x)).collect()
        }
        _ => args.to_vec(),
    };
    for v in &flat {
        nums.push(scalar(v, name, span)?);
    }
    match (name, nums.len()) {
        ("vec2" | "Vec2", 2) => Ok(Value::Vec2([nums[0], nums[1]])),
        ("vec3" | "Vec3", 3) => Ok(Value::Vec3([nums[0], nums[1], nums[2]])),
        ("color" | "Color", 3) => Ok(Value::Color([nums[0], nums[1], nums[2], 1.0])),
        ("color" | "Color", 4) => Ok(Value::Color([nums[0], nums[1], nums[2], nums[3]])),
        _ => Err(err(span, "E0355", format!("`{name}` got {} number(s); expected {}", nums.len(), match name {
            "vec2" | "Vec2" => "2",
            "vec3" | "Vec3" => "3",
            _ => "3 or 4",
        }))),
    }
}
