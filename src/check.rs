//! Static type checker — a pragmatic bidirectional pass that runs *before*
//! execution (the editor's Check button, RUSPY_SCHEMA.md §7/§8).
//!
//! ruspy is **gradually typed**: annotations are enforced where present, but
//! unannotated variables/params are `Ty::Dynamic` and never cause a false positive,
//! so existing untyped programs keep type-checking. What it catches statically:
//! annotation mismatches, non-`bool` conditions, undefined variables, call arity,
//! argument types, operator operand types, and returning a value of the wrong type.

use crate::ast::{Expr, ExprKind, LogicOp, Param, Program, Stmt, StmtKind, UnOp};
use crate::diagnostics::{Diag, Span};
use crate::ty::{FloatWidth, IntWidth, Ty};
use crate::value::BinOp;
use std::collections::HashMap;

/// A function's static signature.
#[derive(Clone)]
struct Sig {
    params: Vec<Ty>,
    ret: Ty,
}

/// Namespaced library packages that must be pulled in with `import <name>;` before
/// use. `tick` / `account` / `position` are *ambient* strategy context (like MQL's
/// predefined variables) and are deliberately not on this list.
const LIBRARY_PACKAGES: &[&str] = &["math", "trade", "series"];

struct Checker {
    /// Variable scopes (name → type); innermost last.
    scopes: Vec<HashMap<String, Ty>>,
    funcs: HashMap<String, Sig>,
    diags: Vec<Diag>,
    /// Package names brought in by `import <name>;` (enforced against use).
    imports: std::collections::HashSet<String>,
    /// Declared return type of the function currently being checked (if any).
    current_ret: Option<Ty>,
    in_loop: usize,
}

impl Checker {
    fn new() -> Self {
        Checker {
            scopes: vec![HashMap::new()],
            funcs: HashMap::new(),
            diags: Vec::new(),
            imports: std::collections::HashSet::new(),
            current_ret: None,
            in_loop: 0,
        }
    }

    /// Flag `pkg.…` when `pkg` is a known library package that wasn't imported and
    /// isn't shadowed by a local variable (in which case `pkg.field` is struct
    /// access, not a package). Makes `import` load-bearing.
    fn enforce_package(&mut self, base: &Expr) {
        if let ExprKind::Ident(id) = &base.node {
            if LIBRARY_PACKAGES.contains(&id.as_str())
                && self.lookup(id).is_none()
                && !self.imports.contains(id)
            {
                self.error(
                    base.span,
                    "E0410",
                    format!("package `{id}` is used but not imported — add `import {id};` at the top"),
                );
            }
        }
    }

    fn error(&mut self, span: Span, code: &'static str, msg: impl Into<String>) {
        self.diags.push(Diag::error(span, code, msg));
    }

    fn declare(&mut self, name: &str, ty: Ty) {
        self.scopes.last_mut().unwrap().insert(name.to_string(), ty);
    }
    fn lookup(&self, name: &str) -> Option<Ty> {
        for s in self.scopes.iter().rev() {
            if let Some(t) = s.get(name) {
                return Some(t.clone());
            }
        }
        None
    }

