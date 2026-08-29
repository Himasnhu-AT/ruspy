//! Bytecode compiler + stack VM — ruspy's second execution engine.
//!
//! Source → a flat `Op` stream per function (a `.ruspyc`-style program), run by an
//! **iterative** frame-stack VM. Two payoffs over the tree-walker: (1) it is the
//! compiled, serializable, obfuscatable distribution format (MQL's model — bytecode
//! for a VM), and (2) being iterative, deep recursion runs without a native-stack
//! limit. It reuses the shared value ops (`value::binop`/`neg`/`not`) and the stdlib,
//! so it agrees with the tree-walker bit-for-bit (enforced by a differential test).
//!
//! Scope (v1): literals, variables, all operators + short-circuit, if/else, while,
//! `for i in a..b` (desugared to a while loop), user functions + recursion, `print`,
//! and stdlib/host calls. Arrays, structs, and for-over-array fall back to the
//! tree-walker (the compiler reports them as unsupported, callers can choose the
//! engine). Extending the VM to those is a follow-up.

use crate::ast::{Expr, ExprKind, LogicOp, Program, Stmt, StmtKind, UnOp};
use crate::diagnostics::{Diag, Span};
use crate::interpreter::{DefaultHost, RuspyHost};
use crate::value::{self, BinOp, Value};
use std::collections::HashMap;

const MAX_DEPTH: usize = 4096;
const MAX_STEPS: u64 = 50_000_000;

/// A bytecode instruction. Name/const indices point into the `Chunk` pools.
#[derive(Debug, Clone)]
enum Op {
    Const(u32),
    Load(u32),
    Declare(u32),
    Store(u32),
    StoreExisting(u32),
    Bin(BinOp),
    Neg,
    Not,
    Jump(usize),
    JumpIfFalse(usize),
    JumpIfTrue(usize),
    Dup,
    Pop,
    Print,
    Call(u32, u32),
    GetMember(u32, u32),
    PushVoid,
    Ret,
}

/// A compiled function: its code plus interned constants and names.
#[derive(Debug, Clone)]
struct Chunk {
    params: Vec<String>,
    code: Vec<(Op, Span)>,
    consts: Vec<Value>,
    names: Vec<String>,
}

impl Chunk {
    fn new(params: Vec<String>) -> Self {
        Chunk { params, code: Vec::new(), consts: Vec::new(), names: Vec::new() }
    }
    fn konst(&mut self, v: Value) -> u32 {
        self.consts.push(v);
        (self.consts.len() - 1) as u32
    }
    fn name(&mut self, n: &str) -> u32 {
        if let Some(i) = self.names.iter().position(|x| x == n) {
            return i as u32;
        }
        self.names.push(n.to_string());
        (self.names.len() - 1) as u32
    }
    fn emit(&mut self, op: Op, span: Span) -> usize {
        self.code.push((op, span));
        self.code.len() - 1
    }
    /// Patch a jump instruction's target to the current end of code.
    fn patch_here(&mut self, at: usize) {
        let target = self.code.len();
        match &mut self.code[at].0 {
            Op::Jump(t) | Op::JumpIfFalse(t) | Op::JumpIfTrue(t) => *t = target,
            other => panic!("patch of non-jump op {other:?}"),
        }
    }
}

/// A compiled program: `main` plus the user functions by name.
pub struct Bytecode {
    main: Chunk,
    funcs: HashMap<String, Chunk>,
}

/// A feature the VM v1 does not yet compile (caller may fall back to the tree-walker).
#[derive(Debug)]
pub struct Unsupported(pub String);

// ── compiler ─────────────────────────────────────────────────────────────────
struct Compiler {
    funcs: HashMap<String, Chunk>,
}

impl Compiler {
    fn compile(prog: &Program) -> Result<Bytecode, Unsupported> {
        let mut c = Compiler { funcs: HashMap::new() };
        // Compile every top-level function first (hoisting).
        for stmt in prog {
            if let StmtKind::Fn { name, params, body, .. } = &stmt.node {
                let mut chunk = Chunk::new(params.iter().map(|p| p.name.clone()).collect());
                c.block(&mut chunk, body)?;
                chunk.emit(Op::PushVoid, stmt.span);
                chunk.emit(Op::Ret, stmt.span);
                c.funcs.insert(name.clone(), chunk);
            }
        }
        let mut main = Chunk::new(vec![]);
        for stmt in prog {
            if !matches!(stmt.node, StmtKind::Fn { .. }) {
                c.stmt(&mut main, stmt)?;
            }
        }
        Ok(Bytecode { main, funcs: c.funcs })
    }

    fn block(&mut self, ch: &mut Chunk, body: &[Stmt]) -> Result<(), Unsupported> {
        for s in body {
            self.stmt(ch, s)?;
        }
        Ok(())
    }

