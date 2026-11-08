//! Embedding API: `Engine` owns loaded modules and host functions; the game calls
//! script functions by name and gets values or located diagnostics back (no panics).

use crate::diagnostics::{Diag, LineIndex};
use crate::interpreter::{Interpreter, RuspyHost};
pub use crate::interpreter::coroutine::Outcome;
use crate::value::Value;
use crate::vm::{self, Bytecode, Vm};
use std::any::Any;
use std::collections::HashMap;
use std::rc::Rc;

/// Passed to every host function. `state` is whatever the caller handed to `Engine::call`
/// (e.g. `&mut World`); downcast it with `ctx.state::<World>()`.
pub struct Ctx<'a> {
    state: &'a mut dyn Any,
    fns: Option<&'a HashMap<String, HostFn>>,
}
impl<'a> Ctx<'a> {
    /// Is a host function registered under `name`? (For object hooks that map `h.name(..)` to a
    /// host function.)
    pub fn has_host(&self, name: &str) -> bool {
        self.fns.is_some_and(|f| f.contains_key(name))
    }
    /// Calls the host function `name` (what a script's `name(args)` would call).
    pub fn call_host(&mut self, name: &str, args: &[Value]) -> Result<Value, String> {
        let fns = self.fns;
        match fns.and_then(|f| f.get(name)) {
            Some(f) => f(self, args),
            None => Err(format!("function `{name}` is not defined")),
        }
    }
    pub fn state<T: 'static>(&mut self) -> Option<&mut T> {
        self.state.downcast_mut::<T>()
    }
}

pub type HostFn = Box<dyn Fn(&mut Ctx, &[Value]) -> Result<Value, String>>;

pub type ObjGetFn = Box<dyn Fn(&mut Ctx, u64, &str) -> Result<Value, String>>;
pub type ObjSetFn = Box<dyn Fn(&mut Ctx, u64, &str, Value) -> Result<(), String>>;
pub type ObjCallFn = Box<dyn Fn(&mut Ctx, u64, &str, &[Value]) -> Result<Value, String>>;

/// Host hooks for object handles (`Value::Object`): what `h.field`, `h.field = v` and
/// `h.method(args)` do. Shared by forked engines.
#[derive(Default)]
struct ObjHooks {
    get: Option<ObjGetFn>,
    set: Option<ObjSetFn>,
    call: Option<ObjCallFn>,
}

/// An error with a resolved source location.
#[derive(Debug, Clone)]
pub struct ScriptError {
    pub module: String,
    pub line: usize,
    pub col: usize,
    pub code: &'static str,
    pub message: String,
    /// Script call stack at the failure, innermost first: `(function, line, col)`.
    pub trace: Vec<(String, usize, usize)>,
}
impl ScriptError {
    /// `  at fn (module:line:col)` per frame, innermost first (empty without a stack).
    pub fn trace_text(&self) -> String {
        self.trace.iter().map(|(f, l, c)| format!("  at {f} ({}:{l}:{c})", self.module)).collect::<Vec<_>>().join("\n")
    }
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
    lines: Rc<LineIndex>,
    exec: Exec,
    warnings: Vec<ScriptError>,
}

pub struct Engine {
    fns: Rc<HashMap<String, HostFn>>,
    hooks: Rc<ObjHooks>,
    modules: Vec<ModuleData>,
    pub step_limit: u64,
    pub backend: Backend,
    self_id: Option<u64>,
}