    fn check_program(&mut self, prog: &Program) {
        // Collect package imports first so use-before-import is diagnosable anywhere.
        for stmt in prog {
            if let StmtKind::Import { spec: crate::ast::ImportSpec::Package(name) } = &stmt.node {
                self.imports.insert(name.clone());
            }
        }
        // Pre-register function signatures (annotations → types; unannotated → Dynamic).
        for stmt in prog {
            if let StmtKind::Fn { name, params, ret, .. } = &stmt.node {
                self.funcs.insert(
                    name.clone(),
                    Sig {
                        params: params.iter().map(param_ty).collect(),
                        ret: ret.clone().unwrap_or(Ty::Dynamic),
                    },
                );
            }
        }
        for stmt in prog {
            self.check_stmt(stmt);
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.node {
            // Imports are collected in a pre-pass (see `check`); nothing to do here.
            StmtKind::Import { .. } => {}
            StmtKind::Fn { params, ret, body, .. } => self.check_fn(params, ret, body),
            StmtKind::Var { name, ty, value } => {
                let vt = self.synth(value);
                if let Some(ann) = ty {
                    if !vt.assignable_to(ann) {
                        self.error(value.span, "E0400", format!("cannot assign `{vt}` to `{name}: {ann}`"));
                    }
                    self.declare(name, ann.clone());
                } else {
                    self.declare(name, vt);
                }
            }
            StmtKind::Assign { name, name_span, op, value } => {
                let vt = self.synth(value);
                match self.lookup(name) {
                    Some(existing) => {
                        // compound assign must produce a compatible arithmetic result
                        let result = match op {
                            Some(o) => self.check_binop(*o, &existing, &vt, stmt.span),
                            None => vt,
                        };
                        if !result.assignable_to(&existing) && !matches!(existing, Ty::Dynamic) {
                            self.error(value.span, "E0401", format!("cannot assign `{result}` to `{name}: {existing}`"));
                        }
                    }
                    None => {
                        if op.is_some() {
                            self.error(*name_span, "E0303", format!("undefined variable `{name}`"));
                        } else {
                            self.declare(name, vt);
                        }
                    }
                }
            }
            StmtKind::IndexAssign { array, index, value, .. } => {
                let at = self.synth(array);
                self.expect_index(&at, array.span);
                let it = self.synth(index);
                if !it.assignable_to(&Ty::Int(IntWidth::Default)) {
                    self.error(index.span, "E0314", format!("array index must be `int`, found `{it}`"));
                }
                self.synth(value);
            }
            StmtKind::FieldAssign { object, value, .. } => {
                self.synth(object);
                self.synth(value);
            }
            StmtKind::Struct { .. } => {} // struct fields are gradually typed (Dynamic)
            StmtKind::Print(e) | StmtKind::Expr(e) => {
                self.synth(e);
            }
            StmtKind::Block(body) => self.check_scope(body),
            StmtKind::If { cond, then, els } => {
                self.expect_bool(cond);
                self.check_scope(then);
                if let Some(e) = els {
                    self.check_scope(e);
                }
            }
            StmtKind::While { cond, body } => {
                self.expect_bool(cond);
                self.in_loop += 1;
                self.check_scope(body);
                self.in_loop -= 1;
            }
            StmtKind::For { var, iter, body } => {
                let elem = self.for_elem_ty(iter);
                self.scopes.push(HashMap::new());
                self.declare(var, elem);
                self.in_loop += 1;
                for s in body {
                    self.check_stmt(s);
                }
                self.in_loop -= 1;
                self.scopes.pop();
            }
            StmtKind::Return(e) => {
                let rt = e.as_ref().map(|x| self.synth(x)).unwrap_or(Ty::Void);
                if let Some(expected) = self.current_ret.clone() {
                    if !rt.assignable_to(&expected) {
                        let span = e.as_ref().map(|x| x.span).unwrap_or(stmt.span);
                        self.error(span, "E0402", format!("returning `{rt}` from a `-> {expected}` function"));
                    }
                }
            }
            StmtKind::Break | StmtKind::Continue => {
                if self.in_loop == 0 {
                    let kw = if matches!(stmt.node, StmtKind::Break) { "break" } else { "continue" };
                    self.error(stmt.span, "E0403", format!("`{kw}` outside of a loop"));
                }
            }
        }
    }

    fn check_fn(&mut self, params: &[Param], ret: &Option<Ty>, body: &[Stmt]) {
        self.scopes.push(HashMap::new());
        for p in params {
            self.declare(&p.name, param_ty(p));
        }
        let prev = self.current_ret.replace(ret.clone().unwrap_or(Ty::Dynamic));
        for s in body {
            self.check_stmt(s);
        }
        self.current_ret = prev;
        self.scopes.pop();
    }

    /// Check a braced block in its own scope.
    fn check_scope(&mut self, body: &[Stmt]) {
        self.scopes.push(HashMap::new());
        for s in body {
            self.check_stmt(s);
        }
        self.scopes.pop();
    }

    /// Infer the type of an expression, recording diagnostics on the way.
    fn synth(&mut self, expr: &Expr) -> Ty {
        match &expr.node {
            ExprKind::Int(_) => Ty::Int(IntWidth::Default),
            ExprKind::Float(_) => Ty::Float(FloatWidth::Default),
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Str(_) => Ty::Str,
            ExprKind::Ident(n) => match self.lookup(n) {
                Some(t) => t,
                None => {
                    self.error(expr.span, "E0303", format!("undefined variable `{n}`"));
                    Ty::Error
                }
            },
            ExprKind::Unary(UnOp::Neg, e) => {
                let t = self.synth(e);
                if !t.is_numeric() && !matches!(t, Ty::Dynamic | Ty::Error) {
                    self.error(expr.span, "E0202", format!("cannot negate `{t}`"));
                    return Ty::Error;
                }
                t
            }
            ExprKind::Unary(UnOp::Not, e) => {
                let t = self.synth(e);
                if !matches!(t, Ty::Bool | Ty::Dynamic | Ty::Error) {
                    self.error(expr.span, "E0203", format!("`!` expects `bool`, found `{t}`"));
                }
                Ty::Bool
            }
            ExprKind::Binary(op, l, r) => {
                let lt = self.synth(l);
                let rt = self.synth(r);
                self.check_binop(*op, &lt, &rt, expr.span)
            }
            ExprKind::Logic(LogicOp::And | LogicOp::Or, l, r) => {
                self.expect_bool(l);
                self.expect_bool(r);
                Ty::Bool
            }
            ExprKind::Call(name, args) => {
                let arg_tys: Vec<(Ty, Span)> = args.iter().map(|a| (self.synth(a), a.span)).collect();
                match self.funcs.get(name).cloned() {
                    Some(sig) => {
                        if args.len() != sig.params.len() {
                            self.error(
                                expr.span,
                                "E0311",
                                format!("`{name}` expects {} argument(s), got {}", sig.params.len(), args.len()),
                            );
                        } else {
                            for ((at, asp), pt) in arg_tys.iter().zip(&sig.params) {
                                if !at.assignable_to(pt) {
                                    self.error(*asp, "E0404", format!("argument of type `{at}` is not `{pt}`"));
                                }
                            }
                        }
                        sig.ret
                    }
                    // Unknown name → assumed a host function; type is dynamic.
                    None => Ty::Dynamic,
                }
            }
            ExprKind::Member(base, _) => {
                self.enforce_package(base); // math.PI etc. must be imported
                Ty::Dynamic // package/host object fields are dynamic
            }
            ExprKind::Array(items) => {
                let mut elem = Ty::Dynamic;
                for (i, it) in items.iter().enumerate() {
                    let t = self.synth(it);
                    if i == 0 {
                        elem = t;
                    } else if !t.assignable_to(&elem) && !elem.assignable_to(&t) {
                        elem = Ty::Dynamic; // heterogeneous → dynamic element
                    }
                }
                Ty::Array(Box::new(elem))
            }
            ExprKind::Index(arr, idx) => {
                let at = self.synth(arr);
                let it = self.synth(idx);
                if !it.assignable_to(&Ty::Int(IntWidth::Default)) && !matches!(it, Ty::Dynamic | Ty::Error) {
                    self.error(idx.span, "E0314", format!("array index must be `int`, found `{it}`"));
                }
                match at {
                    Ty::Array(el) => *el,
                    Ty::Dynamic | Ty::Error => Ty::Dynamic,
                    other => {
                        self.error(arr.span, "E0313", format!("cannot index `{other}`"));
                        Ty::Error
                    }
                }
            }
            ExprKind::Method(recv, _, args) => {
                // A method on a bare identifier that isn't a local variable is a
                // package call (`math.sin`, `trade.buy`) — enforce the import; don't
                // treat the base as an undefined variable. Otherwise it's a value
                // method (array builtins) whose receiver is a real expression.
                match &recv.node {
                    ExprKind::Ident(id) if self.lookup(id).is_none() => self.enforce_package(recv),
                    _ => {
                        self.synth(recv);
                    }
                }
                for a in args {
                    self.synth(a);
                }
                Ty::Dynamic // method result types are dynamic for now
            }
            ExprKind::Range { start, end, .. } => {
                self.synth(start);
                self.synth(end);
                Ty::Dynamic
            }
            ExprKind::StructLit { fields, .. } => {
                for (_, e) in fields {
                    self.synth(e);
                }
                Ty::Dynamic // struct instances are gradually typed for now
            }
        }
    }

    /// Result type of a binary operator, reporting operand-type errors.
    fn check_binop(&mut self, op: BinOp, lt: &Ty, rt: &Ty, span: Span) -> Ty {
        use BinOp::*;
        if matches!(lt, Ty::Error) || matches!(rt, Ty::Error) {
            return Ty::Error;
        }
        match op {
            Eq | Ne => Ty::Bool,
            Lt | Le | Gt | Ge => {
                if (lt.is_numeric() && rt.is_numeric())
                    || (matches!(lt, Ty::Str) && matches!(rt, Ty::Str))
                    || matches!(lt, Ty::Dynamic)
                    || matches!(rt, Ty::Dynamic)
                {
                    Ty::Bool
                } else {
                    self.error(span, "E0204", format!("cannot compare `{lt}` and `{rt}`"));
                    Ty::Bool
                }
            }
            Add if matches!(lt, Ty::Str) && matches!(rt, Ty::Str) => Ty::Str,
            Add | Sub | Mul | Div | Rem => match lt.arith_result(rt) {
                Some(t) => t,
                None => {
                    self.error(span, "E0204", format!("operator cannot apply to `{lt}` and `{rt}`"));
                    Ty::Error
                }
            },
        }
    }

    fn expect_bool(&mut self, e: &Expr) {
        let t = self.synth(e);
        if !matches!(t, Ty::Bool | Ty::Dynamic | Ty::Error) {
            self.error(e.span, "E0201", format!("condition must be `bool`, found `{t}`"));
        }
    }

    fn expect_index(&mut self, at: &Ty, span: Span) {
        if !matches!(at, Ty::Array(_) | Ty::Dynamic | Ty::Error) {
            self.error(span, "E0313", format!("cannot index `{at}`"));
        }
    }

    fn for_elem_ty(&mut self, iter: &Expr) -> Ty {
        match &iter.node {
            ExprKind::Range { start, end, .. } => {
                self.synth(start);
                self.synth(end);
                Ty::Int(IntWidth::Default)
            }
            _ => match self.synth(iter) {
                Ty::Array(el) => *el,
                _ => Ty::Dynamic,
            },
        }
    }
}

fn param_ty(p: &Param) -> Ty {
    p.ty.clone().unwrap_or(Ty::Dynamic)
}

/// Type-check a whole program, returning all diagnostics (empty = well-typed).
pub fn check(prog: &Program) -> Vec<Diag> {
    let mut c = Checker::new();
    c.check_program(prog);
    c.diags
}

/// Convenience: lex + parse + type-check a source string, returning every diagnostic.
pub fn check_str(src: &str) -> Vec<Diag> {
    let (prog, mut diags) = crate::parser::parse_program(src);
    if !diags.iter().any(|d| d.is_error()) {
        diags.extend(check(&prog));
    }
    diags
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) {
        let d = check_str(src);
        assert!(d.is_empty(), "expected no type errors for {src:?}, got {d:?}");
    }
    fn code(src: &str) -> &'static str {
        let d = check_str(src);
        assert!(!d.is_empty(), "expected a type error for {src:?}");
        d[0].code
    }

    #[test]
    fn library_packages_must_be_imported() {
        // Imported → fine (method and constant).
        ok("import math; r = math.pow(2.0, 3.0); a = math.PI;");
        // Used without import → E0410, for both a method and a constant.
        assert_eq!(code("r = math.pow(2.0, 3.0);"), "E0410");
        assert_eq!(code("a = math.PI + 1.0;"), "E0410");
        assert_eq!(code("trade.buy(0.1);"), "E0410");
        // Ambient host objects need no import.
        ok("p = tick.bid; if p > 1.0 { print p; }");
        ok("e = account.equity; print e;");
        // A local variable named like a package shadows it (struct access, not a pkg).
        ok("math = 5; y = math;");
    }

    #[test]
    fn untyped_programs_still_pass() {
        // gradual typing: no false positives on the existing untyped style
        ok("x = 10; y = x + 5; print y;");
        ok("fn add(a, b) { return a + b; } print add(1, 2);");
        ok("price = tick.bid; if price > 100.0 { buy(0.1); }"); // host calls are dynamic
        ok("a = [1, 2, 3]; for v in a { print v; }");
    }

    #[test]
    fn annotations_are_enforced() {
        ok("x: int = 10;");
        ok("x: float = 10;"); // int widens to float
        assert_eq!(code("x: int = \"hi\";"), "E0400"); // str not assignable to int
        assert_eq!(code("x: bool = 1;"), "E0400");
        assert_eq!(code("x: int = 1.5;"), "E0400"); // float does not narrow to int
    }

    #[test]
    fn conditions_must_be_bool() {
        assert_eq!(code("if 1 { print 2; }"), "E0201");
        assert_eq!(code("while 0 { }"), "E0201");
        ok("if 1 > 0 { print 2; }");
    }

    #[test]
    fn undefined_variables_are_caught_before_running() {
        assert_eq!(code("print y;"), "E0303");
        assert_eq!(code("x += 1;"), "E0303");
    }

    #[test]
    fn typed_functions_check_args_and_returns() {
        ok("fn area(w: float, h: float) -> float { return w * h; } print area(2.0, 3.0);");
        assert_eq!(code("fn f(a: int) -> int { return a; } print f(1, 2);"), "E0311"); // arity
        assert_eq!(code("fn f(a: int) -> int { return a; } print f(\"x\");"), "E0404"); // arg type
        assert_eq!(code("fn f() -> int { return \"x\"; }"), "E0402"); // wrong return type
        assert_eq!(code("fn ok(a: bool) -> bool { return !a; } print ok(1);"), "E0404");
    }

    #[test]
    fn bad_operators_and_indexing() {
        assert_eq!(code("x = true + 1;"), "E0204");
        assert_eq!(code("x = 3; y = x[0];"), "E0313"); // index a non-array
    }
}