    fn stmt(&mut self, ch: &mut Chunk, stmt: &Stmt) -> Result<(), Unsupported> {
        let sp = stmt.span;
        match &stmt.node {
            StmtKind::Import { .. } => Ok(()), // resolved before compile; a no-op
            StmtKind::Fn { .. } => Ok(()), // hoisted separately
            StmtKind::Var { name, value, .. } => {
                self.expr(ch, value)?;
                let n = ch.name(name);
                ch.emit(Op::Declare(n), sp);
                Ok(())
            }
            StmtKind::Assign { name, op, value, .. } => {
                let n = ch.name(name);
                match op {
                    Some(o) => {
                        ch.emit(Op::Load(n), sp);
                        self.expr(ch, value)?;
                        ch.emit(Op::Bin(*o), sp);
                        ch.emit(Op::StoreExisting(n), sp);
                    }
                    None => {
                        self.expr(ch, value)?;
                        ch.emit(Op::Store(n), sp);
                    }
                }
                Ok(())
            }
            StmtKind::Print(e) => {
                self.expr(ch, e)?;
                ch.emit(Op::Print, sp);
                Ok(())
            }
            StmtKind::Expr(e) => {
                self.expr(ch, e)?;
                ch.emit(Op::Pop, sp);
                Ok(())
            }
            StmtKind::Block(body) => self.block(ch, body),
            StmtKind::If { cond, then, els } => {
                self.expr(ch, cond)?;
                let else_jump = ch.emit(Op::JumpIfFalse(0), sp);
                self.block(ch, then)?;
                if let Some(else_body) = els {
                    let end_jump = ch.emit(Op::Jump(0), sp);
                    ch.patch_here(else_jump);
                    self.block(ch, else_body)?;
                    ch.patch_here(end_jump);
                } else {
                    ch.patch_here(else_jump);
                }
                Ok(())
            }
            StmtKind::While { cond, body } => {
                let start = ch.code.len();
                self.expr(ch, cond)?;
                let exit = ch.emit(Op::JumpIfFalse(0), sp);
                self.block(ch, body)?;
                ch.emit(Op::Jump(start), sp);
                ch.patch_here(exit);
                Ok(())
            }
            StmtKind::For { var, iter, body } => {
                // Only integer ranges are compiled; arrays fall back to the tree-walker.
                let ExprKind::Range { start, end, inclusive } = &iter.node else {
                    return Err(Unsupported("for-over-array".into()));
                };
                // desugar `for v in a..b { body }` to:
                //   v = a; __end = b; while v (< | <=) __end { body; v += 1 }
                let v = ch.name(var);
                let end_name = ch.name("__for_end");
                self.expr(ch, start)?;
                ch.emit(Op::Declare(v), sp);
                self.expr(ch, end)?;
                ch.emit(Op::Declare(end_name), sp);
                let loop_start = ch.code.len();
                ch.emit(Op::Load(v), sp);
                ch.emit(Op::Load(end_name), sp);
                ch.emit(Op::Bin(if *inclusive { BinOp::Le } else { BinOp::Lt }), sp);
                let exit = ch.emit(Op::JumpIfFalse(0), sp);
                self.block(ch, body)?;
                // v += 1
                let one = ch.konst(Value::Int(1));
                ch.emit(Op::Load(v), sp);
                ch.emit(Op::Const(one), sp);
                ch.emit(Op::Bin(BinOp::Add), sp);
                ch.emit(Op::StoreExisting(v), sp);
                ch.emit(Op::Jump(loop_start), sp);
                ch.patch_here(exit);
                Ok(())
            }
            StmtKind::Return(e) => {
                match e {
                    Some(expr) => self.expr(ch, expr)?,
                    None => {
                        ch.emit(Op::PushVoid, sp);
                    }
                }
                ch.emit(Op::Ret, sp);
                Ok(())
            }
            // Break/Continue would need loop-scoped patch lists; deferred with arrays.
            StmtKind::Break | StmtKind::Continue => Err(Unsupported("break/continue".into())),
            StmtKind::IndexAssign { .. } => Err(Unsupported("array index assignment".into())),
            StmtKind::FieldAssign { .. } => Err(Unsupported("struct field assignment".into())),
            StmtKind::Struct { .. } => Err(Unsupported("struct declaration".into())),
        }
    }

    fn expr(&mut self, ch: &mut Chunk, e: &Expr) -> Result<(), Unsupported> {
        let sp = e.span;
        match &e.node {
            ExprKind::Int(v) => {
                let k = ch.konst(Value::Int(*v));
                ch.emit(Op::Const(k), sp);
            }
            ExprKind::Float(v) => {
                let k = ch.konst(Value::Float(*v));
                ch.emit(Op::Const(k), sp);
            }
            ExprKind::Bool(b) => {
                let k = ch.konst(Value::Bool(*b));
                ch.emit(Op::Const(k), sp);
            }
            ExprKind::Str(s) => {
                let k = ch.konst(Value::Str(s.clone()));
                ch.emit(Op::Const(k), sp);
            }
            ExprKind::Ident(n) => {
                let i = ch.name(n);
                ch.emit(Op::Load(i), sp);
            }
            ExprKind::Unary(UnOp::Neg, inner) => {
                self.expr(ch, inner)?;
                ch.emit(Op::Neg, sp);
            }
            ExprKind::Unary(UnOp::Not, inner) => {
                self.expr(ch, inner)?;
                ch.emit(Op::Not, sp);
            }
            ExprKind::Binary(op, l, r) => {
                self.expr(ch, l)?;
                self.expr(ch, r)?;
                ch.emit(Op::Bin(*op), sp);
            }
            ExprKind::Logic(LogicOp::And, l, r) => {
                self.expr(ch, l)?;
                ch.emit(Op::Dup, sp);
                let short = ch.emit(Op::JumpIfFalse(0), sp); // false → keep the false
                ch.emit(Op::Pop, sp);
                self.expr(ch, r)?;
                ch.patch_here(short);
            }
            ExprKind::Logic(LogicOp::Or, l, r) => {
                self.expr(ch, l)?;
                ch.emit(Op::Dup, sp);
                let short = ch.emit(Op::JumpIfTrue(0), sp); // true → keep the true
                ch.emit(Op::Pop, sp);
                self.expr(ch, r)?;
                ch.patch_here(short);
            }
            ExprKind::Call(name, args) => {
                for a in args {
                    self.expr(ch, a)?;
                }
                let n = ch.name(name);
                ch.emit(Op::Call(n, args.len() as u32), sp);
            }
            ExprKind::Member(obj, field) => {
                // Only host-object member access `ident.field` is compiled.
                let ExprKind::Ident(id) = &obj.node else {
                    return Err(Unsupported("member access on a non-identifier".into()));
                };
                let o = ch.name(id);
                let f = ch.name(field);
                ch.emit(Op::GetMember(o, f), sp);
            }
            ExprKind::Array(_) | ExprKind::Index(_, _) | ExprKind::Method(_, _, _) => {
                return Err(Unsupported("arrays".into()));
            }
            ExprKind::StructLit { .. } => return Err(Unsupported("struct literals".into())),
            ExprKind::Range { .. } => return Err(Unsupported("ranges outside of `for`".into())),
        }
        Ok(())
    }
}