struct Bridge<'a, 'b> {
    fns: &'a HashMap<String, HostFn>,
    hooks: &'a ObjHooks,
    self_id: Option<u64>,
    ctx: Ctx<'b>,
}
impl RuspyHost for Bridge<'_, '_> {
    fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String> {
        match self.fns.get(name) {
            Some(f) => f(&mut self.ctx, &args),
            None => Err(format!("function `{name}` is not defined")),
        }
    }
    fn object_get(&mut self, id: u64, name: &str) -> Result<Value, String> {
        match &self.hooks.get {
            Some(f) => f(&mut self.ctx, id, name),
            None => Err(format!("object Node#{id} has no property `{name}`")),
        }
    }
    fn object_set(&mut self, id: u64, name: &str, value: Value) -> Result<(), String> {
        match &self.hooks.set {
            Some(f) => f(&mut self.ctx, id, name, value),
            None => Err(format!("cannot set `{name}` on object Node#{id}")),
        }
    }
    fn object_call(&mut self, id: u64, name: &str, args: Vec<Value>) -> Result<Value, String> {
        match &self.hooks.call {
            Some(f) => f(&mut self.ctx, id, name, &args),
            None => Err(format!("object Node#{id} has no method `{name}`")),
        }
    }
    fn is_self(&self, id: u64) -> bool {
        self.self_id == Some(id)
    }
    fn resolve_name(&mut self, name: &str) -> Option<Value> {
        if name == "self" {
            return self.self_id.map(Value::Object);
        }
        None
    }
    fn get_member(&mut self, object: &str, member: &str) -> Result<Value, String> {
        match self.fns.get(&format!("{object}.{member}")) {
            Some(f) => f(&mut self.ctx, &[]),
            None => Err(format!("`{object}` has no member `{member}`")),
        }
    }
    /// `obj.method(args)` on a bare name calls the host function registered as `"obj.method"`.
    fn call_method(&mut self, object: &str, method: &str, args: Vec<Value>) -> Result<Value, String> {
        match self.fns.get(&format!("{object}.{method}")) {
            Some(f) => f(&mut self.ctx, &args),
            None => Err(format!("no method `{object}.{method}`")),
        }
    }
}

impl Engine {
    pub fn new(backend: Backend) -> Self {
        Engine { fns: Rc::new(HashMap::new()), hooks: Rc::new(ObjHooks::default()), modules: vec![], step_limit: 1_000_000, backend, self_id: None }
    }

    /// A new engine with no modules that shares this one's host functions (cheap: one `Rc`
    /// clone). Lets a host give every script instance its own engine, so instances can call
    /// each other re-entrantly, without registering the functions again.
    pub fn fork(&self) -> Engine {
        Engine { fns: self.fns.clone(), hooks: self.hooks.clone(), modules: vec![], step_limit: self.step_limit, backend: self.backend, self_id: None }
    }

    /// Is a host function registered under `name`?
    pub fn has_host(&self, name: &str) -> bool {
        self.fns.contains_key(name)
    }

    /// Calls host function `name` from the host side (no script running); `state` reaches the
    /// function through its `Ctx`.
    pub fn call_host(&self, name: &str, args: &[Value], state: &mut dyn Any) -> Result<Value, String> {
        let mut ctx = Ctx { state, fns: Some(&self.fns) };
        ctx.call_host(name, args)
    }

    /// The object handle this engine's scripts run on. `self` in a script resolves to it, and
    /// `self.x` / `self.f()` read the script's own variables / functions first (so a running
    /// instance never needs the host to reach back into itself). Set it before `load`.
    pub fn set_self_handle(&mut self, id: Option<u64>) {
        self.self_id = id;
    }

    /// `h.name` on a `Value::Object` handle.
    pub fn register_object_get(&mut self, f: impl Fn(&mut Ctx, u64, &str) -> Result<Value, String> + 'static) {
        Rc::get_mut(&mut self.hooks).expect("register object hooks before forking the engine").get = Some(Box::new(f));
    }
    /// `h.name = v` (and compound `h.name += v`, which reads via the get hook first).
    pub fn register_object_set(&mut self, f: impl Fn(&mut Ctx, u64, &str, Value) -> Result<(), String> + 'static) {
        Rc::get_mut(&mut self.hooks).expect("register object hooks before forking the engine").set = Some(Box::new(f));
    }
    /// `h.name(args)` on a `Value::Object` handle.
    pub fn register_object_call(&mut self, f: impl Fn(&mut Ctx, u64, &str, &[Value]) -> Result<Value, String> + 'static) {
        Rc::get_mut(&mut self.hooks).expect("register object hooks before forking the engine").call = Some(Box::new(f));
    }

    pub fn register(&mut self, name: &str, f: impl Fn(&mut Ctx, &[Value]) -> Result<Value, String> + 'static) {
        Rc::get_mut(&mut self.fns)
            .expect("register host functions before forking the engine")
            .insert(name.to_string(), Box::new(f));
    }

