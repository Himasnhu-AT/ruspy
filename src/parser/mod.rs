//! Parser — precedence-climbing recursive descent producing the `Expr`/`Stmt` AST.
//!
//! Rewrite of the original token-as-operator parser. Precedence (loosest → tightest):
//! `||` · `&&` · comparison (non-associative) · `+ -` · `* / %` · unary `- !` ·
//! postfix `() .` · primary. On error it records a `Diag` and recovers to the next
//! `;`/`}` so a whole file's errors surface at once.

use crate::ast::{Expr, ExprKind, LogicOp, Param, Program, Stmt, StmtKind, UnOp};
use crate::diagnostics::{Diag, Span, Spanned};
use crate::lexer::Token;
use crate::ty::Ty;
use crate::value::BinOp;

pub struct Parser {
    tokens: Vec<Spanned<Token>>,
    pos: usize,
    /// Declared struct names, pre-scanned so `Name { .. }` disambiguates from a block.
    struct_names: std::collections::HashSet<String>,
}

type PResult<T> = Result<T, Diag>;

impl Parser {
    pub fn new(tokens: Vec<Spanned<Token>>) -> Self {
        // Pre-scan `struct <Ident>` so a struct literal `Point { x: 1 }` is not mistaken
        // for a block; a bare `if cond { }` (cond not a struct name) stays a block.
        let mut struct_names = std::collections::HashSet::new();
        for w in tokens.windows(2) {
            if matches!(w[0].node, Token::Struct) {
                if let Token::Identifier(n) = &w[1].node {
                    struct_names.insert(n.clone());
                }
            }
        }
        Parser { tokens, pos: 0, struct_names }
    }

    // ── cursor ──────────────────────────────────────────────────────────────
    fn cur(&self) -> &Token {
        &self.tokens[self.pos].node
    }
    fn cur_span(&self) -> Span {
        self.tokens[self.pos].span
    }
    fn peek(&self, k: usize) -> &Token {
        let i = (self.pos + k).min(self.tokens.len() - 1);
        &self.tokens[i].node
    }
    fn at_eof(&self) -> bool {
        matches!(self.cur(), Token::Eof)
    }
    fn bump(&mut self) -> Spanned<Token> {
        let t = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }
    /// Same-discriminant check (ignores literal payloads).
    fn is(&self, t: &Token) -> bool {
        std::mem::discriminant(self.cur()) == std::mem::discriminant(t)
    }
    fn eat(&mut self, want: Token) -> PResult<Span> {
        if self.is(&want) {
            Ok(self.bump().span)
        } else {
            Err(Diag::error(
                self.cur_span(),
                "E0100",
                format!("expected `{}`, found `{}`", tok_desc(&want), tok_desc(self.cur())),
            ))
        }
    }

    // ── entry ───────────────────────────────────────────────────────────────
    /// Parse a whole program, collecting diagnostics and recovering between statements.
    pub fn parse(&mut self) -> (Program, Vec<Diag>) {
        let mut stmts = Vec::new();
        let mut diags = Vec::new();
        while !self.at_eof() {
            let before = self.pos;
            match self.statement() {
                Ok(s) => stmts.push(s),
                Err(d) => {
                    diags.push(d);
                    self.recover();
                }
            }
            // Guarantee forward progress. A stray top-level `}` makes `statement()`
            // fail without advancing and `recover()` return on the `}` without
            // consuming it — which would spin forever, pushing a diagnostic every
            // pass until it OOMs. If nothing moved, consume one token. (`bump` is a
            // no-op at EOF, so this can't run past the end.)
            if self.pos == before {
                self.bump();
            }
        }
        (stmts, diags)
    }

    /// Skip tokens until just past the next `;` or the end of the current `{...}`.
    fn recover(&mut self) {
        while !self.at_eof() {
            match self.cur() {
                Token::Semicolon => {
                    self.bump();
                    return;
                }
                Token::RBrace => return,
                _ => {
                    self.bump();
                }
            }
        }
    }