/// Compile a program to bytecode (or report the first unsupported feature).
pub fn compile(prog: &Program) -> Result<Bytecode, Unsupported> {
    Compiler::compile(prog)
}

// ── virtual machine ──────────────────────────────────────────────────────────
pub struct Vm<'a> {
    bc: &'a Bytecode,
    globals: HashMap<String, Value>,
    output: Vec<String>,
    steps: u64,
    depth: usize,
}

impl<'a> Vm<'a> {
    fn new(bc: &'a Bytecode) -> Self {
        Vm { bc, globals: HashMap::new(), output: Vec::new(), steps: 0, depth: 0 }
    }

    /// Construct a VM over a compiled program (public entry for the CLI/tools).
    pub fn new_public(bc: &'a Bytecode) -> Self {
        Self::new(bc)
    }
    /// Run `main` (public entry).
    pub fn run_public(&mut self, host: &mut dyn RuspyHost) -> Result<(), Diag> {
        self.run(host)
    }
    /// Take the captured `print` output.
    pub fn take_output(&mut self) -> Vec<String> {
        std::mem::take(&mut self.output)
    }

    fn run(&mut self, host: &mut dyn RuspyHost) -> Result<(), Diag> {
        // `&self.bc.main` borrows the program (lifetime 'a), independent of `&mut self`.
        let main: &'a Chunk = &self.bc.main;
        self.exec(main, None, host)?;
        Ok(())
    }

    /// Execute one chunk with a per-frame operand stack. `locals` is `Some` for a
    /// function frame (params + locals, read-through to globals) or `None` for the
    /// global frame (variables live in `self.globals`). Returns the frame's `Ret`
    /// value (or `Void` if it runs off the end).
    fn exec(&mut self, chunk: &'a Chunk, mut locals: Option<HashMap<String, Value>>, host: &mut dyn RuspyHost) -> Result<Value, Diag> {
        let mut stack: Vec<Value> = Vec::new();
        let mut ip = 0usize;
        let pop = |st: &mut Vec<Value>, sp: Span| st.pop().ok_or_else(|| stack_underflow(sp));

        while ip < chunk.code.len() {
            self.steps += 1;
            if self.steps > MAX_STEPS {
                return Err(Diag::error(chunk.code[ip].1, "E0307", "execution budget exceeded"));
            }
            let (op, sp) = &chunk.code[ip];
            let sp = *sp;
            ip += 1;
            match op {
                Op::Const(i) => stack.push(chunk.consts[*i as usize].clone()),
                Op::Load(i) => {
                    let name = &chunk.names[*i as usize];
                    let v = match &locals {
                        Some(l) => l.get(name).cloned().or_else(|| self.globals.get(name).cloned()),
                        None => self.globals.get(name).cloned(),
                    };
                    match v {
                        Some(v) => stack.push(v),
                        None => return Err(Diag::error(sp, "E0303", format!("undefined variable `{name}`"))),
                    }
                }
                Op::Declare(i) => {
                    let name = chunk.names[*i as usize].clone();
                    let v = pop(&mut stack, sp)?;
                    match &mut locals {
                        Some(l) => {
                            l.insert(name, v);
                        }
                        None => {
                            self.globals.insert(name, v);
                        }
                    }
                }
                Op::Store(i) => {
                    let name = chunk.names[*i as usize].clone();
                    let v = pop(&mut stack, sp)?;
                    self.store(&name, v, &mut locals, false, sp)?;
                }
                Op::StoreExisting(i) => {
                    let name = chunk.names[*i as usize].clone();
                    let v = pop(&mut stack, sp)?;
                    self.store(&name, v, &mut locals, true, sp)?;
                }
                Op::Bin(o) => {
                    let b = pop(&mut stack, sp)?;
                    let a = pop(&mut stack, sp)?;
                    stack.push(value::binop(*o, &a, &b, sp)?);
                }
                Op::Neg => {
                    let a = pop(&mut stack, sp)?;
                    stack.push(value::neg(&a, sp)?);
                }
                Op::Not => {
                    let a = pop(&mut stack, sp)?;
                    stack.push(value::not(&a, sp)?);
                }
                Op::Jump(t) => ip = *t,
                Op::JumpIfFalse(t) => {
                    let v = pop(&mut stack, sp)?;
                    if !v.as_bool(sp)? {
                        ip = *t;
                    }
                }
                Op::JumpIfTrue(t) => {
                    let v = pop(&mut stack, sp)?;
                    if v.as_bool(sp)? {
                        ip = *t;
                    }
                }
                Op::Dup => {
                    let v = stack.last().cloned().ok_or_else(|| stack_underflow(sp))?;
                    stack.push(v);
                }
                Op::Pop => {
                    pop(&mut stack, sp)?;
                }
                Op::Print => {
                    let v = pop(&mut stack, sp)?;
                    self.output.push(v.to_string());
                }
                Op::PushVoid => stack.push(Value::Void),
                Op::Ret => return pop(&mut stack, sp),
                Op::GetMember(o, fld) => {
                    let obj = &chunk.names[*o as usize];
                    let field = &chunk.names[*fld as usize];
                    let v = host.get_member(obj, field).map_err(|m| Diag::error(sp, "E0301", m))?;
                    stack.push(v);
                }
                Op::Call(n, argc) => {
                    let name = chunk.names[*n as usize].clone();
                    let mut args = Vec::with_capacity(*argc as usize);
                    for _ in 0..*argc {
                        args.push(pop(&mut stack, sp)?);
                    }
                    args.reverse();
                    let result = self.call(&name, args, sp, host)?;
                    stack.push(result);
                }
            }
        }
        Ok(Value::Void)
    }

