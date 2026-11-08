//! Coroutines: functions containing `await` run on an explicit cursor stack instead of the Rust
//! stack, so they can suspend at an `await` and resume later.
//!
//! `await` is allowed as a whole statement (`await e;`, `x = await e;`, `x: T = await e;`,
//! `return await e;`) anywhere inside the function's blocks, `if`, `while`, `for` and `match`
//! bodies. Everything else in a coroutine runs through the normal interpreter.

use super::*;

/// Result of running a coroutine until it stops.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done(Value),
    /// Stopped at an `await`; the value is what was awaited.
    Suspended(Value),
}

// The cursors point into `Coroutine::root` (an `Rc<Vec<Stmt>>` that is never mutated and lives
// as long as the coroutine), so the raw pointers stay valid.
enum Cursor {
    Block { stmts: *const [Stmt], idx: usize },
    While { cond: *const Expr, body: *const [Stmt] },
    For { var: String, items: Vec<Value>, idx: usize, body: *const [Stmt] },
}

pub struct Coroutine {
    _root: std::rc::Rc<Vec<Stmt>>,
    frame: HashMap<String, Value>,
    stack: Vec<Cursor>,
    /// Suspended at the statement the top block cursor points at.
    pending: bool,
    name: String,
}

impl Coroutine {
    pub(super) fn new(root: std::rc::Rc<Vec<Stmt>>, frame: HashMap<String, Value>, name: &str) -> Coroutine {
        let first = Cursor::Block { stmts: root.as_slice() as *const [Stmt], idx: 0 };
        Coroutine { _root: root, frame, stack: vec![first], pending: false, name: name.to_string() }
    }
}

/// The awaited expression if `s` is a statement whose value is `await e`.
fn leaf_await(s: &Stmt) -> Option<&Expr> {
    let e = match &s.node {
        StmtKind::Expr(e) => e,
        StmtKind::Assign { value, .. } | StmtKind::Var { value, .. } => value,
        StmtKind::Return(Some(e)) => e,
        _ => return None,
    };
    match &e.node {
        ExprKind::Await(inner) => Some(inner),
        _ => None,
    }
}

fn stmt_has_await(s: &Stmt) -> bool {
    if leaf_await(s).is_some() {
        return true;
    }
    match &s.node {
        StmtKind::If { then, els, .. } => contains_await(then) || els.as_ref().is_some_and(|e| contains_await(e)),
        StmtKind::While { body, .. } | StmtKind::For { body, .. } => contains_await(body),
        StmtKind::Block(b) => contains_await(b),
        StmtKind::Match { arms, .. } => arms.iter().any(|a| contains_await(&a.body)),
        _ => false,
    }
}

/// Does a function body contain a (statement-level) `await`? Nested `fn`s do not count.
pub(super) fn contains_await(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_has_await)
}

impl Interpreter {
    /// Runs `co` (with `resume` as the value of the `await` it is suspended at) until it
    /// finishes or hits the next `await`.
    pub(super) fn co_run(&mut self, co: &mut Coroutine, resume: Option<Value>, host: &mut dyn RuspyHost) -> Result<Outcome, Diag> {
        self.frames.push(std::mem::take(&mut co.frame));
        let saved = std::mem::take(&mut self.cs);
        self.cs = crate::callstack::CallStack::default();
        self.cs.adopt_source(&saved);
        self.cs.push(&co.name);
        let mut r = self.co_loop(co, resume, host);
        if let Err(d) = &mut r {
            self.cs.attach(d);
        }
        self.cs = saved;
        co.frame = self.frames.pop().unwrap_or_default();
        r
    }