    // ── statements ──────────────────────────────────────────────────────────
    fn statement(&mut self) -> PResult<Stmt> {
        match self.cur() {
            Token::Import => self.import_stmt(),
            Token::Print => self.print_stmt(),
            Token::If => self.if_stmt(),
            Token::Fn | Token::Def => self.fn_decl(),
            Token::Struct => self.struct_decl(),
            Token::Return => self.return_stmt(),
            Token::While => self.while_stmt(),
            Token::For => self.for_stmt(),
            Token::Break => {
                let sp = self.bump().span;
                let end = self.eat(Token::Semicolon)?;
                Ok(Spanned::new(StmtKind::Break, sp.to(end)))
            }
            Token::Continue => {
                let sp = self.bump().span;
                let end = self.eat(Token::Semicolon)?;
                Ok(Spanned::new(StmtKind::Continue, sp.to(end)))
            }
            Token::LBrace => {
                let start = self.cur_span();
                let body = self.block()?;
                Ok(Spanned::new(StmtKind::Block(body), start.to(self.prev_span())))
            }
            // Typed declaration is the one form that needs `ident :` lookahead;
            // everything else parses an expression and checks for an assignment.
            Token::Identifier(_) if matches!(self.peek(1), Token::Colon) => self.typed_var(),
            _ => self.expr_or_assign(),
        }
    }

    /// Parse an expression, then decide: assignment (`= += …` to an lvalue) or a bare
    /// expression statement. Unifies `x = e`, `x += e`, and `arr[i] = e`.
    fn expr_or_assign(&mut self) -> PResult<Stmt> {
        let lhs = self.expression()?;
        let op = match self.cur() {
            Token::Assign => None,
            Token::PlusEq => Some(BinOp::Add),
            Token::MinusEq => Some(BinOp::Sub),
            Token::StarEq => Some(BinOp::Mul),
            Token::SlashEq => Some(BinOp::Div),
            Token::PercentEq => Some(BinOp::Rem),
            _ => {
                // plain expression statement
                let start = lhs.span;
                let end = self.eat(Token::Semicolon)?;
                return Ok(Spanned::new(StmtKind::Expr(lhs), start.to(end)));
            }
        };
        self.bump(); // the assignment operator
        let value = self.expression()?;
        let end = self.eat(Token::Semicolon)?;
        let span = lhs.span.to(end);
        match lhs.node {
            ExprKind::Ident(name) => Ok(Spanned::new(StmtKind::Assign { name, name_span: lhs.span, op, value }, span)),
            ExprKind::Index(array, index) => {
                Ok(Spanned::new(StmtKind::IndexAssign { array: *array, index: *index, op, value }, span))
            }
            ExprKind::Member(object, field) => {
                Ok(Spanned::new(StmtKind::FieldAssign { object: *object, field, op, value }, span))
            }
            _ => Err(Diag::error(lhs.span, "E0107", "invalid assignment target")
                .with_help("assign to a variable, array element, or struct field")),
        }
    }

    fn struct_decl(&mut self) -> PResult<Stmt> {
        let start = self.eat(Token::Struct)?;
        let (name, _) = self.ident_name()?;
        self.eat(Token::LBrace)?;
        let mut fields = Vec::new();
        while !matches!(self.cur(), Token::RBrace | Token::Eof) {
            let (f, fsp) = self.ident_name()?;
            if fields.contains(&f) {
                return Err(Diag::error(fsp, "E0108", format!("duplicate field `{f}`")));
            }
            fields.push(f);
            // fields separated by `,` or `;` (both accepted), trailing allowed
            if matches!(self.cur(), Token::Comma | Token::Semicolon) {
                self.bump();
            }
        }
        let end = self.eat(Token::RBrace)?;
        Ok(Spanned::new(StmtKind::Struct { name, fields }, start.to(end)))
    }