    fn err(m: &ModuleData, d: &Diag) -> ScriptError {
        let (line, col) = m.lines.locate(d.span.start);
        let trace = d.trace.iter().map(|t| { let (l, c) = m.lines.locate(t.span.start); (t.func.clone(), l, c) }).collect();
        ScriptError { module: m.name.clone(), line, col, code: d.code, message: d.message.clone(), trace }
    }

    /// Parse, type-check and run top-level statements (state init). `state` is available to
    /// host functions called from top level.
    pub fn load(&mut self, name: &str, src: &str, state: &mut dyn Any) -> Result<Module, Vec<ScriptError>> {
        let lines = Rc::new(LineIndex::new(src));
        let shell = ModuleData { name: name.into(), lines, exec: Exec::Interp(Interpreter::new()), warnings: vec![] };
        let diags = crate::check::check_str(src);
        if diags.iter().any(|d| d.is_error()) {
            return Err(diags.iter().filter(|d| d.is_error()).map(|d| Self::err(&shell, d)).collect());
        }
        let warnings: Vec<ScriptError> = diags.iter().filter(|d| !d.is_error()).map(|d| Self::err(&shell, d)).collect();
        let (prog, _) = crate::parser::parse_program(src);
        let mut bridge = Bridge { fns: &self.fns, hooks: &self.hooks, self_id: self.self_id, ctx: Ctx { state, fns: Some(&self.fns) } };
        let exec = match self.backend {
            Backend::Interpreter => {
                let mut i = Interpreter::new();
                i.set_source(name, shell.lines.clone());
                i.set_max_steps(self.step_limit);
                i.run(&prog, &mut bridge).map_err(|d| vec![Self::err(&shell, &d)])?;
                Exec::Interp(i)
            }
            Backend::Vm => {
                let bc = vm::compile(&prog).map_err(|u| vec![ScriptError { module: name.into(), line: 0, col: 0, code: "E0900", message: format!("VM unsupported: {}", u.0), trace: vec![] }])?;
                let mut v = Vm::new_public(&bc);
                v.set_source(name, shell.lines.clone());
                v.set_max_steps(self.step_limit);
                v.run_public(&mut bridge).map_err(|d| vec![Self::err(&shell, &d)])?;
                let globals = v.take_globals();
                Exec::Vm { bc: std::rc::Rc::new(bc), globals }
            }
        };
        self.modules.push(ModuleData { exec, warnings, ..shell });
        Ok(Module(self.modules.len() - 1))
    }

    /// A module-level variable's current value (`None` if the module never set it).
    pub fn global(&self, m: Module, name: &str) -> Option<Value> {
        match &self.modules[m.0].exec {
            Exec::Interp(i) => i.global(name).cloned(),
            Exec::Vm { globals, .. } => globals.get(name).cloned(),
        }
    }

    /// Sets a module-level variable (creates it if missing). Functions see the new value on
    /// their next call.
    pub fn set_global(&mut self, m: Module, name: &str, v: Value) {
        match &mut self.modules[m.0].exec {
            Exec::Interp(i) => i.set_var(name, v),
            Exec::Vm { globals, .. } => {
                globals.insert(name.to_string(), v);
            }
        }
    }

    /// Names of the module-level variables, sorted.
    pub fn global_names(&self, m: Module) -> Vec<String> {
        let mut v: Vec<String> = match &self.modules[m.0].exec {
            Exec::Interp(i) => i.global_names(),
            Exec::Vm { globals, .. } => globals.keys().cloned().collect(),
        };
        v.sort();
        v
    }

    pub fn has_fn(&self, m: Module, f: &str) -> bool {
        match &self.modules[m.0].exec {
            Exec::Interp(i) => i.has_fn(f),
            Exec::Vm { bc, .. } => Vm::new_public(bc).has_fn(f),
        }
    }

    /// Checker warnings for module `m` (e.g. `W0101` integer division), located. Empty when clean.
    pub fn warnings(&self, m: Module) -> &[ScriptError] {
        &self.modules[m.0].warnings
    }

    /// Does calling `f` start a coroutine (its body has `await`)? Interpreter modules only.
    pub fn is_coroutine(&self, m: Module, f: &str) -> bool {
        match &self.modules[m.0].exec {
            Exec::Interp(i) => i.is_coroutine(f),
            Exec::Vm { .. } => false,
        }
    }