    fn co_loop(&mut self, co: &mut Coroutine, resume: Option<Value>, host: &mut dyn RuspyHost) -> Result<Outcome, Diag> {
        let sp0 = Span::new(0, 0);
        if co.pending {
            co.pending = false;
            let v = resume.unwrap_or(Value::Null);
            let Some(Cursor::Block { stmts, idx }) = co.stack.last_mut() else {
                return Err(Diag::error(sp0, "E0362", "internal: coroutine cursor lost"));
            };
            // SAFETY: see `Cursor`.
            let stmt = unsafe { &(**stmts)[*idx] };
            *idx += 1;
            match &stmt.node {
                StmtKind::Var { name, value, .. } => {
                    let _ = value;
                    self.declare(name, v);
                }
                StmtKind::Assign { name, name_span, op, .. } => {
                    let new = match op {
                        Some(o) => {
                            let cur = self.lookup(name).cloned().ok_or_else(|| undefined(name, *name_span))?;
                            value::binop(*o, &cur, &v, stmt.span)?
                        }
                        None => v,
                    };
                    if !self.assign_existing(name, new.clone()) {
                        if op.is_some() {
                            return Err(undefined(name, *name_span));
                        }
                        self.declare(name, new);
                    }
                }
                StmtKind::Return(_) => return Ok(Outcome::Done(v)),
                _ => {}
            }
        }
        loop {
            self.tick(sp0)?;
            // Copy what the top cursor needs, then release the borrow of `co.stack`.
            enum Next {
                Pop,
                Stmt(*const Stmt),
                LoopTest(*const Expr, *const [Stmt]),
                ForNext(*const [Stmt]),
            }
            let next = match co.stack.last_mut() {
                None => return Ok(Outcome::Done(Value::Void)),
                Some(Cursor::Block { stmts, idx }) => {
                    // SAFETY: see `Cursor`.
                    let slice = unsafe { &**stmts };
                    if *idx >= slice.len() {
                        Next::Pop
                    } else {
                        Next::Stmt(&slice[*idx] as *const Stmt)
                    }
                }
                Some(Cursor::While { cond, body }) => Next::LoopTest(*cond, *body),
                Some(Cursor::For { items, idx, body, .. }) => {
                    if *idx < items.len() {
                        Next::ForNext(*body)
                    } else {
                        Next::Pop
                    }
                }
            };
            match next {
                Next::Pop => {
                    co.stack.pop();
                }
                Next::LoopTest(cond, body) => {
                    // SAFETY: see `Cursor`.
                    let cond = unsafe { &*cond };
                    self.tick(cond.span)?;
                    if self.eval(cond, host)?.as_bool(cond.span)? {
                        co.stack.push(Cursor::Block { stmts: body, idx: 0 });
                    } else {
                        co.stack.pop();
                    }
                }
                Next::ForNext(body) => {
                    if let Some(Cursor::For { var, items, idx, .. }) = co.stack.last_mut() {
                        let v = items[*idx].clone();
                        *idx += 1;
                        let var = var.clone();
                        self.declare(&var, v);
                    }
                    co.stack.push(Cursor::Block { stmts: body, idx: 0 });
                }
                Next::Stmt(sp) => {
                    // SAFETY: see `Cursor`.
                    let stmt: &Stmt = unsafe { &*sp };
                    if let Some(inner) = leaf_await(stmt) {
                        let v = self.eval(inner, host)?;
                        co.pending = true; // the cursor stays on this statement
                        return Ok(Outcome::Suspended(v));
                    }
                    if let Some(Cursor::Block { idx, .. }) = co.stack.last_mut() {
                        *idx += 1;
                    }
                    if !stmt_has_await(stmt) {
                        match self.exec(stmt, host)? {
                            Flow::Normal => {}
                            Flow::Return(v) => return Ok(Outcome::Done(v)),
                            Flow::Break => unwind(co, true, stmt.span)?,
                            Flow::Continue => unwind(co, false, stmt.span)?,
                        }
                        continue;
                    }
                    match &stmt.node {
                        StmtKind::If { cond, then, els } => {
                            let body = if self.eval(cond, host)?.as_bool(cond.span)? { Some(then) } else { els.as_ref() };
                            if let Some(b) = body {
                                co.stack.push(Cursor::Block { stmts: b.as_slice() as *const [Stmt], idx: 0 });
                            }
                        }
                        StmtKind::Block(b) => co.stack.push(Cursor::Block { stmts: b.as_slice() as *const [Stmt], idx: 0 }),
                        StmtKind::While { cond, body } => {
                            co.stack.push(Cursor::While { cond: cond as *const Expr, body: body.as_slice() as *const [Stmt] })
                        }
                        StmtKind::For { var: _, iter, body } => {
                            let items = self.for_items(iter, host)?;
                            let StmtKind::For { var, .. } = &stmt.node else { unreachable!() };
                            co.stack.push(Cursor::For { var: var.clone(), items, idx: 0, body: body.as_slice() as *const [Stmt] });
                        }
                        StmtKind::Match { scrut, arms } => {
                            let v = self.eval(scrut, host)?;
                            'arms: for arm in arms {
                                let mut hit = arm.wild;
                                for p in &arm.pats {
                                    if hit {
                                        break;
                                    }
                                    let pv = self.eval(p, host)?;
                                    hit = value::values_equal(&v, &pv);
                                }
                                if hit {
                                    co.stack.push(Cursor::Block { stmts: arm.body.as_slice() as *const [Stmt], idx: 0 });
                                    break 'arms;
                                }
                            }
                        }
                        _ => return Err(Diag::error(stmt.span, "E0362", "internal: unsupported statement around `await`")),
                    }
                }
            }
        }
    }
}

/// `break` (`is_break`) pops through the nearest loop; `continue` pops down to it.
fn unwind(co: &mut Coroutine, is_break: bool, span: Span) -> Result<(), Diag> {
    while let Some(top) = co.stack.last() {
        if matches!(top, Cursor::While { .. } | Cursor::For { .. }) {
            if is_break {
                co.stack.pop();
            }
            return Ok(());
        }
        co.stack.pop();
    }
    let (code, what) = if is_break { ("E0305", "break") } else { ("E0306", "continue") };
    Err(Diag::error(span, code, format!("`{what}` outside of a loop")))
}
