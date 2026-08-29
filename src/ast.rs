//! The abstract syntax tree — a proper `Expr` / `Stmt` split with spans.
//!
//! Replaces the original single `ASTNode` enum that stored operators as raw
//! `Token`s (which forced `unreachable!()` arms in the interpreter). Operators are
//! now typed (`BinOp` / `UnOp` / `LogicOp`), every node carries a `Span`, and
//! statements are distinct from expressions.

use crate::diagnostics::{Span, Spanned};
use crate::ty::Ty;
use crate::value::BinOp;

/// An expression, tagged with its source span.
pub type Expr = Spanned<ExprKind>;

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    /// A variable reference.
    Ident(String),
    /// Unary `-x` / `!x`.
    Unary(UnOp, Box<Expr>),
    /// Arithmetic or comparison (`+ - * / %  == != < <= > >=`).
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// Short-circuiting `&&` / `||` (kept separate — evaluation order matters).
    Logic(LogicOp, Box<Expr>, Box<Expr>),
    /// A call `name(args...)` — resolved by the host today (M6 adds user functions).
    Call(String, Vec<Expr>),
    /// Member access `obj.field` — resolved by the host.
    Member(Box<Expr>, String),
    /// A range `a..b` (exclusive) or `a..=b` (inclusive), used by `for`.
    Range { start: Box<Expr>, end: Box<Expr>, inclusive: bool },
    /// An array literal `[a, b, c]`.
    Array(Vec<Expr>),
    /// Indexing `arr[i]`.
    Index(Box<Expr>, Box<Expr>),
    /// A method call `recv.name(args...)` (array builtins today, host later).
    Method(Box<Expr>, String, Vec<Expr>),
    /// A struct literal `Name { field: expr, ... }`.
    StructLit { name: String, fields: Vec<(String, Expr)> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicOp {
    And,
    Or,
}

/// A statement, tagged with its source span.
pub type Stmt = Spanned<StmtKind>;

/// What an `import` brings in.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportSpec {
    /// `import math;` — a built-in namespaced package (math / trade / series).
    Package(String),
    /// `import "path.ruspy";` — a user source file, resolved relative to the importer.
    File(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// `import math;` or `import "utils.ruspy";`. File imports are spliced away by
    /// the loader; package imports remain so the checker can enforce them.
    Import { spec: ImportSpec },
    /// Variable binding: `x = expr;` or `x: T = expr;` (M5 splits `let` from assign).
    Var { name: String, ty: Option<Ty>, value: Expr },
    /// Reassignment, optionally compound: `x = e`, `x += e`, `x *= e`, …
    Assign { name: String, name_span: Span, op: Option<BinOp>, value: Expr },
    /// Index assignment `arr[i] = e` (optionally compound).
    IndexAssign { array: Expr, index: Expr, op: Option<BinOp>, value: Expr },
    /// Field assignment `obj.field = e` (optionally compound).
    FieldAssign { object: Expr, field: String, op: Option<BinOp>, value: Expr },
    /// A struct declaration `struct Name { a, b, c }`.
    Struct { name: String, fields: Vec<String> },
    /// `print expr;`
    Print(Expr),
    /// A bare expression used as a statement.
    Expr(Expr),
    /// `{ stmts... }`
    Block(Vec<Stmt>),
    /// `if cond { then } [else { els }]` — `else if` desugars to a nested `If`.
    If { cond: Expr, then: Vec<Stmt>, els: Option<Vec<Stmt>> },
    /// A function definition: `fn name(params) [-> ret] { body }` (also `def`).
    /// Each param may carry an optional type annotation; unannotated params are
    /// gradually typed (`Ty::Dynamic`).
    Fn { name: String, params: Vec<Param>, ret: Option<Ty>, body: Vec<Stmt> },
    /// `return expr;` / `return;`
    Return(Option<Expr>),
    /// `while cond { body }`
    While { cond: Expr, body: Vec<Stmt> },
    /// `for var in iter { body }` — `iter` is a range (`a..b`) or an array (M8).
    For { var: String, iter: Expr, body: Vec<Stmt> },
    /// `break;`
    Break,
    /// `continue;`
    Continue,
}

/// A function parameter with an optional type annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Option<Ty>,
    pub span: Span,
}

/// A whole parsed program.
pub type Program = Vec<Stmt>;