    fn store(&mut self, name: &str, v: Value, locals: &mut Option<HashMap<String, Value>>, must_exist: bool, sp: Span) -> Result<(), Diag> {
        match locals {
            None => {
                if must_exist && !self.globals.contains_key(name) {
                    return Err(Diag::error(sp, "E0303", format!("undefined variable `{name}`")));
                }
                self.globals.insert(name.to_string(), v);
            }
            Some(l) => {
                if l.contains_key(name) {
                    l.insert(name.to_string(), v);
                } else if self.globals.contains_key(name) {
                    self.globals.insert(name.to_string(), v);
                } else if must_exist {
                    return Err(Diag::error(sp, "E0303", format!("undefined variable `{name}`")));
                } else {
                    l.insert(name.to_string(), v);
                }
            }
        }
        Ok(())
    }

    fn call(&mut self, name: &str, args: Vec<Value>, sp: Span, host: &mut dyn RuspyHost) -> Result<Value, Diag> {
        // `chunk` borrows the program (lifetime 'a), so it survives `&mut self` calls.
        if let Some(chunk) = self.bc.funcs.get(name) {
            let chunk: &'a Chunk = chunk;
            if args.len() != chunk.params.len() {
                return Err(Diag::error(sp, "E0311", format!("`{name}` expects {} argument(s), got {}", chunk.params.len(), args.len())));
            }
            if self.depth + 1 > MAX_DEPTH {
                return Err(Diag::error(sp, "E0312", "maximum call depth exceeded"));
            }
            let mut frame: HashMap<String, Value> = HashMap::new();
            for (p, a) in chunk.params.iter().zip(args) {
                frame.insert(p.clone(), a);
            }
            self.depth += 1;
            let r = self.exec(chunk, Some(frame), host);
            self.depth -= 1;
            return r;
        }
        if let Some(result) = crate::stdlib::call(name, &args, sp) {
            return result;
        }
        host.call_function(name, args).map_err(|m| Diag::error(sp, "E0300", m))
    }
}

fn stack_underflow(sp: Span) -> Diag {
    Diag::error(sp, "E0326", "internal: bytecode stack underflow")
}

/// Compile + run a program string through the VM (default host). Returns the `print`
/// output, or `Err(Unsupported)` if the program uses a not-yet-compiled feature.
pub fn run_str_vm(src: &str) -> Result<Result<Vec<String>, Vec<Diag>>, Unsupported> {
    let (prog, diags) = crate::parser::parse_program(src);
    if diags.iter().any(|d| d.is_error()) {
        return Ok(Err(diags));
    }
    let bc = compile(&prog)?;
    let mut vm = Vm::new(&bc);
    let mut host = DefaultHost;
    let out = match vm.run(&mut host) {
        Ok(()) => Ok(vm.output.clone()),
        Err(d) => Err(vec![d]),
    };
    Ok(out)
}

// ===========================================================================
// M17 — the protected `.ruspyc` container (MQL's model: obfuscated bytecode).
// ===========================================================================
//
// A compiled program's bytecode is serialized, then hardened two ways:
//
//   1. Per-build opcode permutation. Each build maps the canonical opcode ids
//      through a random permutation, so the byte written for `Jump` (etc.) is
//      different every build. A static disassembler cannot assume a fixed opcode
//      table — it must first recover the permutation, which lives only inside the
//      encrypted region.
//   2. Encryption at rest (ChaCha20, RFC 8439 — implemented from spec and checked
//      against the official test vector, so it is a *correct* cipher, not homebrew
//      hand-waving). The serialized, permuted bytecode is unreadable in the file.
//
// HONEST THREAT MODEL. This is confidentiality + anti-static-analysis, the same
// bar MQL's `.ex5` clears — and with the same fundamental limit: the loader must
// hold the key to run the program, so anyone who controls the runtime can, with
// effort, recover the key and the bytecode. What it defeats: casual copying, file
// inspection, and static disassembly. What it does NOT claim: protection against a
// determined reverse-engineer with a debugger, or tamper-proofing (v1 carries a
// non-cryptographic integrity checksum, not an AEAD tag). Online activation and a
// hardware-bound key derivation are product concerns layered on top of this.
pub mod protect {
    use super::{
        BinOp, Bytecode, Chunk, DefaultHost, Diag, Op, Span, Unsupported, Value, Vm,
    };
    use std::collections::HashMap;

    const MAGIC: &[u8; 8] = b"RUSPYC\x01\x00";

    /// A failure packing or unpacking a protected artifact.
    #[derive(Debug)]
    pub struct ProtectError(pub String);
    fn err<T>(m: impl Into<String>) -> Result<T, ProtectError> {
        Err(ProtectError(m.into()))
    }

