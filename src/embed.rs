//! Embedding API: `Engine` owns loaded modules and host functions; the game calls
//! script functions by name and gets values or located diagnostics back (no panics).

use crate::diagnostics::{Diag, LineIndex};
use crate::interpreter::{Interpreter, RuspyHost};
use crate::value::Value;
use crate::vm::{self, Bytecode, Vm};
use std::any::Any;
use std::collections::HashMap;

/// Passed to every host function. `state` is whatever the caller handed to `Engine::call`
/// (e.g. `&mut World`); downcast it with `ctx.state::<World>()`.
pub struct Ctx<'a> {
    state: &'a mut dyn Any,
}
impl<'a> Ctx<'a> {
    pub fn state<T: 'static>(&mut self) -> Option<&mut T> {
        self.state.downcast_mut::<T>()
    }
}

pub type HostFn = Box<dyn Fn(&mut Ctx, &[Value]) -> Result<Value, String>>;

/// An error with a resolved source location.
#[derive(Debug, Clone)]
pub struct ScriptError {
    pub module: String,
    pub line: usize,
    pub col: usize,
    pub code: &'static str,
    pub message: String,
}
impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}: [{}] {}", self.module, self.line, self.col, self.code, self.message)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    Interpreter,
    Vm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Module(usize);

enum Exec {
    Interp(Interpreter),
    // The module owns bytecode + globals; a short-lived Vm borrows them for each call.
    Vm { bc: std::rc::Rc<Bytecode>, globals: HashMap<String, Value> },
}
struct ModuleData {
    name: String,
    lines: LineIndex,
    exec: Exec,
}

pub struct Engine {
    fns: HashMap<String, HostFn>,
    modules: Vec<ModuleData>,
    pub step_limit: u64,
    pub backend: Backend,
}

struct Bridge<'a, 'b> {
    fns: &'a HashMap<String, HostFn>,
    ctx: Ctx<'b>,
}
impl RuspyHost for Bridge<'_, '_> {
    fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        match self.fns.get(name) {
            Some(f) => f(&mut self.ctx, &args),
            None => Err(format!("function `{name}` is not defined")),
        }
    }
    fn get_member(&mut self, object: &str, member: &str) -> Result<Value, String> {
        Err(format!("`{object}` has no member `{member}`"))
    }
}

impl Engine {
    pub fn new(backend: Backend) -> Self {
        Engine { fns: HashMap::new(), modules: vec![], step_limit: 1_000_000, backend }
    }

    pub fn register(&mut self, name: &str, f: impl Fn(&mut Ctx, &[Value]) -> Result<Value, String> + 'static) {
        self.fns.insert(name.to_string(), Box::new(f));
    }

    fn err(m: &ModuleData, d: &Diag) -> ScriptError {
        let (line, col) = m.lines.locate(d.span.start);
        ScriptError { module: m.name.clone(), line, col, code: d.code, message: d.message.clone() }
    }

    /// Parse, type-check and run top-level statements (state init). `state` is available to
    /// host functions called from top level.
    pub fn load(&mut self, name: &str, src: &str, state: &mut dyn Any) -> Result<Module, Vec<ScriptError>> {
        let lines = LineIndex::new(src);
        let shell = ModuleData { name: name.into(), lines, exec: Exec::Interp(Interpreter::new()) };
        let diags = crate::check::check_str(src);
        if diags.iter().any(|d| d.is_error()) {
            return Err(diags.iter().filter(|d| d.is_error()).map(|d| Self::err(&shell, d)).collect());
        }
        let (prog, _) = crate::parser::parse_program(src);
        let mut bridge = Bridge { fns: &self.fns, ctx: Ctx { state } };
        let exec = match self.backend {
            Backend::Interpreter => {
                let mut i = Interpreter::new();
                i.set_max_steps(self.step_limit);
                i.run(&prog, &mut bridge).map_err(|d| vec![Self::err(&shell, &d)])?;
                Exec::Interp(i)
            }
            Backend::Vm => {
                let bc = vm::compile(&prog).map_err(|u| vec![ScriptError { module: name.into(), line: 0, col: 0, code: "E0900", message: format!("VM unsupported: {}", u.0) }])?;
                let mut v = Vm::new_public(&bc);
                v.set_max_steps(self.step_limit);
                v.run_public(&mut bridge).map_err(|d| vec![Self::err(&shell, &d)])?;
                let globals = v.take_globals();
                Exec::Vm { bc: std::rc::Rc::new(bc), globals }
            }
        };
        self.modules.push(ModuleData { exec, ..shell });
        Ok(Module(self.modules.len() - 1))
    }

