//! Tree-walking interpreter over the `Expr`/`Stmt` AST.
//!
//! The reference semantics engine the VM (M13) and Cranelift (M14) must match. It
//! never panics and never wedges: every failure is a `Diag`, recursion is depth-
//! capped, and a fuel counter bounds total work so an infinite loop returns a
//! diagnostic instead of hanging the host. Functions get their own call frame with
//! read-through to globals; free calls / `obj.field` still delegate to a `RuspyHost`.

use crate::ast::{Expr, ExprKind, LogicOp, Program, Stmt, StmtKind, UnOp};
use crate::diagnostics::{Diag, Span};
use crate::value::{self, Value};
use std::collections::HashMap;

/// Max nested call depth before a diagnostic (prevents native stack overflow).
const MAX_DEPTH: usize = 512;
/// Max evaluation steps before a diagnostic (bounds infinite loops/recursion).
const MAX_STEPS: u64 = 20_000_000;

/// The host environment ruspy delegates unresolved calls and member accesses to.
pub trait RuspyHost {
    fn call_function(&mut self, name: &str, args: Vec<Value>) -> Result<Value, String>;
    fn get_member(&mut self, object: &str, member: &str) -> Result<Value, String>;
    /// Call a package method `object.method(args)` (e.g. `trade.buy(0.1)`). The
    /// interpreter tries the built-in pure packages (`crate::packages`) first and
    /// only falls here for host-backed packages (account / trade / series). The
    /// default rejects everything, so a host opts in by overriding this.
    fn call_method(&mut self, object: &str, method: &str, args: Vec<Value>) -> Result<Value, String> {
        let _ = args;
        Err(format!("no package method `{object}.{method}`"))
    }
}

/// A host that provides nothing — every call/member access is an error.
pub struct DefaultHost;
impl RuspyHost for DefaultHost {
    fn call_function(&mut self, name: &str, _args: Vec<Value>) -> Result<Value, String> {
        Err(format!("function `{name}` is not defined"))
    }
    fn get_member(&mut self, object: &str, member: &str) -> Result<Value, String> {
        Err(format!("`{object}` has no member `{member}`"))
    }
}

/// A user-defined function.
#[derive(Clone)]
struct FnDef {
    params: Vec<String>,
    body: Vec<Stmt>,
}

/// Non-local control flow threaded through statement execution.
enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