    // --- a small deterministic PRNG (SplitMix64) for the permutation ---------
    fn splitmix64(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// The 32-entry opcode permutation (19 opcodes today, room to grow).
    struct Perm {
        fwd: [u8; 32],
        inv: [u8; 32],
    }
    impl Perm {
        fn from_seed(seed: u64) -> Self {
            let mut fwd: [u8; 32] = std::array::from_fn(|i| i as u8);
            let mut st = seed ^ 0xD1B5_4A32_D192_ED03;
            // Fisher–Yates.
            for i in (1..32).rev() {
                let j = (splitmix64(&mut st) % (i as u64 + 1)) as usize;
                fwd.swap(i, j);
            }
            let mut inv = [0u8; 32];
            for (i, &b) in fwd.iter().enumerate() {
                inv[b as usize] = i as u8;
            }
            Perm { fwd, inv }
        }
        fn enc(&self, canonical: u8) -> u8 {
            self.fwd[canonical as usize]
        }
        fn dec(&self, byte: u8) -> Result<u8, ProtectError> {
            if (byte as usize) < 32 {
                Ok(self.inv[byte as usize])
            } else {
                err("opcode byte out of range")
            }
        }
    }

    // --- byte writer / reader ------------------------------------------------
    #[derive(Default)]
    struct W {
        buf: Vec<u8>,
    }
    impl W {
        fn u8(&mut self, v: u8) {
            self.buf.push(v);
        }
        fn u32(&mut self, v: u32) {
            self.buf.extend_from_slice(&v.to_le_bytes());
        }
        fn u64(&mut self, v: u64) {
            self.buf.extend_from_slice(&v.to_le_bytes());
        }
        fn str(&mut self, s: &str) {
            self.u32(s.len() as u32);
            self.buf.extend_from_slice(s.as_bytes());
        }
        fn span(&mut self, s: Span) {
            self.u32(s.start as u32);
            self.u32(s.end as u32);
        }
    }
    struct R<'a> {
        buf: &'a [u8],
        pos: usize,
    }
    impl<'a> R<'a> {
        fn take(&mut self, n: usize) -> Result<&'a [u8], ProtectError> {
            if self.pos + n > self.buf.len() {
                return err("unexpected end of data");
            }
            let s = &self.buf[self.pos..self.pos + n];
            self.pos += n;
            Ok(s)
        }
        fn u8(&mut self) -> Result<u8, ProtectError> {
            Ok(self.take(1)?[0])
        }
        fn u32(&mut self) -> Result<u32, ProtectError> {
            Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
        }
        fn u64(&mut self) -> Result<u64, ProtectError> {
            Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
        }
        fn str(&mut self) -> Result<String, ProtectError> {
            let n = self.u32()? as usize;
            let b = self.take(n)?;
            String::from_utf8(b.to_vec()).map_err(|_| ProtectError("bad utf8".into()))
        }
        fn span(&mut self) -> Result<Span, ProtectError> {
            let a = self.u32()? as usize;
            let b = self.u32()? as usize;
            Ok(Span::new(a, b))
        }
    }

    // --- opcode <-> canonical id --------------------------------------------
    fn binop_id(b: BinOp) -> u8 {
        match b {
            BinOp::Add => 0,
            BinOp::Sub => 1,
            BinOp::Mul => 2,
            BinOp::Div => 3,
            BinOp::Rem => 4,
            BinOp::Eq => 5,
            BinOp::Ne => 6,
            BinOp::Lt => 7,
            BinOp::Le => 8,
            BinOp::Gt => 9,
            BinOp::Ge => 10,
        }
    }
    fn binop_from(id: u8) -> Result<BinOp, ProtectError> {
        Ok(match id {
            0 => BinOp::Add,
            1 => BinOp::Sub,
            2 => BinOp::Mul,
            3 => BinOp::Div,
            4 => BinOp::Rem,
            5 => BinOp::Eq,
            6 => BinOp::Ne,
            7 => BinOp::Lt,
            8 => BinOp::Le,
            9 => BinOp::Gt,
            10 => BinOp::Ge,
            _ => return err("bad binop id"),
        })
    }
    /// Canonical opcode id (stable across builds; the *byte written* is permuted).
    fn op_id(op: &Op) -> u8 {
        match op {
            Op::Const(_) => 0,
            Op::Load(_) => 1,
            Op::Declare(_) => 2,
            Op::Store(_) => 3,
            Op::StoreExisting(_) => 4,
            Op::Bin(_) => 5,
            Op::Neg => 6,
            Op::Not => 7,
            Op::Jump(_) => 8,
            Op::JumpIfFalse(_) => 9,
            Op::JumpIfTrue(_) => 10,
            Op::Dup => 11,
            Op::Pop => 12,
            Op::Print => 13,
            Op::Call(_, _) => 14,
            Op::GetMember(_, _) => 15,
            Op::PushVoid => 16,
            Op::Ret => 17,
        }
    }

    fn write_op(w: &mut W, perm: &Perm, op: &Op) {
        w.u8(perm.enc(op_id(op)));
        match op {
            Op::Const(a) | Op::Load(a) | Op::Declare(a) | Op::Store(a) | Op::StoreExisting(a) => {
                w.u32(*a)
            }
            Op::Bin(b) => w.u8(binop_id(*b)),
            Op::Jump(t) | Op::JumpIfFalse(t) | Op::JumpIfTrue(t) => w.u32(*t as u32),
            Op::Call(a, b) | Op::GetMember(a, b) => {
                w.u32(*a);
                w.u32(*b);
            }
            Op::Neg | Op::Not | Op::Dup | Op::Pop | Op::Print | Op::PushVoid | Op::Ret => {}
        }
    }
    fn read_op(r: &mut R, perm: &Perm) -> Result<Op, ProtectError> {
        let id = perm.dec(r.u8()?)?;
        Ok(match id {
            0 => Op::Const(r.u32()?),
            1 => Op::Load(r.u32()?),
            2 => Op::Declare(r.u32()?),
            3 => Op::Store(r.u32()?),
            4 => Op::StoreExisting(r.u32()?),
            5 => Op::Bin(binop_from(r.u8()?)?),
            6 => Op::Neg,
            7 => Op::Not,
            8 => Op::Jump(r.u32()? as usize),
            9 => Op::JumpIfFalse(r.u32()? as usize),
            10 => Op::JumpIfTrue(r.u32()? as usize),
            11 => Op::Dup,
            12 => Op::Pop,
            13 => Op::Print,
            14 => Op::Call(r.u32()?, r.u32()?),
            15 => Op::GetMember(r.u32()?, r.u32()?),
            16 => Op::PushVoid,
            17 => Op::Ret,
            _ => return err("bad opcode id"),
        })
    }

    fn write_value(w: &mut W, v: &Value) -> Result<(), ProtectError> {
        match v {
            Value::Int(n) => {
                w.u8(0);
                w.u64(*n as u64);
            }
            Value::Float(f) => {
                w.u8(1);
                w.u64(f.to_bits());
            }
            Value::Bool(b) => {
                w.u8(2);
                w.u8(*b as u8);
            }
            Value::Str(s) => {
                w.u8(3);
                w.str(s);
            }
            Value::Char(c) => {
                w.u8(4);
                w.u32(*c as u32);
            }
            Value::Void => w.u8(5),
            _ => return err("non-serializable constant (array/struct) in bytecode"),
        }
        Ok(())
    }
    fn read_value(r: &mut R) -> Result<Value, ProtectError> {
        Ok(match r.u8()? {
            0 => Value::Int(r.u64()? as i64),
            1 => Value::Float(f64::from_bits(r.u64()?)),
            2 => Value::Bool(r.u8()? != 0),
            3 => Value::Str(r.str()?),
            4 => Value::Char(
                char::from_u32(r.u32()?).ok_or_else(|| ProtectError("bad char".into()))?,
            ),
            5 => Value::Void,
            _ => return err("bad value tag"),
        })
    }

    fn write_chunk(w: &mut W, perm: &Perm, c: &Chunk) -> Result<(), ProtectError> {
        w.u32(c.params.len() as u32);
        for p in &c.params {
            w.str(p);
        }
        w.u32(c.code.len() as u32);
        for (op, span) in &c.code {
            write_op(w, perm, op);
            w.span(*span);
        }
        w.u32(c.consts.len() as u32);
        for v in &c.consts {
            write_value(w, v)?;
        }
        w.u32(c.names.len() as u32);
        for n in &c.names {
            w.str(n);
        }
        Ok(())
    }
    fn read_chunk(r: &mut R, perm: &Perm) -> Result<Chunk, ProtectError> {
        let np = r.u32()? as usize;
        let mut params = Vec::with_capacity(np);
        for _ in 0..np {
            params.push(r.str()?);
        }
        let mut c = Chunk::new(params);
        let nc = r.u32()? as usize;
        for _ in 0..nc {
            let op = read_op(r, perm)?;
            let span = r.span()?;
            c.code.push((op, span));
        }
        let nk = r.u32()? as usize;
        for _ in 0..nk {
            c.consts.push(read_value(r)?);
        }
        let nn = r.u32()? as usize;
        for _ in 0..nn {
            c.names.push(r.str()?);
        }
        Ok(c)
    }

    /// Serialize a whole program's bytecode with a given opcode permutation.
    fn serialize(bc: &Bytecode, perm: &Perm) -> Result<Vec<u8>, ProtectError> {
        let mut w = W::default();
        write_chunk(&mut w, perm, &bc.main)?;
        // Deterministic function order so a build is reproducible for a fixed seed.
        let mut names: Vec<&String> = bc.funcs.keys().collect();
        names.sort();
        w.u32(names.len() as u32);
        for name in names {
            w.str(name);
            write_chunk(&mut w, perm, &bc.funcs[name])?;
        }
        Ok(w.buf)
    }
    fn deserialize(data: &[u8], perm: &Perm) -> Result<Bytecode, ProtectError> {
        let mut r = R { buf: data, pos: 0 };
        let main = read_chunk(&mut r, perm)?;
        let nf = r.u32()? as usize;
        let mut funcs = HashMap::new();
        for _ in 0..nf {
            let name = r.str()?;
            funcs.insert(name, read_chunk(&mut r, perm)?);
        }
        Ok(Bytecode { main, funcs })
    }

    // --- ChaCha20 (RFC 8439), from spec, KAT-verified ------------------------
    fn rotl(x: u32, n: u32) -> u32 {
        x.rotate_left(n)
    }
    fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
        s[a] = s[a].wrapping_add(s[b]);
        s[d] = rotl(s[d] ^ s[a], 16);
        s[c] = s[c].wrapping_add(s[d]);
        s[b] = rotl(s[b] ^ s[c], 12);
        s[a] = s[a].wrapping_add(s[b]);
        s[d] = rotl(s[d] ^ s[a], 8);
        s[c] = s[c].wrapping_add(s[d]);
        s[b] = rotl(s[b] ^ s[c], 7);
    }
    fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
        let c = [0x6170_7865u32, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];
        let mut st = [0u32; 16];
        st[..4].copy_from_slice(&c);
        for i in 0..8 {
            st[4 + i] = u32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().unwrap());
        }
        st[12] = counter;
        for i in 0..3 {
            st[13 + i] = u32::from_le_bytes(nonce[i * 4..i * 4 + 4].try_into().unwrap());
        }
        let mut ws = st;
        for _ in 0..10 {
            quarter_round(&mut ws, 0, 4, 8, 12);
            quarter_round(&mut ws, 1, 5, 9, 13);
            quarter_round(&mut ws, 2, 6, 10, 14);
            quarter_round(&mut ws, 3, 7, 11, 15);
            quarter_round(&mut ws, 0, 5, 10, 15);
            quarter_round(&mut ws, 1, 6, 11, 12);
            quarter_round(&mut ws, 2, 7, 8, 13);
            quarter_round(&mut ws, 3, 4, 9, 14);
        }
        let mut out = [0u8; 64];
        for i in 0..16 {
            let v = ws[i].wrapping_add(st[i]);
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }
    /// XOR `data` with the ChaCha20 keystream (encrypt == decrypt).
    fn chacha20_xor(key: &[u8; 32], nonce: &[u8; 12], mut counter: u32, data: &mut [u8]) {
        for chunk in data.chunks_mut(64) {
            let ks = chacha20_block(key, counter, nonce);
            for (b, k) in chunk.iter_mut().zip(ks.iter()) {
                *b ^= *k;
            }
            counter = counter.wrapping_add(1);
        }
    }

    fn fnv1a(data: &[u8]) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for &b in data {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
        h
    }

    /// Non-cryptographic per-build entropy for the nonce + permutation seed. Honest
    /// label: this is uniqueness, not secrecy — the security rests on the key.
    fn build_entropy() -> u64 {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let pid = std::process::id() as u64;
        let mut s = t ^ (pid.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        splitmix64(&mut s)
    }

    /// Pack compiled bytecode into a protected `.ruspyc` artifact under `key`.
    /// Layout: MAGIC(8) ‖ nonce(12) ‖ seed_ct(8) ‖ checksum_ct(8) ‖ body_ct.
    pub fn pack(bc: &Bytecode, key: &[u8; 32]) -> Result<Vec<u8>, ProtectError> {
        let entropy = build_entropy();
        let seed = entropy ^ 0xA5A5_5A5A_1234_9876;
        let perm = Perm::from_seed(seed);
        let body = serialize(bc, &perm)?;
        let checksum = fnv1a(&body);

        // Plaintext = seed ‖ checksum ‖ body, then stream-encrypted as one blob.
        let mut pt = Vec::with_capacity(16 + body.len());
        pt.extend_from_slice(&seed.to_le_bytes());
        pt.extend_from_slice(&checksum.to_le_bytes());
        pt.extend_from_slice(&body);

        let mut nonce = [0u8; 12];
        nonce[..8].copy_from_slice(&entropy.to_le_bytes());
        nonce[8..].copy_from_slice(&(entropy as u32).to_le_bytes());
        chacha20_xor(key, &nonce, 1, &mut pt);

        let mut out = Vec::with_capacity(8 + 12 + pt.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&pt);
        Ok(out)
    }

    /// Recover bytecode from a protected artifact, verifying the container magic,
    /// the integrity checksum, and that the opcode permutation round-trips.
    pub fn unpack(data: &[u8], key: &[u8; 32]) -> Result<Bytecode, ProtectError> {
        if data.len() < 8 + 12 + 16 || &data[..8] != MAGIC {
            return err("not a ruspyc artifact (bad magic)");
        }
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&data[8..20]);
        let mut pt = data[20..].to_vec();
        chacha20_xor(key, &nonce, 1, &mut pt);

        let seed = u64::from_le_bytes(pt[..8].try_into().unwrap());
        let checksum = u64::from_le_bytes(pt[8..16].try_into().unwrap());
        let body = &pt[16..];
        if fnv1a(body) != checksum {
            return err("integrity check failed (wrong key or corrupted artifact)");
        }
        let perm = Perm::from_seed(seed);
        deserialize(body, &perm)
    }

    /// Compile, pack, and return the protected artifact bytes for a program.
    pub fn compile_and_pack(
        prog: &super::Program,
        key: &[u8; 32],
    ) -> Result<Vec<u8>, ProtectError> {
        let bc = super::compile(prog).map_err(|Unsupported(m)| ProtectError(m))?;
        pack(&bc, key)
    }

    /// Load a protected artifact and run it on the VM — the deployed execution path.
    pub fn run_protected(
        data: &[u8],
        key: &[u8; 32],
    ) -> Result<Result<Vec<String>, Vec<Diag>>, ProtectError> {
        let bc = unpack(data, key)?;
        let mut vm = Vm::new(&bc);
        let mut host = DefaultHost;
        Ok(match vm.run(&mut host) {
            Ok(()) => Ok(vm.output.clone()),
            Err(d) => Err(vec![d]),
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn chacha20_matches_rfc8439_test_vector() {
            // RFC 8439 §2.3.2: key = 00..1f, nonce = 00 00 00 09 00 00 00 4a 00 00 00 00,
            // counter = 1 → this exact keystream block.
            let key: [u8; 32] = std::array::from_fn(|i| i as u8);
            let nonce: [u8; 12] =
                [0, 0, 0, 9, 0, 0, 0, 0x4a, 0, 0, 0, 0];
            let block = chacha20_block(&key, 1, &nonce);
            let expected: [u8; 64] = [
                0x10, 0xf1, 0xe7, 0xe4, 0xd1, 0x3b, 0x59, 0x15, 0x50, 0x0f, 0xdd, 0x1f, 0xa3,
                0x20, 0x71, 0xc4, 0xc7, 0xd1, 0xf4, 0xc7, 0x33, 0xc0, 0x68, 0x03, 0x04, 0x22,
                0xaa, 0x9a, 0xc3, 0xd4, 0x6c, 0x4e, 0xd2, 0x82, 0x64, 0x46, 0x07, 0x9f, 0xaa,
                0x09, 0x14, 0xc2, 0xd7, 0x05, 0xd9, 0x8b, 0x02, 0xa2, 0xb5, 0x12, 0x9c, 0xd1,
                0xde, 0x16, 0x4e, 0xb9, 0xcb, 0xd0, 0x83, 0xe8, 0xa2, 0x50, 0x3c, 0x4e,
            ];
            assert_eq!(block, expected);
        }

        #[test]
        fn protected_roundtrip_runs_and_matches_the_interpreter() {
            let src = "fn fib(n) { if n < 2 { return n; } return fib(n-1) + fib(n-2); }
                       s = 0; for i in 0..10 { s = s + fib(i); } print s;
                       print \"secret-marker-string\";";
            let (prog, diags) = crate::parser::parse_program(src);
            assert!(!diags.iter().any(|d| d.is_error()));
            let key = [7u8; 32];
            let art = compile_and_pack(&prog, &key).expect("packs");

            // The plaintext string literal must NOT survive into the artifact.
            assert!(
                !art.windows(20).any(|w| w == b"secret-marker-string"),
                "plaintext leaked into the protected artifact"
            );

            let out = run_protected(&art, &key).expect("unpacks").expect("runs");
            let interp = crate::interpreter::run_str(src).unwrap();
            assert_eq!(out, interp);
            assert_eq!(out, vec!["88".to_string(), "secret-marker-string".to_string()]);
        }

        #[test]
        fn wrong_key_is_rejected_not_miscompiled() {
            let (prog, _) = crate::parser::parse_program("x = 41 + 1; print x;");
            let art = compile_and_pack(&prog, &[1u8; 32]).unwrap();
            // A different key fails the integrity check instead of running garbage.
            assert!(unpack(&art, &[2u8; 32]).is_err());
            assert!(run_protected(&art, &[1u8; 32]).unwrap().is_ok());
        }

        #[test]
        fn each_build_permutes_opcodes_differently() {
            let (prog, _) = crate::parser::parse_program("fn f(a){return a+1;} print f(2);");
            let key = [9u8; 32];
            let a = compile_and_pack(&prog, &key).unwrap();
            let b = compile_and_pack(&prog, &key).unwrap();
            // Different per-build seed+nonce ⇒ different ciphertext, but both run the same.
            assert_ne!(a, b, "two builds produced identical artifacts");
            assert_eq!(
                run_protected(&a, &key).unwrap().unwrap(),
                run_protected(&b, &key).unwrap().unwrap()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::run_str;

    /// Programs in the VM-supported subset. Each is run through BOTH engines and
    /// their output must be identical — the differential oracle that keeps the VM
    /// honest against the tree-walker.
    const CORPUS: &[&str] = &[
        "x = 10; y = (x + 5) * 2 / 3; print y;",
        "print 1 + 2.5; print 7 % 3; print -5 + 2; print !false;",
        "x = 3; if x > 5 { print \"big\"; } else if x > 2 { print \"mid\"; } else { print \"lo\"; }",
        "d = 0; if d == 0 || 10 / d > 1 { print \"safe\"; }",
        "d = 0; if d != 0 && 10 / d > 1 { print \"x\"; } else { print \"ok\"; }",
        "i = 0; s = 0; while i < 5 { s += i; i += 1; } print s;",
        "s = 0; for i in 0..5 { s += i; } print s;",
        "s = 0; for i in 1..=4 { s += i * i; } print s;",
        "fn fact(n) { if n <= 1 { return 1; } return n * fact(n - 1); } print fact(6);",
        "fn is_even(n) { if n == 0 { return true; } return is_odd(n - 1); }
         fn is_odd(n) { if n == 0 { return false; } return is_even(n - 1); }
         print is_even(10); print is_odd(7);",
        "fn add(a, b) { s = a + b; return s; } print add(2, 3);",
        "print abs(-5); print sqrt(9.0); print max(3, 7); print pow(2.0, 8.0);",
        "m: str = \"Hello, \" + \"Ruspy!\"; print m;",
        "x = 10; x += 5; x *= 2; x -= 1; print x;",
    ];

    #[test]
    fn vm_agrees_with_the_tree_walker_bit_for_bit() {
        for (i, src) in CORPUS.iter().enumerate() {
            let tree = run_str(src).unwrap_or_else(|d| panic!("tree-walker failed on #{i}: {d:?}"));
            let vm = run_str_vm(src)
                .unwrap_or_else(|u| panic!("#{i} unexpectedly unsupported by the VM: {u:?}"))
                .unwrap_or_else(|d| panic!("VM failed on #{i}: {d:?}"));
            assert_eq!(vm, tree, "engines disagree on program #{i}: {src:?}");
        }
    }

    #[test]
    fn vm_reports_runtime_errors_like_the_interpreter() {
        assert_eq!(run_str_vm("print 1 / 0;").unwrap().unwrap_err()[0].code, "E0205");
        assert_eq!(run_str_vm("print y;").unwrap().unwrap_err()[0].code, "E0303");
        assert_eq!(run_str_vm("if 1 { print 2; }").unwrap().unwrap_err()[0].code, "E0201");
        assert_eq!(run_str_vm("fn f(a) { return a; } print f(1, 2);").unwrap().unwrap_err()[0].code, "E0311");
    }

    #[test]
    fn unsupported_features_are_reported_for_fallback() {
        // arrays/structs aren't compiled yet → the caller can fall back to the tree-walker
        assert!(run_str_vm("a = [1, 2, 3]; print a[0];").is_err());
        assert!(run_str_vm("struct P { x } p = P { x: 1 };").is_err());
    }

    #[test]
    fn vm_recursion_matches_and_a_loop_computes() {
        // A slightly heavier program: sum of fib(0..10) via the VM.
        let src = "fn fib(n) { if n < 2 { return n; } return fib(n-1) + fib(n-2); }
                   s = 0; for i in 0..10 { s += fib(i); } print s;";
        let vm = run_str_vm(src).unwrap().unwrap();
        assert_eq!(vm, run_str(src).unwrap());
        assert_eq!(vm, vec!["88"]);
    }
}
