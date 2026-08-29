//! Built-in **pure** packages — namespaced standard-library functions a strategy
//! pulls in with `import <name>;`. Pure means no host / market state, so they
//! resolve in any host (including `run_str`'s default). Today: `math`.
//!
//! Host-backed packages (`account`, `trade`, `series`) live in the strategy host
//! (`ruspy_host`) and are reached via `RuspyHost::call_method` / `get_member`
//! after the functions here return `None`.
//!
//! Resolution order for `pkg.member` / `pkg.method(args)` on an unbound identifier:
//! **pure packages here → host packages**. That mirrors the call path (user fn →
//! stdlib → host) and keeps math available with no wiring.

use crate::diagnostics::{Diag, Span};
use crate::value::Value;

/// Package names that resolve purely here (no host). The checker uses this to know
/// `math` is a real package name (so `import math;` is meaningful and enforceable).
pub fn is_pure_package(name: &str) -> bool {
    matches!(name, "math")
}

/// A constant read `pkg.FIELD`. `None` when `pkg` isn't a pure package (fall through
/// to the host object, e.g. `tick.bid`).
pub fn get_member(pkg: &str, field: &str, span: Span) -> Option<Result<Value, Diag>> {
    if pkg != "math" {
        return None;
    }
    Some(match field {
        "PI" => Ok(Value::Float(std::f64::consts::PI)),
        "E" => Ok(Value::Float(std::f64::consts::E)),
        "TAU" => Ok(Value::Float(std::f64::consts::TAU)),
        "SQRT2" => Ok(Value::Float(std::f64::consts::SQRT_2)),
        "LN2" => Ok(Value::Float(std::f64::consts::LN_2)),
        "INF" => Ok(Value::Float(f64::INFINITY)),
        "NAN" => Ok(Value::Float(f64::NAN)),
        _ => Err(err(span, format!("`math` has no constant `{field}` (try PI, E, TAU, SQRT2, LN2, INF, NAN)"))),
    })
}

/// A method call `pkg.method(args)`. `None` when `pkg` isn't a pure package.
pub fn call_method(pkg: &str, method: &str, args: &[Value], span: Span) -> Option<Result<Value, Diag>> {
    if pkg != "math" {
        return None;
    }
    Some(math_call(method, args, span))
}

fn math_call(name: &str, args: &[Value], span: Span) -> Result<Value, Diag> {
    match name {
        // trig
        "sin" => f1(name, args, span, f64::sin),
        "cos" => f1(name, args, span, f64::cos),
        "tan" => f1(name, args, span, f64::tan),
        "asin" => f1(name, args, span, f64::asin),
        "acos" => f1(name, args, span, f64::acos),
        "atan" => f1(name, args, span, f64::atan),
        "sinh" => f1(name, args, span, f64::sinh),
        "cosh" => f1(name, args, span, f64::cosh),
        "tanh" => f1(name, args, span, f64::tanh),
        // exp / log
        "exp" => f1(name, args, span, f64::exp),
        "ln" => f1(name, args, span, f64::ln),
        "log10" => f1(name, args, span, f64::log10),
        "log2" => f1(name, args, span, f64::log2),
        "log" => f2(name, args, span, |x, b| x.log(b)),
        // powers / roots
        "sqrt" => f1(name, args, span, f64::sqrt),
        "cbrt" => f1(name, args, span, f64::cbrt),
        "pow" => f2(name, args, span, f64::powf),
        "hypot" => f2(name, args, span, f64::hypot),
        // rounding / shape
        "abs" => f1(name, args, span, f64::abs),
        "floor" => f1(name, args, span, f64::floor),
        "ceil" => f1(name, args, span, f64::ceil),
        "round" => f1(name, args, span, f64::round),
        "trunc" => f1(name, args, span, f64::trunc),
        "fract" => f1(name, args, span, f64::fract),
        // angles
        "to_radians" => f1(name, args, span, f64::to_radians),
        "to_degrees" => f1(name, args, span, f64::to_degrees),
        "atan2" => f2(name, args, span, f64::atan2),
        "fmod" => f2(name, args, span, |a, b| a % b),
        // reductions
        "min" => f2(name, args, span, f64::min),
        "max" => f2(name, args, span, f64::max),
        "sign" => {
            arity(name, args, 1, span)?;
            let x = num(&args[0], name, span)?;
            Ok(Value::Float(if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 }))
        }
        "clamp" => {
            arity(name, args, 3, span)?;
            let x = num(&args[0], name, span)?;
            let a = num(&args[1], name, span)?;
            let b = num(&args[2], name, span)?;
            Ok(Value::Float(x.clamp(a.min(b), a.max(b))))
        }
        "is_nan" => {
            arity(name, args, 1, span)?;
            Ok(Value::Bool(num(&args[0], name, span)?.is_nan()))
        }
        "is_finite" => {
            arity(name, args, 1, span)?;
            Ok(Value::Bool(num(&args[0], name, span)?.is_finite()))
        }
        _ => Err(err(span, format!("`math` has no function `{name}`"))),
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────
fn err(span: Span, msg: String) -> Diag {
    Diag::error(span, "E0510", msg)
}

fn arity(name: &str, args: &[Value], want: usize, span: Span) -> Result<(), Diag> {
    if args.len() == want {
        Ok(())
    } else {
        Err(err(span, format!("`math.{name}` takes {want} argument(s), got {}", args.len())))
    }
}

fn num(v: &Value, name: &str, span: Span) -> Result<f64, Diag> {
    v.as_f64().ok_or_else(|| err(span, format!("`math.{name}` expects a number, found `{}`", v.type_name())))
}

fn f1(name: &str, args: &[Value], span: Span, f: impl Fn(f64) -> f64) -> Result<Value, Diag> {
    arity(name, args, 1, span)?;
    Ok(Value::Float(f(num(&args[0], name, span)?)))
}

fn f2(name: &str, args: &[Value], span: Span, f: impl Fn(f64, f64) -> f64) -> Result<Value, Diag> {
    arity(name, args, 2, span)?;
    Ok(Value::Float(f(num(&args[0], name, span)?, num(&args[1], name, span)?)))
}

#[cfg(test)]
mod tests {
    use crate::interpreter::run_str;

    fn out(src: &str) -> Vec<String> {
        run_str(src).unwrap_or_else(|d| panic!("{d:?}"))
    }

    #[test]
    fn math_constants_and_functions() {
        assert_eq!(out("import math; print math.pow(2.0, 10.0);"), vec!["1024"]);
        assert_eq!(out("import math; print math.max(3.0, 7.0); print math.min(3.0, 7.0);"), vec!["7", "3"]);
        assert_eq!(out("import math; print math.sign(-4.0); print math.sign(0.0);"), vec!["-1", "0"]);
        // sin(PI) ≈ 0
        let o = out("import math; r = math.sin(math.PI); if math.abs(r) < 0.0001 { print \"zero\"; }");
        assert_eq!(o, vec!["zero"]);
        // clamp orders its bounds
        assert_eq!(out("import math; print math.clamp(15.0, 10.0, 0.0);"), vec!["10"]);
    }

    #[test]
    fn math_misuse_is_a_diagnostic_not_a_panic() {
        assert!(run_str("import math; print math.sqrt(\"x\");").is_err());
        assert!(run_str("import math; print math.pow(2.0);").is_err()); // arity
        assert!(run_str("import math; print math.frobnicate(1.0);").is_err()); // unknown fn
        assert!(run_str("import math; print math.NOPE;").is_err()); // unknown constant
    }
}