    fn print_stmt(&mut self) -> PResult<Stmt> {
        let start = self.eat(Token::Print)?;
        let e = self.expression()?;
        let end = self.eat(Token::Semicolon)?;
        Ok(Spanned::new(StmtKind::Print(e), start.to(end)))
    }

    /// `import math;` (a built-in package) or `import "utils.ruspy";` (a user file).
    fn import_stmt(&mut self) -> PResult<Stmt> {
        let start = self.eat(Token::Import)?;
        let spec = match self.cur().clone() {
            Token::StringLiteral(path) => {
                self.bump();
                crate::ast::ImportSpec::File(path)
            }
            Token::Identifier(name) => {
                self.bump();
                crate::ast::ImportSpec::Package(name)
            }
            _ => {
                return Err(Diag::error(
                    self.cur_span(),
                    "E0110",
                    "expected a package name or a \"file.ruspy\" string after `import`",
                ));
            }
        };
        let end = self.eat(Token::Semicolon)?;
        Ok(Spanned::new(StmtKind::Import { spec }, start.to(end)))
    }

    fn ident_name(&mut self) -> PResult<(String, Span)> {
        match self.cur().clone() {
            Token::Identifier(n) => {
                let sp = self.bump().span;
                Ok((n, sp))
            }
            _ => Err(Diag::error(self.cur_span(), "E0101", "expected an identifier")),
        }
    }

    fn typed_var(&mut self) -> PResult<Stmt> {
        let (name, start) = self.ident_name()?;
        self.eat(Token::Colon)?;
        let ty = self.parse_type()?;
        self.eat(Token::Assign)?;
        let value = self.expression()?;
        let end = self.eat(Token::Semicolon)?;
        Ok(Spanned::new(StmtKind::Var { name, ty: Some(ty), value }, start.to(end)))
    }



    fn block(&mut self) -> PResult<Vec<Stmt>> {
        self.eat(Token::LBrace)?;
        let mut body = Vec::new();
        while !matches!(self.cur(), Token::RBrace | Token::Eof) {
            body.push(self.statement()?);
        }
        self.eat(Token::RBrace)?;
        Ok(body)
    }

    fn if_stmt(&mut self) -> PResult<Stmt> {
        let start = self.eat(Token::If)?;
        let cond = self.expression()?;
        let then = self.block()?;
        let els = if matches!(self.cur(), Token::Else) {
            self.bump();
            if matches!(self.cur(), Token::If) {
                // `else if` → a nested If as the single else-statement
                Some(vec![self.if_stmt()?])
            } else {
                Some(self.block()?)
            }
        } else {
            None
        };
        let end = self.prev_span();
        Ok(Spanned::new(StmtKind::If { cond, then, els }, start.to(end)))
    }

    fn fn_decl(&mut self) -> PResult<Stmt> {
        let start = self.bump().span; // `fn` or `def`
        let (name, _) = self.ident_name()?;
        self.eat(Token::LParen)?;
        let mut params: Vec<Param> = Vec::new();
        if !matches!(self.cur(), Token::RParen) {
            let p = self.param()?;
            self.reject_dup_param(&mut params, p)?;
            while matches!(self.cur(), Token::Comma) {
                self.bump();
                if matches!(self.cur(), Token::RParen) {
                    break;
                }
                let p = self.param()?;
                self.reject_dup_param(&mut params, p)?;
            }
        }
        self.eat(Token::RParen)?;
        // optional return type: `-> Ty`
        let ret = if matches!(self.cur(), Token::Arrow) {
            self.bump();
            Some(self.parse_type()?)
        } else {
            None
        };
        let body = self.block()?;
        let end = self.prev_span();
        Ok(Spanned::new(StmtKind::Fn { name, params, ret, body }, start.to(end)))
    }

    /// A parameter `name` or `name: Ty`.
    fn param(&mut self) -> PResult<Param> {
        let (name, span) = self.ident_name()?;
        let ty = if matches!(self.cur(), Token::Colon) {
            self.bump();
            Some(self.parse_type()?)
        } else {
            None
        };
        Ok(Param { name, ty, span })
    }