    pub fn has_fn(&self, m: Module, f: &str) -> bool {
        match &self.modules[m.0].exec {
            Exec::Interp(i) => i.has_fn(f),
            Exec::Vm { bc, .. } => Vm::new_public(bc).has_fn(f),
        }
    }

    pub fn call(&mut self, m: Module, f: &str, args: Vec<Value>, state: &mut dyn Any) -> Result<Value, ScriptError> {
        let limit = self.step_limit;
        let md = &mut self.modules[m.0];
        let mut bridge = Bridge { fns: &self.fns, ctx: Ctx { state } };
        let r = match &mut md.exec {
            Exec::Interp(i) => {
                i.set_max_steps(limit);
                i.call_named(f, args, &mut bridge)
            }
            Exec::Vm { bc, globals } => {
                let mut v = Vm::new_public(bc);
                v.set_max_steps(limit);
                v.set_globals(std::mem::take(globals));
                let r = v.call_named(f, args, &mut bridge);
                *globals = v.take_globals();
                r
            }
        };
        r.map_err(|d| Self::err(md, &d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    type World = HashMap<String, Value>;

    fn engine(b: Backend) -> Engine {
        let mut e = Engine::new(b);
        e.register("get", |c, a| {
            let w = c.state::<World>().ok_or("no world")?;
            let Value::Str(k) = &a[0] else { return Err("get: path must be str".into()) };
            Ok(w.get(k).cloned().unwrap_or(Value::Int(0)))
        });
        e.register("set", |c, a| {
            let w = c.state::<World>().ok_or("no world")?;
            let Value::Str(k) = &a[0] else { return Err("set: path must be str".into()) };
            w.insert(k.clone(), a[1].clone());
            Ok(Value::Void)
        });
        e.register("tostr", |_c, a| Ok(Value::Str(a[0].to_string())));
        e.register("log", |_c, a| {
            let _ = a; // no-op in bench
            Ok(Value::Void)
        });
        e
    }

    const SCRIPT: &str = r#"
fn on_overlap(other) {
    score = get("score") + 1;
    set("score", score);
    log("score " + tostr(score));
}
"#;

    fn bench(b: Backend) {
        let mut e = engine(b);
        let mut w: World = HashMap::new();
        let m = e.load("t.ruspy", SCRIPT, &mut w).unwrap_or_else(|er| panic!("{er:?}"));
        let t = Instant::now();
        for _ in 0..1000 {
            e.call(m, "on_overlap", vec![Value::Int(7)], &mut w).unwrap_or_else(|er| panic!("{er}"));
        }
        let us = t.elapsed().as_secs_f64() * 1e6 / 1000.0;
        println!("BENCH {b:?}: {us:.2} us/call, score={:?}", w["score"]);
        assert_eq!(w["score"], Value::Int(1000));
    }

    #[test]
    fn bench_interp() { bench(Backend::Interpreter) }
    #[test]
    fn bench_vm() { bench(Backend::Vm) }

    #[test]
    fn infinite_loop_stops() {
        for b in [Backend::Interpreter, Backend::Vm] {
            let mut e = engine(b);
            let mut w = World::new();
            let m = e.load("l.ruspy", "fn spin() { while true { } }", &mut w).unwrap();
            let err = e.call(m, "spin", vec![], &mut w).unwrap_err();
            println!("{b:?}: {err}");
            assert_eq!(err.code, "E0307");
        }
    }

    #[test]
    fn syntax_error_has_line() {
        let mut e = engine(Backend::Interpreter);
        let mut w = World::new();
        let errs = e.load("bad.ruspy", "fn f() {\n  x = ;\n}\n", &mut w).unwrap_err();
        println!("{}", errs[0]);
        assert_eq!(errs[0].line, 2);
    }

    #[test]
    fn host_error_is_value() {
        let mut e = engine(Backend::Interpreter);
        let mut w = World::new();
        let m = e.load("h.ruspy", "fn f() { nope(); }", &mut w);
        println!("load: {:?}", m.as_ref().err());
        if let Ok(m) = m { println!("{}", e.call(m, "f", vec![], &mut w).unwrap_err()); }
    }

    fn run_ret(src: &str, call: &str) -> Value {
        for b in [Backend::Interpreter, Backend::Vm] {
            let mut e = Engine::new(b);
            let mut w = World::new();
            let m = e.load("t.ruspy", src, &mut w).unwrap_or_else(|er| panic!("{er:?}"));
            let v = e.call(m, call, vec![], &mut w).unwrap_or_else(|er| panic!("{er}"));
            if b == Backend::Vm {
                return v;
            }
        }
        unreachable!()
    }

    #[test]
    fn deep_nesting_is_a_diagnostic_not_a_crash() {
        let mut e = Engine::new(Backend::Interpreter);
        let mut w = World::new();
        for src in [
            format!("fn f() {{ x = {}1{}; }}", "(".repeat(1000), ")".repeat(1000)),
            format!("fn f() {{ x = {}1; }}", "-".repeat(5000)),
            format!("fn f() {{ {} {} }}", "if true {".repeat(1000), "}".repeat(1000)),
            format!("fn f() {{ x = 1{}; }}", "+1".repeat(5000)),
        ] {
            let errs = e.load("deep.ruspy", &src, &mut w).unwrap_err();
            assert!(errs.iter().any(|x| x.code == "E0109"), "{:?}", errs.first());
        }
        // Moderate nesting still works.
        let ok = format!("fn f() {{ return {}1{}; }}", "(".repeat(30), ")".repeat(30));
        assert!(e.load("ok.ruspy", &ok, &mut w).is_ok());
    }

    #[test]
    fn tostr_and_string_concat() {
        let src = r#"
fn a() { return "n=" + 3; }
fn b() { return 2.5 + "x"; }
fn c() { return "ok " + true; }
fn d() { return "v" + tostr(7) + tostr(1.5); }
"#;
        assert_eq!(run_ret(src, "a"), Value::Str("n=3".into()));
        assert_eq!(run_ret(src, "b"), Value::Str("2.5x".into()));
        assert_eq!(run_ret(src, "c"), Value::Str("ok true".into()));
        assert_eq!(run_ret(src, "d"), Value::Str("v71.5".into()));
    }

    #[test]
    fn null_and_is_null() {
        let src = r#"
fn n() { return null; }
fn t() { x = null; return is_null(x); }
fn f() { return is_null(3); }
fn eq() { x = null; return x == null; }
fn ne() { return 1 == null; }
fn show() { return "got " + tostr(null); }
"#;
        assert_eq!(run_ret(src, "n"), Value::Null);
        assert_eq!(run_ret(src, "t"), Value::Bool(true));
        assert_eq!(run_ret(src, "f"), Value::Bool(false));
        assert_eq!(run_ret(src, "eq"), Value::Bool(true));
        assert_eq!(run_ret(src, "ne"), Value::Bool(false));
        assert_eq!(run_ret(src, "show"), Value::Str("got null".into()));
    }
}