    /// Coroutines that suspended during `load`/`call`/`resume` since the last time: each
    /// `(token, awaited value)`. A call that starts a coroutine returns `void` once it reaches
    /// its first `await`; the host decides when to [`Engine::resume`] it.
    pub fn take_suspended(&mut self, m: Module) -> Vec<(u64, Value)> {
        match &mut self.modules[m.0].exec {
            Exec::Interp(i) => i.take_suspended(),
            Exec::Vm { .. } => vec![],
        }
    }

    /// Resumes coroutine `token` (`value` is what its `await` evaluates to). Returns whether it
    /// finished (`Done(return value)`) or suspended again (`Suspended(awaited)`, same token).
    pub fn resume(&mut self, m: Module, token: u64, value: Value, state: &mut dyn Any) -> Result<Outcome, ScriptError> {
        let limit = self.step_limit;
        let md = &mut self.modules[m.0];
        let mut bridge = Bridge { fns: &self.fns, hooks: &self.hooks, self_id: self.self_id, ctx: Ctx { state, fns: Some(&self.fns) } };
        let r = match &mut md.exec {
            Exec::Interp(i) => {
                i.set_max_steps(limit);
                i.resume(token, value, &mut bridge)
            }
            Exec::Vm { .. } => Err(Diag::error(crate::diagnostics::Span::new(0, 0), "E0362", "coroutines need the interpreter backend")),
        };
        r.map_err(|d| Self::err(md, &d))
    }

    /// Drops a suspended coroutine (its owner is gone).
    pub fn drop_coroutine(&mut self, m: Module, token: u64) {
        if let Exec::Interp(i) = &mut self.modules[m.0].exec {
            i.drop_coroutine(token);
        }
    }

    /// Number of suspended coroutines in module `m`.
    pub fn coroutine_count(&self, m: Module) -> usize {
        match &self.modules[m.0].exec {
            Exec::Interp(i) => i.coroutine_count(),
            Exec::Vm { .. } => 0,
        }
    }