    fn reject_dup_param(&self, params: &mut Vec<Param>, p: Param) -> PResult<()> {
        if params.iter().any(|x| x.name == p.name) {
            return Err(Diag::error(p.span, "E0106", format!("duplicate parameter `{}`", p.name)));
        }
        params.push(p);
        Ok(())
    }

    fn return_stmt(&mut self) -> PResult<Stmt> {
        let start = self.eat(Token::Return)?;
        if matches!(self.cur(), Token::Semicolon) {
            let end = self.bump().span;
            return Ok(Spanned::new(StmtKind::Return(None), start.to(end)));
        }
        let e = self.expression()?;
        let end = self.eat(Token::Semicolon)?;
        Ok(Spanned::new(StmtKind::Return(Some(e)), start.to(end)))
    }

    fn while_stmt(&mut self) -> PResult<Stmt> {
        let start = self.eat(Token::While)?;
        let cond = self.expression()?;
        let body = self.block()?;
        Ok(Spanned::new(StmtKind::While { cond, body }, start.to(self.prev_span())))
    }

    fn for_stmt(&mut self) -> PResult<Stmt> {
        let start = self.eat(Token::For)?;
        let (var, _) = self.ident_name()?;
        self.eat(Token::In)?;
        let iter = self.expression()?;
        let body = self.block()?;
        Ok(Spanned::new(StmtKind::For { var, iter, body }, start.to(self.prev_span())))
    }

    fn parse_type(&mut self) -> PResult<Ty> {
        if let Some(ty) = Ty::from_token(self.cur()) {
            self.bump();
            Ok(ty)
        } else {
            Err(Diag::error(self.cur_span(), "E0102", format!("expected a type, found `{}`", tok_desc(self.cur()))))
        }
    }

    // ── expressions (precedence climbing) ────────────────────────────────────
    fn expression(&mut self) -> PResult<Expr> {
        self.range_expr()
    }

    /// Ranges bind loosest so `for i in 0..n` and `a..b` read naturally.
    fn range_expr(&mut self) -> PResult<Expr> {
        let lhs = self.or_expr()?;
        let inclusive = match self.cur() {
            Token::DotDot => false,
            Token::DotDotEq => true,
            _ => return Ok(lhs),
        };
        self.bump();
        let rhs = self.or_expr()?;
        let span = lhs.span.to(rhs.span);
        Ok(Spanned::new(ExprKind::Range { start: Box::new(lhs), end: Box::new(rhs), inclusive }, span))
    }

