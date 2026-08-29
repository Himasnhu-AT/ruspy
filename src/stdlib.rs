//! The ruspy standard library — builtin functions callable from any strategy.
//!
//! Two families: **math** scalars (`abs`, `sqrt`, `pow`, `floor`, `ceil`, `round`,
//! `min`, `max`, `ln`, `exp`) and **indicators** over a price array returning the
//! latest value (`sma`, `ema`, `wma`, `rsi`, `stdev`, `mean`, `sum`, `highest`,
//! `lowest`). Resolved in the call path before the host, so a strategy can use them
//! with no host wiring. NaN warmup discipline: an indicator with too little data
//! returns `nan` (checkable with `is_nan`).

use crate::diagnostics::{Diag, Span};
use crate::value::Value;

/// Is `name` a stdlib builtin? (Used by the checker to treat these calls as known.)
pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "abs" | "sqrt" | "pow" | "floor" | "ceil" | "round" | "min" | "max" | "ln" | "exp"
            | "is_nan" | "is_finite" | "nan" | "sma" | "ema" | "wma" | "rsi" | "stdev" | "mean"
            | "sum" | "highest" | "lowest" | "len"
    )
}

/// Call a builtin. Returns `None` if `name` is not a builtin (fall through to the
/// user's functions / the host). Never panics — misuse is a `Diag`.
pub fn call(name: &str, args: &[Value], span: Span) -> Option<Result<Value, Diag>> {
    if !is_builtin(name) {
        return None;
    }
    Some(dispatch(name, args, span))
}