    pub fn call(&mut self, m: Module, f: &str, args: Vec<Value>, state: &mut dyn Any) -> Result<Value, ScriptError> {
        let limit = self.step_limit;
        let md = &mut self.modules[m.0];
        let mut bridge = Bridge { fns: &self.fns, hooks: &self.hooks, self_id: self.self_id, ctx: Ctx { state, fns: Some(&self.fns) } };
        let r = match &mut md.exec {
            Exec::Interp(i) => {
                i.set_max_steps(limit);
                i.call_named(f, args, &mut bridge)
            }
            Exec::Vm { bc, globals } => {
                let mut v = Vm::new_public(bc);
                v.set_source(&md.name, md.lines.clone());
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

/// What a source file declares at top level, in source order (no type check, no run).
/// Hosts use it to list a script's member variables and functions (e.g. in an inspector).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outline {
    /// Top-level variables, first assignment order, deduplicated.
    pub vars: Vec<String>,
    /// Top-level functions: name and parameter names.
    pub fns: Vec<(String, Vec<String>)>,
}

pub fn outline(src: &str) -> Outline {
    use crate::ast::StmtKind;
    let (prog, _) = crate::parser::parse_program(src);
    let mut o = Outline::default();
    for st in &prog {
        match &st.node {
            StmtKind::Var { name, .. } | StmtKind::Assign { name, .. } => {
                if !o.vars.contains(name) {
                    o.vars.push(name.clone());
                }
            }
            StmtKind::Fn { name, params, .. } => {
                o.fns.push((name.clone(), params.iter().map(|p| p.name.clone()).collect()));
            }
            _ => {}
        }
    }
    o
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

    #[test]
    fn fork_globals_and_methods() {
        for b in [Backend::Interpreter, Backend::Vm] {
            let mut base = engine(b);
            base.register("node.name", |_c, a| Ok(Value::Str(format!("n{}", a.len()))));
            base.register("game.level", |_c, _a| Ok(Value::Int(3)));
            let mut w = World::new();
            let src = "speed = 2;\nfn bump() { speed = speed + 1; return speed; }\n";
            let mut e1 = base.fork();
            let mut e2 = base.fork();
            let m1 = e1.load("a", src, &mut w).unwrap();
            let m2 = e2.load("a", src, &mut w).unwrap();
            assert_eq!(e1.call(m1, "bump", vec![], &mut w).unwrap(), Value::Int(3));
            assert_eq!(e1.global(m1, "speed"), Some(Value::Int(3)));
            assert_eq!(e2.global(m2, "speed"), Some(Value::Int(2)), "instances are separate");
            e2.set_global(m2, "speed", Value::Int(10));
            assert_eq!(e2.call(m2, "bump", vec![], &mut w).unwrap(), Value::Int(11));
            assert_eq!(e1.global_names(m1), vec!["speed".to_string()]);
        }
        // Methods on bare names route to "obj.method" host functions (interpreter path).
        let mut base = Engine::new(Backend::Interpreter);
        base.register("node.name", |_c, a| Ok(Value::Str(format!("n{}", a.len()))));
        base.register("game.level", |_c, _a| Ok(Value::Int(3)));
        let mut e = base.fork();
        let mut w = World::new();
        let m = e.load("m", "fn f() { return node.name(1, 2); }\nfn g() { return game.level; }", &mut w).unwrap();
        assert_eq!(e.call(m, "f", vec![], &mut w).unwrap(), Value::Str("n2".into()));
        assert_eq!(e.call(m, "g", vec![], &mut w).unwrap(), Value::Int(3));
    }

    /// A tiny object world: id -> field map; "pos" is a host-computed property.
    type Objs = HashMap<u64, HashMap<String, Value>>;

    fn obj_engine(b: Backend) -> Engine {
        let mut e = Engine::new(b);
        e.register("tostr", |_c, a| Ok(Value::Str(a[0].to_string())));
        e.register("spawn", |c, _a| {
            let w = c.state::<Objs>().ok_or("no world")?;
            let id = w.len() as u64 + 1;
            w.insert(id, HashMap::new());
            Ok(Value::Object(id))
        });
        e.register_object_get(|c, id, name| {
            let w = c.state::<Objs>().ok_or("no world")?;
            w.get(&id).and_then(|o| o.get(name)).cloned().ok_or(format!("no property `{name}`"))
        });
        e.register_object_set(|c, id, name, v| {
            let w = c.state::<Objs>().ok_or("no world")?;
            w.get_mut(&id).ok_or("gone")?.insert(name.to_string(), v);
            Ok(())
        });
        e.register("get_name", |c, a| {
            let Value::Object(id) = a[0] else { return Err("not an object".into()) };
            let w = c.state::<Objs>().ok_or("no world")?;
            Ok(w[&id].get("name").cloned().unwrap_or(Value::Null))
        });
        e.register_object_call(|c, id, name, args| {
            if c.has_host(name) {
                let mut a = vec![Value::Object(id)];
                a.extend_from_slice(args);
                return c.call_host(name, &a);
            }
            Ok(Value::Str(format!("{id}.{name}/{}", args.len())))
        });
        e
    }

    #[test]
    fn object_handles_get_set_call_and_compare() {
        let src = r#"
fn run() {
    a = spawn(); b = spawn();
    a.hp = 10;
    a.hp -= 3;
    a.name = "kid";
    out = [a.hp, a.name, a.go(1, 2), a.get_name(), a == b, a == a, is_null(a), tostr(a), get_name(a)];
    return out;
}
"#;
        for b in [Backend::Interpreter] {
            let mut e = obj_engine(b);
            let mut w = Objs::new();
            let m = e.load("o", src, &mut w).unwrap_or_else(|er| panic!("{er:?}"));
            let v = e.call(m, "run", vec![], &mut w).unwrap_or_else(|er| panic!("{er}"));
            assert_eq!(v.to_string(), "[7, kid, 1.go/2, kid, false, true, false, Node#1, kid]", "{b:?}");
        }
    }

    #[test]
    fn self_handle_prefers_own_globals_and_functions() {
        let src = r#"
hp = 5;
fn bump() { self.hp += 1; self.pos = 3; return self.hp; }
fn twice() { return self.bump() + self.bump(); }
fn other() { return self.later(2); }
fn me() { return self; }
"#;
        for b in [Backend::Interpreter] {
            let mut e = obj_engine(b);
            e.set_self_handle(Some(9));
            let mut w = Objs::new();
            w.insert(9, HashMap::new());
            let m = e.load("s", src, &mut w).unwrap_or_else(|er| panic!("{er:?}"));
            assert_eq!(e.call(m, "bump", vec![], &mut w).unwrap(), Value::Int(6), "{b:?}");
            assert_eq!(e.global(m, "hp"), Some(Value::Int(6)));
            assert_eq!(w[&9]["pos"], Value::Int(3), "non-member goes to the host");
            assert_eq!(e.call(m, "twice", vec![], &mut w).unwrap(), Value::Int(15));
            assert_eq!(e.call(m, "other", vec![], &mut w).unwrap(), Value::Str("9.later/1".into()));
            assert_eq!(e.call(m, "me", vec![], &mut w).unwrap(), Value::Object(9));
        }
    }

    #[test]
    fn dicts_need_the_interpreter_and_the_vm_reports_it() {
        let src = r#"fn f() { d = {"a": 1}; d.b = 2; d["a"] += 1; return d["a"] + d.b + len(d); }"#;
        let mut w = World::new();
        let mut i = Engine::new(Backend::Interpreter);
        let m = i.load("t", src, &mut w).unwrap();
        assert_eq!(i.call(m, "f", vec![], &mut w).unwrap(), Value::Int(6));
        let errs = Engine::new(Backend::Vm).load("t", src, &mut w).unwrap_err();
        assert_eq!(errs[0].code, "E0900");
    }

    #[test]
    fn vm_reads_object_properties() {
        let src = "fn f() { a = spawn(); a2 = a; return a2.hp; }\nfn g() { return self.hp; }";
        let mut e = obj_engine(Backend::Vm);
        e.set_self_handle(Some(1));
        let mut w = Objs::new();
        w.insert(1, HashMap::from([("hp".to_string(), Value::Int(4))]));
        let m = e.load("v", src, &mut w).unwrap_or_else(|er| panic!("{er:?}"));
        assert_eq!(e.call(m, "f", vec![], &mut w).unwrap_err().code, "E0331"); // new object has no hp
        assert_eq!(e.call(m, "g", vec![], &mut w).unwrap(), Value::Int(4));
    }

    fn aw(v: &str) -> Value {
        Value::Str(v.into())
    }

    #[test]
    fn coroutines_suspend_resume_and_keep_locals() {
        let src = r#"
total = 0;
log_line = "";
fn run(n) {
    i = 0;
    while i < n {
        got = await "tick";
        total += got;
        if got > 5 {
            x = await "big";
            log_line += x;
            continue;
        }
        i += 1;
    }
    for k in [10, 20] {
        await k;
        if k == 10 { continue; }
    }
    match n { 2 => { y = await "m"; total += y; } _ => { } }
    return total + 1000;
}
fn plain() { return 7; }
"#;
        let mut e = Engine::new(Backend::Interpreter);
        let mut w = World::new();
        let m = e.load("c", src, &mut w).unwrap_or_else(|er| panic!("{er:?}"));
        assert!(e.is_coroutine(m, "run") && !e.is_coroutine(m, "plain"));
        assert_eq!(e.call(m, "plain", vec![], &mut w).unwrap(), Value::Int(7));
        assert!(e.take_suspended(m).is_empty());
        assert_eq!(e.call(m, "run", vec![Value::Int(2)], &mut w).unwrap(), Value::Void);
        let s = e.take_suspended(m);
        assert_eq!(s, vec![(1, aw("tick"))]);
        assert_eq!(e.coroutine_count(m), 1);
        // tick(1), tick(9 -> big), tick(2), then the for loop (10, 20), then match
        assert_eq!(e.resume(m, 1, Value::Int(1), &mut w).unwrap(), Outcome::Suspended(aw("tick")));
        assert_eq!(e.resume(m, 1, Value::Int(9), &mut w).unwrap(), Outcome::Suspended(aw("big")));
        assert_eq!(e.resume(m, 1, aw("!"), &mut w).unwrap(), Outcome::Suspended(aw("tick")));
        assert_eq!(e.resume(m, 1, Value::Int(2), &mut w).unwrap(), Outcome::Suspended(Value::Int(10)));
        assert_eq!(e.resume(m, 1, Value::Null, &mut w).unwrap(), Outcome::Suspended(Value::Int(20)));
        assert_eq!(e.resume(m, 1, Value::Null, &mut w).unwrap(), Outcome::Suspended(aw("m")));
        assert_eq!(e.resume(m, 1, Value::Int(100), &mut w).unwrap(), Outcome::Done(Value::Int(1112)));
        assert_eq!(e.global(m, "log_line"), Some(aw("!")));
        assert_eq!(e.coroutine_count(m), 0);
        assert_eq!(e.resume(m, 1, Value::Null, &mut w).unwrap_err().code, "E0362");
    }

    #[test]
    fn coroutines_are_independent_and_droppable_and_errors_are_located() {
        let src = r#"n = 0;
fn go(tag) { v = await tag; n += v; return n; }
fn boom() {
  await 1;
  y = 1 / 0;
}
fn nested() { z = go("inner"); return z; }
"#;
        let mut e = Engine::new(Backend::Interpreter);
        let mut w = World::new();
        let m = e.load("c", src, &mut w).unwrap();
        e.call(m, "go", vec![aw("a")], &mut w).unwrap();
        e.call(m, "go", vec![aw("b")], &mut w).unwrap();
        let toks: Vec<u64> = e.take_suspended(m).into_iter().map(|(t, _)| t).collect();
        assert_eq!(toks.len(), 2);
        e.drop_coroutine(m, toks[0]);
        assert_eq!(e.resume(m, toks[1], Value::Int(5), &mut w).unwrap(), Outcome::Done(Value::Int(5)));
        // A call from ordinary code starts a detached coroutine and yields void.
        assert_eq!(e.call(m, "nested", vec![], &mut w).unwrap(), Value::Void);
        assert_eq!(e.take_suspended(m).len(), 1);
        e.call(m, "boom", vec![], &mut w).unwrap();
        let t = e.take_suspended(m).pop().unwrap().0;
        let err = e.resume(m, t, Value::Null, &mut w).unwrap_err();
        assert_eq!((err.line, err.code), (5, "E0205"), "{err}");
        assert_eq!(e.coroutine_count(m), 1, "only the detached `nested` one is left: dropped and failed ones are freed");
    }

    #[test]
    fn await_must_be_a_statement_and_needs_the_interpreter() {
        let mut w = World::new();
        let mut e = Engine::new(Backend::Interpreter);
        let errs = e.load("c", "fn f() { x = 1 + await 2; }", &mut w).unwrap_err();
        assert_eq!(errs[0].code, "E0360");
        let errs = e.load("c", "x = await 2;", &mut w).unwrap_err();
        assert_eq!(errs[0].code, "E0361");
        let errs = Engine::new(Backend::Vm).load("c", "fn f() { await 1; }", &mut w).unwrap_err();
        assert_eq!(errs[0].code, "E0900");
    }

    #[test]
    fn errors_carry_a_call_stack_and_get_stack_works_on_both_backends() {
        let src = r#"fn a(x) {
    return b(x) + 1;
}
fn b(x) {
    y = 10 / x;
    return y;
}
fn stack() { return c(); }
fn c() { return get_stack(); }
fn show() { print_stack(); }
"#;
        for bk in [Backend::Interpreter, Backend::Vm] {
            let mut e = Engine::new(bk);
            let logged = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
            let l2 = logged.clone();
            e.register("print", move |_c, a| {
                l2.borrow_mut().push(a[0].to_string());
                Ok(Value::Void)
            });
            let mut w = World::new();
            let m = e.load("t.ruspy", src, &mut w).unwrap_or_else(|er| panic!("{er:?}"));
            let err = e.call(m, "a", vec![Value::Int(0)], &mut w).unwrap_err();
            let t: Vec<_> = err.trace.iter().map(|(f, l, _)| (f.as_str(), *l)).collect();
            assert_eq!(t, vec![("b", 5), ("a", 2)], "{bk:?}: {err}");
            assert_eq!(err.trace_text().lines().next().unwrap(), "  at b (t.ruspy:5:9)", "{bk:?}");
            let v = e.call(m, "stack", vec![], &mut w).unwrap();
            assert_eq!(
                v.to_string(),
                r#"[{"function": "c", "file": "t.ruspy", "line": 9, "col": 17}, {"function": "stack", "file": "t.ruspy", "line": 8, "col": 21}]"#,
                "{bk:?}"
            );
            e.call(m, "show", vec![], &mut w).unwrap();
            assert_eq!(logged.borrow()[0], "stack:\n  at show (t.ruspy:10:13)", "{bk:?}");
        }
    }

    #[test]
    fn coroutine_errors_have_a_trace() {
        let src = "fn go() {\n  await 1;\n  helper();\n}\nfn helper() {\n  x = 1 / 0;\n}\n";
        let mut e = Engine::new(Backend::Interpreter);
        let mut w = World::new();
        let m = e.load("c", src, &mut w).unwrap();
        e.call(m, "go", vec![], &mut w).unwrap();
        let t = e.take_suspended(m).pop().unwrap().0;
        let err = e.resume(m, t, Value::Null, &mut w).unwrap_err();
        let names: Vec<_> = err.trace.iter().map(|(f, l, _)| (f.as_str(), *l)).collect();
        assert_eq!(names, vec![("helper", 6), ("go", 3)]);
    }

    #[test]
    fn int_division_warns_and_casts_work() {
        let src = "hp = 7;\nmax_hp: int = 2;\nfn bar() { return hp / max_hp; }\nfn ok1() { return 7.0 / 2; }\nfn ok2(a, b) { return a / b; }\nfn ok3() { return float(hp) / max_hp; }\nfn ok4() { return idiv(hp, max_hp); }\nfn lit() { return 1 / 2; }\n";
        let mut w = World::new();
        let mut e = Engine::new(Backend::Interpreter);
        let m = e.load("w.ruspy", src, &mut w).unwrap();
        let ws: Vec<_> = e.warnings(m).iter().map(|x| (x.code, x.line)).collect();
        assert_eq!(ws, vec![("W0101", 3), ("W0101", 8)], "{:?}", e.warnings(m));
        // Nothing changes at runtime.
        assert_eq!(e.call(m, "bar", vec![], &mut w).unwrap(), Value::Int(3));
        assert_eq!(e.call(m, "ok1", vec![], &mut w).unwrap(), Value::Float(3.5));
        assert_eq!(e.call(m, "ok3", vec![], &mut w).unwrap(), Value::Float(3.5));
        assert_eq!(e.call(m, "ok4", vec![], &mut w).unwrap(), Value::Int(3));
        assert_eq!(e.call(m, "ok2", vec![Value::Int(7), Value::Int(2)], &mut w).unwrap(), Value::Int(3));
        let mut e2 = Engine::new(Backend::Vm);
        let m2 = e2.load("w.ruspy", "fn f() { return 3 / 2; }", &mut w).unwrap();
        assert_eq!(e2.warnings(m2).len(), 1);
    }

    #[test]
    fn casts() {
        let src = r#"fn f() {
    return [int(3.9), int("42"), int("x"), int(true), float(2), float("1.5"), str(12) + "!", str(1.5), bool(0), bool(""), bool("a"), bool([]), bool([0]), bool(null), int(-2.7)];
}
fn g() { x: int = int(7.5); return x; }"#;
        for bk in [Backend::Interpreter] {
            let mut e = Engine::new(bk);
            let mut w = World::new();
            let m = e.load("c", src, &mut w).unwrap_or_else(|er| panic!("{er:?}"));
            let v = e.call(m, "f", vec![], &mut w).unwrap();
            assert_eq!(v.to_string(), "[3, 42, 0, 1, 2, 1.5, 12!, 1.5, false, false, true, false, true, false, -2]");
            assert_eq!(e.call(m, "g", vec![], &mut w).unwrap(), Value::Int(7));
        }
    }

    #[test]
    fn outline_lists_vars_and_fns() {
        let o = outline("speed = 5.0;\nhp = 3;\nspeed = 6.0;\nfn _ready() { }\nfn hit(dmg, from) { hp = hp - dmg; }\n");
        assert_eq!(o.vars, vec!["speed".to_string(), "hp".to_string()]);
        assert_eq!(o.fns, vec![("_ready".to_string(), vec![]), ("hit".to_string(), vec!["dmg".to_string(), "from".to_string()])]);
    }
}