    fn or_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.and_expr()?;
        while matches!(self.cur(), Token::PipePipe) {
            self.bump();
            let rhs = self.and_expr()?;
            let span = lhs.span.to(rhs.span);
            lhs = Spanned::new(ExprKind::Logic(LogicOp::Or, Box::new(lhs), Box::new(rhs)), span);
        }
        Ok(lhs)
    }

    fn and_expr(&mut self) -> PResult<Expr> {
        let mut lhs = self.comparison()?;
        while matches!(self.cur(), Token::AmpAmp) {
            self.bump();
            let rhs = self.comparison()?;
            let span = lhs.span.to(rhs.span);
            lhs = Spanned::new(ExprKind::Logic(LogicOp::And, Box::new(lhs), Box::new(rhs)), span);
        }
        Ok(lhs)
    }

    fn comparison(&mut self) -> PResult<Expr> {
        let lhs = self.additive()?;
        if let Some(op) = cmp_op(self.cur()) {
            self.bump();
            let rhs = self.additive()?;
            let span = lhs.span.to(rhs.span);
            // Non-associative: `a < b < c` is a parse error.
            if cmp_op(self.cur()).is_some() {
                return Err(Diag::error(self.cur_span(), "E0103", "chained comparison is not allowed")
                    .with_help("parenthesize, e.g. `(a < b) && (b < c)`"));
            }
            Ok(Spanned::new(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), span))
        } else {
            Ok(lhs)
        }
    }

    fn additive(&mut self) -> PResult<Expr> {
        let mut lhs = self.multiplicative()?;
        while let Some(op) = add_op(self.cur()) {
            self.bump();
            let rhs = self.multiplicative()?;
            let span = lhs.span.to(rhs.span);
            lhs = Spanned::new(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), span);
        }
        Ok(lhs)
    }

    fn multiplicative(&mut self) -> PResult<Expr> {
        let mut lhs = self.unary()?;
        while let Some(op) = mul_op(self.cur()) {
            self.bump();
            let rhs = self.unary()?;
            let span = lhs.span.to(rhs.span);
            lhs = Spanned::new(ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), span);
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> PResult<Expr> {
        match self.cur() {
            Token::Minus => {
                let start = self.bump().span;
                let e = self.unary()?;
                let span = start.to(e.span);
                Ok(Spanned::new(ExprKind::Unary(UnOp::Neg, Box::new(e)), span))
            }
            Token::Bang => {
                let start = self.bump().span;
                let e = self.unary()?;
                let span = start.to(e.span);
                Ok(Spanned::new(ExprKind::Unary(UnOp::Not, Box::new(e)), span))
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut node = self.primary()?;
        loop {
            match self.cur() {
                Token::LParen => {
                    // Calls only on a bare identifier (free functions / host calls).
                    let name = match &node.node {
                        ExprKind::Ident(n) => n.clone(),
                        _ => return Err(Diag::error(node.span, "E0104", "only a named function can be called")),
                    };
                    let args = self.arg_list()?;
                    node = Spanned::new(ExprKind::Call(name, args), node.span.to(self.prev_span()));
                }
                Token::LBracket => {
                    self.bump();
                    let index = self.expression()?;
                    let end = self.eat(Token::RBracket)?;
                    let nsp = node.span;
                    node = Spanned::new(ExprKind::Index(Box::new(node), Box::new(index)), nsp.to(end));
                }
                Token::Dot => {
                    self.bump();
                    let (member, msp) = self.ident_name()?;
                    let nsp = node.span;
                    if matches!(self.cur(), Token::LParen) {
                        // method call `recv.name(args...)`
                        let args = self.arg_list()?;
                        node = Spanned::new(ExprKind::Method(Box::new(node), member, args), nsp.to(self.prev_span()));
                    } else {
                        node = Spanned::new(ExprKind::Member(Box::new(node), member), nsp.to(msp));
                    }
                }
                _ => break,
            }
        }
        Ok(node)
    }

    /// Parse a struct literal `Name { field: expr, ... }` (the name is already known).
    fn struct_literal(&mut self, name: String, start: Span) -> PResult<Expr> {
        self.bump(); // the name
        self.eat(Token::LBrace)?;
        let mut fields = Vec::new();
        while !matches!(self.cur(), Token::RBrace | Token::Eof) {
            let (f, fsp) = self.ident_name()?;
            if fields.iter().any(|(n, _): &(String, Expr)| n == &f) {
                return Err(Diag::error(fsp, "E0108", format!("duplicate field `{f}`")));
            }
            self.eat(Token::Colon)?;
            let value = self.expression()?;
            fields.push((f, value));
            if matches!(self.cur(), Token::Comma) {
                self.bump();
            }
        }
        let end = self.eat(Token::RBrace)?;
        Ok(Spanned::new(ExprKind::StructLit { name, fields }, start.to(end)))
    }

    /// Parse `( a, b, c, )` — a parenthesized, comma-separated, trailing-comma-ok list.
    fn arg_list(&mut self) -> PResult<Vec<Expr>> {
        self.eat(Token::LParen)?;
        let mut args = Vec::new();
        if !matches!(self.cur(), Token::RParen) {
            args.push(self.expression()?);
            while matches!(self.cur(), Token::Comma) {
                self.bump();
                if matches!(self.cur(), Token::RParen) {
                    break;
                }
                args.push(self.expression()?);
            }
        }
        self.eat(Token::RParen)?;
        Ok(args)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let sp = self.cur_span();
        match self.cur().clone() {
            Token::Number(v) => {
                self.bump();
                Ok(Spanned::new(ExprKind::Int(v), sp))
            }
            Token::FloatLiteral(v) => {
                self.bump();
                Ok(Spanned::new(ExprKind::Float(v), sp))
            }
            Token::StringLiteral(s) => {
                self.bump();
                Ok(Spanned::new(ExprKind::Str(s), sp))
            }
            Token::True => {
                self.bump();
                Ok(Spanned::new(ExprKind::Bool(true), sp))
            }
            Token::False => {
                self.bump();
                Ok(Spanned::new(ExprKind::Bool(false), sp))
            }
            Token::Identifier(n) => {
                // `Name { ... }` is a struct literal only when `Name` was declared as a
                // struct; otherwise it's a plain identifier (and any following `{` is a
                // block, e.g. an `if cond { }` header).
                if self.struct_names.contains(&n) && matches!(self.peek(1), Token::LBrace) {
                    return self.struct_literal(n, sp);
                }
                self.bump();
                Ok(Spanned::new(ExprKind::Ident(n), sp))
            }
            Token::LParen => {
                self.bump();
                let e = self.expression()?;
                self.eat(Token::RParen)?;
                Ok(e)
            }
            Token::LBracket => {
                let start = self.bump().span;
                let mut items = Vec::new();
                if !matches!(self.cur(), Token::RBracket) {
                    items.push(self.expression()?);
                    while matches!(self.cur(), Token::Comma) {
                        self.bump();
                        if matches!(self.cur(), Token::RBracket) {
                            break;
                        }
                        items.push(self.expression()?);
                    }
                }
                let end = self.eat(Token::RBracket)?;
                Ok(Spanned::new(ExprKind::Array(items), start.to(end)))
            }
            other => Err(Diag::error(sp, "E0105", format!("unexpected `{}` in expression", tok_desc(&other)))),
        }
    }

    /// Span of the token just consumed (for end-of-statement spans).
    fn prev_span(&self) -> Span {
        let i = self.pos.saturating_sub(1);
        self.tokens[i].span
    }
}

/// Public convenience: lex + parse, returning the program and *all* diagnostics.
pub fn parse_program(src: &str) -> (Program, Vec<Diag>) {
    let (tokens, mut diags) = crate::lexer::tokenize(src);
    let (prog, pdiags) = Parser::new(tokens).parse();
    diags.extend(pdiags);
    (prog, diags)
}

// ── token → operator helpers ─────────────────────────────────────────────────
fn cmp_op(t: &Token) -> Option<BinOp> {
    Some(match t {
        Token::Eq => BinOp::Eq,
        Token::NotEq => BinOp::Ne,
        Token::Lt => BinOp::Lt,
        Token::LtEq => BinOp::Le,
        Token::Gt => BinOp::Gt,
        Token::GtEq => BinOp::Ge,
        _ => return None,
    })
}
fn add_op(t: &Token) -> Option<BinOp> {
    Some(match t {
        Token::Plus => BinOp::Add,
        Token::Minus => BinOp::Sub,
        _ => return None,
    })
}
fn mul_op(t: &Token) -> Option<BinOp> {
    Some(match t {
        Token::Asterisk => BinOp::Mul,
        Token::Slash => BinOp::Div,
        Token::Percent => BinOp::Rem,
        _ => return None,
    })
}

/// A human-readable description of a token for error messages.
fn tok_desc(t: &Token) -> String {
    match t {
        Token::Identifier(n) => format!("identifier `{n}`"),
        Token::Number(v) => v.to_string(),
        Token::FloatLiteral(v) => v.to_string(),
        Token::StringLiteral(_) => "string".into(),
        Token::Semicolon => ";".into(),
        Token::Colon => ":".into(),
        Token::Assign => "=".into(),
        Token::LParen => "(".into(),
        Token::RParen => ")".into(),
        Token::LBrace => "{".into(),
        Token::RBrace => "}".into(),
        Token::Eof => "end of input".into(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Program {
        let (prog, diags) = parse_program(src);
        assert!(diags.is_empty(), "unexpected diagnostics for {src:?}: {diags:?}");
        prog
    }

    #[test]
    fn precedence_and_associativity() {
        // 1 + 2 * 3  →  1 + (2 * 3)
        let p = parse_ok("x = 1 + 2 * 3;");
        if let StmtKind::Assign { value, .. } = &p[0].node {
            if let ExprKind::Binary(BinOp::Add, _, rhs) = &value.node {
                assert!(matches!(rhs.node, ExprKind::Binary(BinOp::Mul, _, _)));
            } else {
                panic!("expected top-level +");
            }
        } else {
            panic!("expected assignment");
        }
    }

    #[test]
    fn logical_and_unary_and_percent() {
        parse_ok("ok = !done && a || b;");
        parse_ok("y = -x + 5 % 2;");
        parse_ok("t = true; f = false;");
    }

    #[test]
    fn else_if_chain_parses() {
        let p = parse_ok("if x > 2 { print 1; } else if x > 1 { print 2; } else { print 3; }");
        assert!(matches!(p[0].node, StmtKind::If { .. }));
    }

    #[test]
    fn compound_assignment() {
        let p = parse_ok("x += 1;");
        assert!(matches!(p[0].node, StmtKind::Assign { op: Some(BinOp::Add), .. }));
    }

    #[test]
    fn chained_comparison_is_rejected() {
        let (_p, diags) = parse_program("b = 1 < 2 < 3;");
        assert!(diags.iter().any(|d| d.code == "E0103"), "chained comparison must error");
    }

    #[test]
    fn recovers_and_reports_multiple_errors() {
        // first stmt has a broken RHS; recovery syncs past its `;` and still parses the
        // second, so one bad statement doesn't swallow the rest of the file.
        let (prog, diags) = parse_program("x = ; y = 2;");
        assert!(diags.iter().any(|d| d.code == "E0105"), "reported the bad expression");
        assert!(
            prog.iter().any(|s| matches!(&s.node, StmtKind::Assign { name, .. } if name == "y")),
            "recovered and parsed the second statement"
        );
    }

    #[test]
    fn malformed_input_terminates_and_never_wedges() {
        // Regression: `fn f( {` left a stray top-level `}` after recovery, where
        // `statement()` failed without advancing and `recover()` returned on the
        // `}` without consuming it — an infinite loop that pushed a diagnostic every
        // pass until it OOM'd. Every case here must terminate with diagnostics.
        for src in ["fn f( { return 1; }", "}", "){}]", "struct S {", "if {"] {
            let (_prog, diags) = parse_program(src);
            assert!(diags.iter().any(|d| d.is_error()), "reported an error for {src:?}");
        }
    }

    #[test]
    fn import_statements_parse() {
        let (prog, diags) = parse_program("import math;\nimport \"lib/util.ruspy\";\nprint 1;");
        assert!(!diags.iter().any(|d| d.is_error()), "clean parse: {diags:?}");
        use crate::ast::ImportSpec;
        let imports: Vec<&ImportSpec> = prog
            .iter()
            .filter_map(|s| match &s.node {
                StmtKind::Import { spec } => Some(spec),
                _ => None,
            })
            .collect();
        assert_eq!(imports.len(), 2);
        assert_eq!(imports[0], &ImportSpec::Package("math".into()));
        assert_eq!(imports[1], &ImportSpec::File("lib/util.ruspy".into()));
    }

    #[test]
    fn calls_members_and_trailing_comma() {
        parse_ok("price = tick.bid;");
        parse_ok("buy(0.1, 0.2,);");
    }
}