pub struct Interpreter {
    /// Scope frames: `[0]` is globals; a call pushes a locals frame.
    frames: Vec<HashMap<String, Value>>,
    funcs: HashMap<String, FnDef>,
    /// Declared struct name → its field names (for literal validation).
    structs: HashMap<String, Vec<String>>,
    output: Vec<String>,
    steps: u64,
    depth: usize,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        Interpreter {
            frames: vec![HashMap::new()],
            funcs: HashMap::new(),
            structs: HashMap::new(),
            output: Vec::new(),
            steps: 0,
            depth: 0,
        }
    }

    /// Inject a global variable from the host (e.g. a strategy's `tick`).
    pub fn set_var(&mut self, name: impl Into<String>, value: Value) {
        self.frames[0].insert(name.into(), value);
    }

    pub fn output(&self) -> &[String] {
        &self.output
    }

    /// Drain the accumulated `print` output (e.g. to journal one strategy tick and
    /// start the next with an empty buffer).
    pub fn take_output(&mut self) -> Vec<String> {
        std::mem::take(&mut self.output)
    }

    /// Is `name` a top-level function the program defined? Used by a strategy host to
    /// probe for optional lifecycle hooks (`on_init` / `on_tick` / `on_deinit`).
    pub fn has_fn(&self, name: &str) -> bool {
        self.funcs.contains_key(name)
    }

    /// Call a top-level function by name with already-evaluated arguments — the entry
    /// point a host uses to drive a strategy lifecycle (`on_tick` once per market tick).
    ///
    /// Each invocation gets a **fresh fuel budget**: `MAX_STEPS` bounds one hook call,
    /// not the whole backtest, so millions of ticks don't exhaust a single counter.
    /// Globals and function definitions persist across calls (interpreter state is
    /// retained), so a strategy can keep state between ticks.
    pub fn call_named(
        &mut self,
        name: &str,
        args: Vec<Value>,
        host: &mut dyn RuspyHost,
    ) -> Result<Value, Diag> {
        self.steps = 0;
        self.depth = 0;
        if !self.funcs.contains_key(name) {
            return Err(Diag::error(Span::new(0, 0), "E0303", format!("no function `{name}` to call")));
        }
        self.call(name, args, Span::new(0, 0), host)
    }

    /// Execute a whole program: hoist top-level functions first (so definitions may
    /// appear after use and recurse mutually), then run the remaining statements.
    pub fn run(&mut self, prog: &Program, host: &mut dyn RuspyHost) -> Result<(), Diag> {
        for stmt in prog {
            match &stmt.node {
                StmtKind::Fn { name, params, body, .. } => {
                    self.funcs.insert(name.clone(), FnDef { params: params.iter().map(|p| p.name.clone()).collect(), body: body.clone() });
                }
                StmtKind::Struct { name, fields } => {
                    self.structs.insert(name.clone(), fields.clone());
                }
                _ => {}
            }
        }
        for stmt in prog {
            match self.exec(stmt, host)? {
                Flow::Normal => {}
                Flow::Return(_) => return Err(Diag::error(stmt.span, "E0304", "`return` outside of a function")),
                Flow::Break => return Err(Diag::error(stmt.span, "E0305", "`break` outside of a loop")),
                Flow::Continue => return Err(Diag::error(stmt.span, "E0306", "`continue` outside of a loop")),
            }
        }
        Ok(())
    }

    fn tick(&mut self, span: Span) -> Result<(), Diag> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(Diag::error(span, "E0307", "execution budget exceeded")
                .with_help("a loop or recursion may not terminate"));
        }
        Ok(())
    }

    fn exec(&mut self, stmt: &Stmt, host: &mut dyn RuspyHost) -> Result<Flow, Diag> {
        self.tick(stmt.span)?;
        match &stmt.node {
            // Imports are resolved before execution (file imports spliced by the
            // loader; package imports enforced by the checker) — a runtime no-op.
            StmtKind::Import { .. } => Ok(Flow::Normal),
            StmtKind::Fn { name, params, body, .. } => {
                // nested definitions register globally too
                self.funcs.insert(name.clone(), FnDef { params: params.iter().map(|p| p.name.clone()).collect(), body: body.clone() });
                Ok(Flow::Normal)
            }
            StmtKind::Var { name, value, .. } => {
                let v = self.eval(value, host)?;
                self.declare(name, v);
                Ok(Flow::Normal)
            }
            StmtKind::Assign { name, name_span, op, value } => {
                let rhs = self.eval(value, host)?;
                let new = match op {
                    Some(o) => {
                        let cur = self.lookup(name).cloned().ok_or_else(|| undefined(name, *name_span))?;
                        value::binop(*o, &cur, &rhs, stmt.span)?
                    }
                    None => rhs,
                };
                if !self.assign_existing(name, new.clone()) {
                    // plain `=` declares; compound assign to an unknown var is an error
                    if op.is_some() {
                        return Err(undefined(name, *name_span));
                    }
                    self.declare(name, new);
                }
                Ok(Flow::Normal)
            }
            StmtKind::IndexAssign { array, index, op, value } => {
                let a = self.eval(array, host)?;
                let i = self.eval(index, host)?;
                let rhs = self.eval(value, host)?;
                let (arr_ref, i) = index_of(&a, &i, stmt.span)?;
                let new = match op {
                    Some(o) => {
                        let cur = arr_ref.borrow()[i].clone();
                        value::binop(*o, &cur, &rhs, stmt.span)?
                    }
                    None => rhs,
                };
                arr_ref.borrow_mut()[i] = new;
                Ok(Flow::Normal)
            }
            StmtKind::Struct { name, fields } => {
                self.structs.insert(name.clone(), fields.clone());
                Ok(Flow::Normal)
            }
            StmtKind::FieldAssign { object, field, op, value } => {
                let obj = self.eval(object, host)?;
                let rhs = self.eval(value, host)?;
                let Value::Struct(sname, map) = obj else {
                    return Err(Diag::error(object.span, "E0320", format!("cannot set field `{field}` on `{}`", obj.type_name())));
                };
                if !map.borrow().contains_key(field) {
                    return Err(Diag::error(stmt.span, "E0321", format!("struct `{sname}` has no field `{field}`")));
                }
                let new = match op {
                    Some(o) => {
                        let cur = map.borrow()[field].clone();
                        value::binop(*o, &cur, &rhs, stmt.span)?
                    }
                    None => rhs,
                };
                map.borrow_mut().insert(field.clone(), new);
                Ok(Flow::Normal)
            }
            StmtKind::Print(e) => {
                let v = self.eval(e, host)?;
                self.output.push(v.to_string());
                Ok(Flow::Normal)
            }
            StmtKind::Expr(e) => {
                self.eval(e, host)?;
                Ok(Flow::Normal)
            }
            StmtKind::Block(body) => self.exec_block(body, host),
            StmtKind::If { cond, then, els } => {
                let c = self.eval(cond, host)?;
                if c.as_bool(cond.span)? {
                    self.exec_block(then, host)
                } else if let Some(else_body) = els {
                    self.exec_block(else_body, host)
                } else {
                    Ok(Flow::Normal)
                }
            }
            StmtKind::Return(e) => {
                let v = match e {
                    Some(expr) => self.eval(expr, host)?,
                    None => Value::Void,
                };
                Ok(Flow::Return(v))
            }
            StmtKind::Break => Ok(Flow::Break),
            StmtKind::Continue => Ok(Flow::Continue),
            StmtKind::While { cond, body } => {
                loop {
                    self.tick(stmt.span)?;
                    let c = self.eval(cond, host)?;
                    if !c.as_bool(cond.span)? {
                        break;
                    }
                    match self.exec_block(body, host)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        Flow::Continue | Flow::Normal => {}
                    }
                }
                Ok(Flow::Normal)
            }
            StmtKind::For { var, iter, body } => {
                // Iterate either an integer range or a snapshot of an array's elements.
                let items = self.for_items(iter, host)?;
                for v in items {
                    self.tick(stmt.span)?;
                    self.declare(var, v);
                    match self.exec_block(body, host)? {
                        Flow::Break => break,
                        Flow::Return(rv) => return Ok(Flow::Return(rv)),
                        Flow::Continue | Flow::Normal => {}
                    }
                }
                Ok(Flow::Normal)
            }
        }
    }

    /// Execute a sequence of statements, short-circuiting on non-`Normal` flow.
    fn exec_block(&mut self, body: &[Stmt], host: &mut dyn RuspyHost) -> Result<Flow, Diag> {
        for s in body {
            match self.exec(s, host)? {
                Flow::Normal => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    /// Materialize the values a `for` iterates: an integer range or an array snapshot
    /// (snapshotting means mutating the array in the body is safe).
    fn for_items(&mut self, iter: &Expr, host: &mut dyn RuspyHost) -> Result<Vec<Value>, Diag> {
        match &iter.node {
            ExprKind::Range { start, end, inclusive } => {
                let s = self.eval(start, host)?;
                let e = self.eval(end, host)?;
                match (s, e) {
                    (Value::Int(s), Value::Int(e)) => {
                        let last = if *inclusive { e } else { e - 1 };
                        Ok((s..=last).map(Value::Int).collect())
                    }
                    _ => Err(Diag::error(iter.span, "E0308", "range bounds must be integers")),
                }
            }
            _ => match self.eval(iter, host)? {
                Value::Array(a) => Ok(a.borrow().clone()),
                other => Err(Diag::error(iter.span, "E0309", format!("`for` expects a range or array, found `{}`", other.type_name()))),
            },
        }
    }

    fn eval(&mut self, expr: &Expr, host: &mut dyn RuspyHost) -> Result<Value, Diag> {
        self.tick(expr.span)?;
        let span = expr.span;
        match &expr.node {
            ExprKind::Int(v) => Ok(Value::Int(*v)),
            ExprKind::Float(v) => Ok(Value::Float(*v)),
            ExprKind::Str(s) => Ok(Value::Str(s.clone())),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Ident(n) => self.lookup(n).cloned().ok_or_else(|| undefined(n, span)),
            ExprKind::Unary(UnOp::Neg, e) => {
                let v = self.eval(e, host)?;
                value::neg(&v, span)
            }
            ExprKind::Unary(UnOp::Not, e) => {
                let v = self.eval(e, host)?;
                value::not(&v, span)
            }
            ExprKind::Binary(op, l, r) => {
                let lv = self.eval(l, host)?;
                let rv = self.eval(r, host)?;
                value::binop(*op, &lv, &rv, span)
            }
            ExprKind::Logic(LogicOp::And, l, r) => {
                let lv = self.eval(l, host)?;
                if !lv.as_bool(l.span)? {
                    return Ok(Value::Bool(false));
                }
                Ok(Value::Bool(self.eval(r, host)?.as_bool(r.span)?))
            }
            ExprKind::Logic(LogicOp::Or, l, r) => {
                let lv = self.eval(l, host)?;
                if lv.as_bool(l.span)? {
                    return Ok(Value::Bool(true));
                }
                Ok(Value::Bool(self.eval(r, host)?.as_bool(r.span)?))
            }
            ExprKind::Call(name, args) => {
                let mut argv = Vec::with_capacity(args.len());
                for a in args {
                    argv.push(self.eval(a, host)?);
                }
                self.call(name, argv, span, host)
            }
            ExprKind::Member(obj, field) => {
                // A bare identifier that is not a local variable is a package (pure
                // built-ins first, e.g. `math.PI`) or a host object (`tick.bid`).
                if let ExprKind::Ident(id) = &obj.node {
                    if self.lookup(id).is_none() {
                        if let Some(result) = crate::packages::get_member(id, field, span) {
                            return result;
                        }
                        return host.get_member(id, field).map_err(|msg| Diag::error(span, "E0301", msg));
                    }
                }
                // Otherwise the object must evaluate to a struct; read its field.
                match self.eval(obj, host)? {
                    Value::Struct(sname, map) => map
                        .borrow()
                        .get(field)
                        .cloned()
                        .ok_or_else(|| Diag::error(span, "E0321", format!("struct `{sname}` has no field `{field}`"))),
                    other => Err(Diag::error(span, "E0322", format!("`{}` has no field `{field}`", other.type_name()))),
                }
            }
            ExprKind::StructLit { name, fields } => {
                let Some(decl_fields) = self.structs.get(name).cloned() else {
                    return Err(Diag::error(span, "E0323", format!("unknown struct `{name}`")));
                };
                let mut map = HashMap::new();
                for (f, e) in fields {
                    if !decl_fields.contains(f) {
                        return Err(Diag::error(e.span, "E0324", format!("struct `{name}` has no field `{f}`")));
                    }
                    map.insert(f.clone(), self.eval(e, host)?);
                }
                for f in &decl_fields {
                    if !map.contains_key(f) {
                        return Err(Diag::error(span, "E0325", format!("missing field `{f}` in `{name}` literal")));
                    }
                }
                Ok(Value::strukt(name.clone(), map))
            }
            ExprKind::Range { .. } => {
                Err(Diag::error(span, "E0310", "a range can only be used as a `for` iterator"))
            }
            ExprKind::Array(items) => {
                let mut vals = Vec::with_capacity(items.len());
                for it in items {
                    vals.push(self.eval(it, host)?);
                }
                Ok(Value::array(vals))
            }
            ExprKind::Index(arr, idx) => {
                let a = self.eval(arr, host)?;
                let i = self.eval(idx, host)?;
                let (arr_ref, i) = index_of(&a, &i, span)?;
                let elem = arr_ref.borrow()[i].clone();
                Ok(elem)
            }
            ExprKind::Method(recv, name, args) => {
                // A method on a bare identifier that isn't a local variable is a
                // package call (`math.sin(x)`, `trade.buy(0.1)`): pure packages
                // first, then the host. Otherwise it's a value method (array builtins).
                if let ExprKind::Ident(id) = &recv.node {
                    if self.lookup(id).is_none() {
                        let mut argv = Vec::with_capacity(args.len());
                        for a in args {
                            argv.push(self.eval(a, host)?);
                        }
                        if let Some(result) = crate::packages::call_method(id, name, &argv, span) {
                            return result;
                        }
                        return host
                            .call_method(id, name, argv)
                            .map_err(|msg| Diag::error(span, "E0330", msg));
                    }
                }
                let r = self.eval(recv, host)?;
                let mut argv = Vec::with_capacity(args.len());
                for a in args {
                    argv.push(self.eval(a, host)?);
                }
                array_method(&r, name, argv, span)
            }
        }
    }

    /// Resolve a call: user functions first (so a user may shadow a builtin), then
    /// the stdlib builtins, then the host.
    fn call(&mut self, name: &str, args: Vec<Value>, span: Span, host: &mut dyn RuspyHost) -> Result<Value, Diag> {
        let Some(func) = self.funcs.get(name).cloned() else {
            if let Some(result) = crate::stdlib::call(name, &args, span) {
                return result;
            }
            return host.call_function(name, args).map_err(|msg| Diag::error(span, "E0300", msg));
        };
        if args.len() != func.params.len() {
            return Err(Diag::error(
                span,
                "E0311",
                format!("`{name}` expects {} argument(s), got {}", func.params.len(), args.len()),
            ));
        }
        if self.depth + 1 > MAX_DEPTH {
            return Err(Diag::error(span, "E0312", "maximum call depth exceeded")
                .with_help("this recursion may not terminate"));
        }
        self.depth += 1;
        let mut frame = HashMap::new();
        for (p, a) in func.params.iter().zip(args) {
            frame.insert(p.clone(), a);
        }
        self.frames.push(frame);
        let flow = self.exec_block(&func.body, host);
        self.frames.pop();
        self.depth -= 1;
        match flow? {
            Flow::Return(v) => Ok(v),
            Flow::Normal => Ok(Value::Void),
            Flow::Break => Err(Diag::error(span, "E0305", "`break` outside of a loop")),
            Flow::Continue => Err(Diag::error(span, "E0306", "`continue` outside of a loop")),
        }
    }

    // ── scope helpers (locals frame with read-through to globals) ─────────────
    fn declare(&mut self, name: &str, value: Value) {
        self.frames.last_mut().unwrap().insert(name.to_string(), value);
    }
    fn lookup(&self, name: &str) -> Option<&Value> {
        if let Some(v) = self.frames.last().unwrap().get(name) {
            return Some(v);
        }
        if self.frames.len() > 1 {
            return self.frames[0].get(name);
        }
        None
    }
    /// Update an existing binding (locals first, then globals). `false` if unbound.
    fn assign_existing(&mut self, name: &str, value: Value) -> bool {
        if self.frames.last().unwrap().contains_key(name) {
            self.frames.last_mut().unwrap().insert(name.to_string(), value);
            return true;
        }
        if self.frames.len() > 1 && self.frames[0].contains_key(name) {
            self.frames[0].insert(name.to_string(), value);
            return true;
        }
        false
    }
}

fn undefined(name: &str, span: Span) -> Diag {
    Diag::error(span, "E0303", format!("undefined variable `{name}`"))
}

/// Validate `arr[idx]`: `arr` must be an array, `idx` a non-negative in-bounds int.
/// Returns the array handle + the usize index (no wraparound, no panic).
fn index_of(arr: &Value, idx: &Value, span: Span) -> Result<(crate::value::ArrayRef, usize), Diag> {
    let Value::Array(a) = arr else {
        return Err(Diag::error(span, "E0313", format!("cannot index `{}` — not an array", arr.type_name())));
    };
    let Value::Int(i) = idx else {
        return Err(Diag::error(span, "E0314", format!("array index must be an int, found `{}`", idx.type_name())));
    };
    let len = a.borrow().len();
    if *i < 0 || (*i as usize) >= len {
        return Err(Diag::error(span, "E0315", format!("index {i} out of bounds for array of length {len}")));
    }
    Ok((a.clone(), *i as usize))
}

/// Dispatch a builtin array method. Non-array receivers, unknown methods, and arity
/// or empty-array misuse all return diagnostics (never a panic).
fn array_method(recv: &Value, name: &str, args: Vec<Value>, span: Span) -> Result<Value, Diag> {
    let Value::Array(a) = recv else {
        return Err(Diag::error(span, "E0316", format!("`{}` has no method `{name}`", recv.type_name())));
    };
    let arity = |want: usize| -> Result<(), Diag> {
        if args.len() == want {
            Ok(())
        } else {
            Err(Diag::error(span, "E0317", format!("`{name}` takes {want} argument(s), got {}", args.len())))
        }
    };
    let empty = || Diag::error(span, "E0318", format!("`{name}` on an empty array"));
    match name {
        "len" => {
            arity(0)?;
            Ok(Value::Int(a.borrow().len() as i64))
        }
        "push" => {
            arity(1)?;
            a.borrow_mut().push(args.into_iter().next().unwrap());
            Ok(Value::Void)
        }
        "pop" => {
            arity(0)?;
            a.borrow_mut().pop().ok_or_else(empty)
        }
        "first" => {
            arity(0)?;
            a.borrow().first().cloned().ok_or_else(empty)
        }
        "last" => {
            arity(0)?;
            a.borrow().last().cloned().ok_or_else(empty)
        }
        "clear" => {
            arity(0)?;
            a.borrow_mut().clear();
            Ok(Value::Void)
        }
        "contains" => {
            arity(1)?;
            let needle = &args[0];
            let found = a.borrow().iter().any(|v| value::binop(value::BinOp::Eq, v, needle, span).map(|r| r == Value::Bool(true)).unwrap_or(false));
            Ok(Value::Bool(found))
        }
        "sum" => {
            arity(0)?;
            let mut acc = Value::Int(0);
            for v in a.borrow().iter() {
                acc = value::binop(value::BinOp::Add, &acc, v, span)?;
            }
            Ok(acc)
        }
        "min" | "max" => {
            arity(0)?;
            let b = a.borrow();
            let mut it = b.iter();
            let mut best = it.next().cloned().ok_or_else(empty)?;
            let op = if name == "min" { value::BinOp::Lt } else { value::BinOp::Gt };
            for v in it {
                if value::binop(op, v, &best, span)? == Value::Bool(true) {
                    best = v.clone();
                }
            }
            Ok(best)
        }
        _ => Err(Diag::error(span, "E0319", format!("unknown array method `{name}`"))
            .with_help("try len/push/pop/first/last/clear/contains/sum/min/max")),
    }
}

/// Run a program string with the default (no-op) host, returning either the `print`
/// output or the diagnostics. The single oracle entry point for tests + tooling.
///
/// The tree-walker recurses on the native stack, so this runs on a dedicated
/// large-stack thread — the `MAX_DEPTH` guard then fires with a diagnostic on deep
/// recursion instead of overflowing a small caller stack. (The register VM in M13
/// is iterative and removes this constraint.)
pub fn run_str(src: &str) -> Result<Vec<String>, Vec<Diag>> {
    let (prog, diags) = crate::parser::parse_program(src);
    if diags.iter().any(|d| d.is_error()) {
        return Err(diags);
    }
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let mut host = DefaultHost;
            let mut interp = Interpreter::new();
            match interp.run(&prog, &mut host) {
                Ok(()) => Ok(interp.output().to_vec()),
                Err(d) => Err(vec![d]),
            }
        })
        .expect("spawn ruspy runner thread")
        .join()
        .expect("ruspy runner thread panicked")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(src: &str) -> Vec<String> {
        run_str(src).unwrap_or_else(|d| panic!("unexpected diagnostics: {d:?}"))
    }
    fn err(src: &str) -> Vec<Diag> {
        run_str(src).expect_err("expected an error")
    }

    #[test]
    fn arithmetic_bodmas_and_print() {
        let o = out("x: int64 = 10; y: int64 = 5; r: int64 = (x + y) * 2 / (5 - 2); print r;");
        assert_eq!(o, vec!["10"]);
    }

    #[test]
    fn booleans_if_else_if_and_short_circuit() {
        assert_eq!(out("x = 3; if x > 5 { print \"big\"; } else if x > 2 { print \"mid\"; } else { print \"small\"; }"), vec!["mid"]);
        assert_eq!(out("d = 0; if d == 0 || 10 / d > 1 { print \"safe\"; }"), vec!["safe"]);
    }

    #[test]
    fn functions_return_and_recursion() {
        // definition after use (hoisting) + recursion
        assert_eq!(out("print fact(5); fn fact(n) { if n <= 1 { return 1; } return n * fact(n - 1); }"), vec!["120"]);
        // `def` is accepted too, and a function's locals don't leak to the caller
        assert_eq!(out("def add(a, b) { s = a + b; return s; } print add(2, 3);"), vec!["5"]);
        assert_eq!(err("def add(a, b) { local = a + b; return local; } print add(1, 2); print local;")[0].code, "E0303");
    }

    #[test]
    fn mutual_recursion() {
        let src = "
            fn is_even(n) { if n == 0 { return true; } return is_odd(n - 1); }
            fn is_odd(n) { if n == 0 { return false; } return is_even(n - 1); }
            print is_even(10); print is_odd(7);";
        assert_eq!(out(src), vec!["true", "true"]);
    }

    #[test]
    fn while_and_for_loops() {
        assert_eq!(out("i = 0; s = 0; while i < 5 { s += i; i += 1; } print s;"), vec!["10"]);
        assert_eq!(out("s = 0; for i in 0..5 { s += i; } print s;"), vec!["10"]);
        assert_eq!(out("s = 0; for i in 1..=3 { s += i; } print s;"), vec!["6"]);
    }

    #[test]
    fn break_and_continue() {
        assert_eq!(out("s = 0; for i in 0..10 { if i == 3 { break; } s += i; } print s;"), vec!["3"]);
        assert_eq!(out("s = 0; for i in 0..5 { if i % 2 == 0 { continue; } s += i; } print s;"), vec!["4"]);
    }

    #[test]
    fn runaway_loop_hits_the_fuel_budget_not_a_hang() {
        // `while true {}` would hang a naive interpreter; here it returns a Diag.
        assert_eq!(err("while true { }")[0].code, "E0307");
    }

    #[test]
    fn deep_recursion_is_a_diag_not_a_stack_overflow() {
        assert_eq!(err("fn f(n) { return f(n + 1); } print f(0);")[0].code, "E0312");
    }

    #[test]
    fn arrays_index_iterate_and_methods() {
        assert_eq!(out("a = [10, 20, 30]; print a[1];"), vec!["20"]);
        assert_eq!(out("a = [1, 2, 3]; a[0] = 9; a[1] += 5; print a[0]; print a[1];"), vec!["9", "7"]);
        assert_eq!(out("a = [1, 2, 3, 4]; print a.len(); print a.sum(); print a.min(); print a.max();"), vec!["4", "10", "1", "4"]);
        assert_eq!(out("a = []; a.push(1); a.push(2); print a.pop(); print a.len();"), vec!["2", "1"]);
        assert_eq!(out("s = 0; for x in [2, 4, 6] { s += x; } print s;"), vec!["12"]);
        assert_eq!(out("a = [1, 2, 3]; print a.contains(2); print a.contains(9);"), vec!["true", "false"]);
    }

    #[test]
    fn arrays_have_reference_semantics() {
        // pushing to an array inside a function mutates the caller's array
        let src = "fn add_one(xs) { xs.push(1); } a = [0]; add_one(a); add_one(a); print a.len();";
        assert_eq!(out(src), vec!["3"]);
    }

    #[test]
    fn array_errors_are_diagnostics() {
        assert_eq!(err("a = [1, 2]; print a[5];")[0].code, "E0315"); // out of bounds
        assert_eq!(err("a = [1, 2]; print a[-1];")[0].code, "E0315"); // negative
        assert_eq!(err("x = 3; print x[0];")[0].code, "E0313"); // index a non-array
        assert_eq!(err("a = []; print a.pop();")[0].code, "E0318"); // pop empty
        assert_eq!(err("a = [1]; a.frobnicate();")[0].code, "E0319"); // unknown method
    }

    #[test]
    fn structs_declare_construct_read_and_assign() {
        let src = "
            struct Trade { side, volume, price }
            t = Trade { side: \"buy\", volume: 0.5, price: 100.0 };
            print t.side; print t.price;
            t.price = 101.5;
            t.volume += 0.25;
            print t.price; print t.volume;";
        assert_eq!(super::run_str(src).unwrap(), vec!["buy", "100", "101.5", "0.75"]);
    }

    #[test]
    fn structs_have_reference_semantics_and_disambiguate_from_blocks() {
        // passing a struct to a function and mutating a field is visible to the caller
        let src = "
            struct Box { n }
            fn bump(b) { b.n += 1; }
            b = Box { n: 5 };
            bump(b); bump(b);
            print b.n;
            // a bare `if cond { }` is still a block, not a struct literal
            x = 3; if x > 1 { print \"ok\"; }";
        assert_eq!(super::run_str(src).unwrap(), vec!["7", "ok"]);
    }

    #[test]
    fn struct_errors_are_diagnostics() {
        assert_eq!(err("struct P { x } p = P { x: 1, y: 2 };")[0].code, "E0324"); // unknown field
        assert_eq!(err("struct P { x, y } p = P { x: 1 };")[0].code, "E0325"); // missing field
        assert_eq!(err("struct P { x } p = P { x: 1 }; print p.z;")[0].code, "E0321"); // no such field
        assert_eq!(err("x = 3; x.field = 1;")[0].code, "E0320"); // set field on non-struct
    }

    #[test]
    fn arity_and_control_flow_errors() {
        assert_eq!(err("fn f(a) { return a; } print f(1, 2);")[0].code, "E0311");
        assert_eq!(err("return 1;")[0].code, "E0304");
        assert_eq!(err("break;")[0].code, "E0305");
    }
}