fn dispatch(name: &str, args: &[Value], span: Span) -> Result<Value, Diag> {
    match name {
        // ── scalar math ──────────────────────────────────────────────────────
        "abs" => Ok(fnum(name, args, span, |x| x.abs())?),
        "sqrt" => Ok(fnum(name, args, span, |x| x.sqrt())?),
        "floor" => Ok(fnum(name, args, span, |x| x.floor())?),
        "ceil" => Ok(fnum(name, args, span, |x| x.ceil())?),
        "round" => Ok(fnum(name, args, span, |x| x.round())?),
        "ln" => Ok(fnum(name, args, span, |x| x.ln())?),
        "exp" => Ok(fnum(name, args, span, |x| x.exp())?),
        "nan" => {
            arity(name, args, 0, span)?;
            Ok(Value::Float(f64::NAN))
        }
        "is_nan" => {
            arity(name, args, 1, span)?;
            Ok(Value::Bool(num(&args[0], name, span)?.is_nan()))
        }
        "is_finite" => {
            arity(name, args, 1, span)?;
            Ok(Value::Bool(num(&args[0], name, span)?.is_finite()))
        }
        "pow" => {
            arity(name, args, 2, span)?;
            Ok(Value::Float(num(&args[0], name, span)?.powf(num(&args[1], name, span)?)))
        }
        "min" => {
            arity(name, args, 2, span)?;
            Ok(pick(&args[0], &args[1], name, span, true)?)
        }
        "max" => {
            arity(name, args, 2, span)?;
            Ok(pick(&args[0], &args[1], name, span, false)?)
        }
        "len" => {
            arity(name, args, 1, span)?;
            match &args[0] {
                Value::Array(a) => Ok(Value::Int(a.borrow().len() as i64)),
                Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
                other => Err(err(span, format!("`len` expects an array or str, found `{}`", other.type_name()))),
            }
        }

        // ── series / indicators ─────────────────────────────────────────────
        "sum" => {
            arity(name, args, 1, span)?;
            let s = series(&args[0], name, span)?;
            Ok(Value::Float(s.iter().sum()))
        }
        "mean" => {
            arity(name, args, 1, span)?;
            let s = series(&args[0], name, span)?;
            if s.is_empty() {
                return Ok(Value::Float(f64::NAN));
            }
            Ok(Value::Float(s.iter().sum::<f64>() / s.len() as f64))
        }
        "stdev" => {
            arity(name, args, 1, span)?;
            let s = series(&args[0], name, span)?;
            Ok(Value::Float(stdev(&s)))
        }
        "highest" | "lowest" => {
            arity(name, args, 2, span)?;
            let s = series(&args[0], name, span)?;
            let n = int(&args[1], name, span)?.max(0) as usize;
            let tail = tail(&s, n);
            if tail.is_empty() {
                return Ok(Value::Float(f64::NAN));
            }
            let v = if name == "highest" {
                tail.iter().cloned().fold(f64::MIN, f64::max)
            } else {
                tail.iter().cloned().fold(f64::MAX, f64::min)
            };
            Ok(Value::Float(v))
        }
        "sma" => {
            arity(name, args, 2, span)?;
            let s = series(&args[0], name, span)?;
            let n = int(&args[1], name, span)?.max(1) as usize;
            let t = tail(&s, n);
            if t.len() < n {
                return Ok(Value::Float(f64::NAN));
            }
            Ok(Value::Float(t.iter().sum::<f64>() / n as f64))
        }
        "wma" => {
            arity(name, args, 2, span)?;
            let s = series(&args[0], name, span)?;
            let n = int(&args[1], name, span)?.max(1) as usize;
            let t = tail(&s, n);
            if t.len() < n {
                return Ok(Value::Float(f64::NAN));
            }
            let (mut num, mut den) = (0.0, 0.0);
            for (i, v) in t.iter().enumerate() {
                let w = (i + 1) as f64;
                num += v * w;
                den += w;
            }
            Ok(Value::Float(num / den))
        }
        "ema" => {
            arity(name, args, 2, span)?;
            let s = series(&args[0], name, span)?;
            let n = int(&args[1], name, span)?.max(1) as usize;
            if s.len() < n {
                return Ok(Value::Float(f64::NAN));
            }
            let k = 2.0 / (n as f64 + 1.0);
            // seed with the SMA of the first `n`, then walk forward (matches qq_chart)
            let mut e = s[..n].iter().sum::<f64>() / n as f64;
            for v in &s[n..] {
                e = v * k + e * (1.0 - k);
            }
            Ok(Value::Float(e))
        }
        "rsi" => {
            arity(name, args, 2, span)?;
            let s = series(&args[0], name, span)?;
            let n = int(&args[1], name, span)?.max(1) as usize;
            if s.len() <= n {
                return Ok(Value::Float(f64::NAN));
            }
            // Wilder's RSI.
            let (mut gain, mut loss) = (0.0, 0.0);
            for w in s.windows(2).take(n) {
                let d = w[1] - w[0];
                if d >= 0.0 {
                    gain += d;
                } else {
                    loss -= d;
                }
            }
            let (mut ag, mut al) = (gain / n as f64, loss / n as f64);
            for w in s.windows(2).skip(n) {
                let d = w[1] - w[0];
                let (g, l) = if d >= 0.0 { (d, 0.0) } else { (0.0, -d) };
                ag = (ag * (n as f64 - 1.0) + g) / n as f64;
                al = (al * (n as f64 - 1.0) + l) / n as f64;
            }
            let rsi = if al == 0.0 { 100.0 } else { 100.0 - 100.0 / (1.0 + ag / al) };
            Ok(Value::Float(rsi))
        }
        _ => Err(err(span, format!("`{name}` is not a builtin"))),
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────
fn err(span: Span, msg: String) -> Diag {
    Diag::error(span, "E0500", msg)
}

fn arity(name: &str, args: &[Value], want: usize, span: Span) -> Result<(), Diag> {
    if args.len() == want {
        Ok(())
    } else {
        Err(err(span, format!("`{name}` takes {want} argument(s), got {}", args.len())))
    }
}

fn num(v: &Value, name: &str, span: Span) -> Result<f64, Diag> {
    match v {
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) => Ok(*f),
        other => Err(err(span, format!("`{name}` expects a number, found `{}`", other.type_name()))),
    }
}

fn int(v: &Value, name: &str, span: Span) -> Result<i64, Diag> {
    match v {
        Value::Int(i) => Ok(*i),
        other => Err(err(span, format!("`{name}` expects an int, found `{}`", other.type_name()))),
    }
}

/// A 1-arg math builtin: preserve int→int for `abs`, else float.
fn fnum(name: &str, args: &[Value], span: Span, f: impl Fn(f64) -> f64) -> Result<Value, Diag> {
    arity(name, args, 1, span)?;
    if name == "abs" {
        if let Value::Int(i) = &args[0] {
            return Ok(Value::Int(i.wrapping_abs()));
        }
    }
    Ok(Value::Float(f(num(&args[0], name, span)?)))
}

fn pick(a: &Value, b: &Value, name: &str, span: Span, want_min: bool) -> Result<Value, Diag> {
    // Preserve int when both are ints.
    if let (Value::Int(x), Value::Int(y)) = (a, b) {
        return Ok(Value::Int(if (x < y) == want_min { *x } else { *y }));
    }
    let (x, y) = (num(a, name, span)?, num(b, name, span)?);
    Ok(Value::Float(if (x < y) == want_min { x } else { y }))
}

fn series(v: &Value, name: &str, span: Span) -> Result<Vec<f64>, Diag> {
    match v {
        Value::Array(a) => {
            let mut out = Vec::with_capacity(a.borrow().len());
            for e in a.borrow().iter() {
                out.push(num(e, name, span)?);
            }
            Ok(out)
        }
        other => Err(err(span, format!("`{name}` expects an array, found `{}`", other.type_name()))),
    }
}

fn tail(s: &[f64], n: usize) -> &[f64] {
    if n >= s.len() {
        s
    } else {
        &s[s.len() - n..]
    }
}

fn stdev(s: &[f64]) -> f64 {
    if s.len() < 2 {
        return f64::NAN;
    }
    let mean = s.iter().sum::<f64>() / s.len() as f64;
    let var = s.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / s.len() as f64;
    var.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::run_str;

    fn out(src: &str) -> Vec<String> {
        run_str(src).unwrap_or_else(|d| panic!("{d:?}"))
    }

    #[test]
    fn math_builtins() {
        assert_eq!(out("print abs(-5); print sqrt(9.0); print pow(2.0, 10.0); print max(3, 7); print min(3, 7);"),
                   vec!["5", "3", "1024", "7", "3"]);
        assert_eq!(out("print floor(3.7); print ceil(3.2); print round(3.5);"), vec!["3", "4", "4"]);
    }

    #[test]
    fn indicators_over_a_series() {
        // SMA of the last 3 of [10..15] = (13+14+15)/3 = 14
        assert_eq!(out("p = [10.0, 11.0, 12.0, 13.0, 14.0, 15.0]; print sma(p, 3);"), vec!["14"]);
        assert_eq!(out("p = [1.0, 2.0, 3.0, 4.0]; print mean(p); print sum(p);"), vec!["2.5", "10"]);
        assert_eq!(out("p = [3.0, 1.0, 4.0, 1.0, 5.0]; print highest(p, 3); print lowest(p, 3);"), vec!["5", "1"]);
    }

    #[test]
    fn ema_and_rsi_run_and_warm_up() {
        // EMA is defined once enough data exists; a short series is NaN.
        assert_eq!(out("p = [1.0, 2.0]; print is_nan(sma(p, 5));"), vec!["true"]);
        // A trending-up series has RSI near 100.
        let o = out("p = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]; r = rsi(p, 3); if r > 90.0 { print \"strong\"; }");
        assert_eq!(o, vec!["strong"]);
    }

    #[test]
    fn builtins_are_diagnostics_on_misuse() {
        assert!(run_str("print sqrt(\"x\");").is_err());
        assert!(run_str("print sma(5, 3);").is_err()); // not an array
        assert!(run_str("print pow(1.0);").is_err()); // arity
    }

    #[test]
    fn a_user_function_shadows_a_builtin() {
        // user-defined `sqrt` wins over the builtin (call path checks user funcs first)
        assert_eq!(out("fn sqrt(x) { return x + 1; } print sqrt(9);"), vec!["10"]);
    }
}
