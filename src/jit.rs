//! Native code generation via Cranelift (M14/M15).
//!
//! Behind the `jit` feature so the core build stays lean. This module lowers the
//! scalar core of ruspy — the per-tick body: f64/i64 math, comparisons, `if`/
//! `while`, and calls — to native machine code, the third execution engine that
//! must agree with the tree-walker and the VM.
//!
//! v0 here is a *smoke*: it JITs a function equivalent to `fn() -> i64 { 42 }` and
//! calls it, proving the toolchain produces and runs native code in this workspace.
//! The real AST→CLIF lowering lands on top of this.
#![cfg(feature = "jit")]

use cranelift::prelude::*;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};

/// JIT-compile `fn() -> i64 { n }` and run it, returning the native result.
/// A minimal end-to-end proof that Cranelift codegen works here.
pub fn smoke_const(n: i64) -> i64 {
    let mut flag_builder = settings::builder();
    flag_builder.set("use_colocated_libcalls", "false").unwrap();
    flag_builder.set("is_pic", "false").unwrap();
    let isa_builder = cranelift_native::builder().expect("host machine is not supported");
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .unwrap();
    let builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    let mut module = JITModule::new(builder);

    let mut ctx = module.make_context();
    let mut fbc = FunctionBuilderContext::new();

    let target_config = module.target_config();
    let int = target_config.pointer_type();
    ctx.func.signature.returns.push(AbiParam::new(int));

    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
        let block = b.create_block();
        b.switch_to_block(block);
        b.seal_block(block);
        let v = b.ins().iconst(int, n);
        b.ins().return_(&[v]);
        b.finalize(target_config);
    }

    let id = module
        .declare_function("smoke", Linkage::Export, &ctx.func.signature)
        .unwrap();
    module.define_function(id, &mut ctx).unwrap();
    module.clear_context(&mut ctx);
    module.finalize_definitions().unwrap();

    let code = module.get_finalized_function(id);
    let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(code) };
    f()
}

/// JIT-compile `fn(x: f64) -> f64 { x*x + 2*x + 1 }` and run it. Proves the actual
/// per-tick shape works natively: an f64 parameter in, f64 arithmetic (fmul/fadd,
/// float const), an f64 result out — the scalar core the AST→CLIF lowering targets.
pub fn smoke_poly(x: f64) -> f64 {
    let mut flag_builder = settings::builder();
    flag_builder.set("use_colocated_libcalls", "false").unwrap();
    flag_builder.set("is_pic", "false").unwrap();
    let isa = cranelift_native::builder()
        .expect("host machine is not supported")
        .finish(settings::Flags::new(flag_builder))
        .unwrap();
    let builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    let mut module = JITModule::new(builder);

    let mut ctx = module.make_context();
    let mut fbc = FunctionBuilderContext::new();
    let target_config = module.target_config();

    ctx.func.signature.params.push(AbiParam::new(types::F64));
    ctx.func.signature.returns.push(AbiParam::new(types::F64));

    {
        let mut b = FunctionBuilder::new(&mut ctx.func, &mut fbc);
        let block = b.create_block();
        b.append_block_params_for_function_params(block);
        b.switch_to_block(block);
        b.seal_block(block);
        let x = b.block_params(block)[0];
        let two = b.ins().f64const(2.0);
        let one = b.ins().f64const(1.0);
        let x2 = b.ins().fmul(x, x); // x*x
        let tx = b.ins().fmul(two, x); // 2*x
        let s = b.ins().fadd(x2, tx); // x*x + 2*x
        let r = b.ins().fadd(s, one); // + 1
        b.ins().return_(&[r]);
        b.finalize(target_config);
    }

    let id = module
        .declare_function("poly", Linkage::Export, &ctx.func.signature)
        .unwrap();
    module.define_function(id, &mut ctx).unwrap();
    module.clear_context(&mut ctx);
    module.finalize_definitions().unwrap();

    let code = module.get_finalized_function(id);
    let f: extern "C" fn(f64) -> f64 = unsafe { std::mem::transmute(code) };
    f(x)
}

// ---------------------------------------------------------------------------
// Real lowering: ruspy AST -> CLIF -> native code (the scalar core).
// ---------------------------------------------------------------------------

use crate::ast::{Expr, ExprKind, LogicOp, Program, Stmt, StmtKind, UnOp};
use crate::value::BinOp;
use cranelift::prelude::Value as CraneliftValue;
use std::collections::HashMap;

/// A feature the native backend does not lower; the caller falls back to the VM
/// or tree-walker, exactly as the VM falls back for arrays/structs.
#[derive(Debug, Clone)]
pub struct NotScalar(pub String);

fn bail<T>(msg: impl Into<String>) -> Result<T, NotScalar> {
    Err(NotScalar(msg.into()))
}

/// The static type of a lowered SSA value. ruspy is dynamically typed at runtime,
/// but the scalar core is monomorphic per expression, so codegen tracks one of
/// these and inserts int->float promotions exactly where `value::binop` would.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ScalarTy {
    I64,
    F64,
    Bool,
}

impl ScalarTy {
    fn clif(self) -> Type {
        match self {
            ScalarTy::I64 => types::I64,
            ScalarTy::F64 => types::F64,
            ScalarTy::Bool => types::I8,
        }
    }
}

/// A compiled program: the JIT module (kept alive so its code stays mapped) plus
/// each function's entry pointer and arity. All functions use a uniform C ABI of
/// `f64` params and an `f64` result — the per-tick numeric boundary.
pub struct CompiledModule {
    _module: JITModule,
    funcs: HashMap<String, (*const u8, usize)>,
}

impl CompiledModule {
    /// Call a compiled function by name with `f64` arguments. Returns `None` if the
    /// name is unknown or the arity doesn't match (up to 6 params supported).
    pub fn call(&self, name: &str, args: &[f64]) -> Option<f64> {
        let &(ptr, arity) = self.funcs.get(name)?;
        if arity != args.len() {
            return None;
        }
        // Safety: `ptr` is a finalized function of exactly this arity with an
        // (f64,..)->f64 signature; the module outlives the call.
        unsafe {
            Some(match args {
                [] => std::mem::transmute::<_, extern "C" fn() -> f64>(ptr)(),
                [a] => std::mem::transmute::<_, extern "C" fn(f64) -> f64>(ptr)(*a),
                [a, b] => std::mem::transmute::<_, extern "C" fn(f64, f64) -> f64>(ptr)(*a, *b),
                [a, b, c] => {
                    std::mem::transmute::<_, extern "C" fn(f64, f64, f64) -> f64>(ptr)(*a, *b, *c)
                }
                [a, b, c, d] => std::mem::transmute::<_, extern "C" fn(f64, f64, f64, f64) -> f64>(
                    ptr,
                )(*a, *b, *c, *d),
                [a, b, c, d, e] => std::mem::transmute::<
                    _,
                    extern "C" fn(f64, f64, f64, f64, f64) -> f64,
                >(ptr)(*a, *b, *c, *d, *e),
                [a, b, c, d, e, f] => std::mem::transmute::<
                    _,
                    extern "C" fn(f64, f64, f64, f64, f64, f64) -> f64,
                >(ptr)(*a, *b, *c, *d, *e, *f),
                _ => return None,
            })
        }
    }

    /// The names of every compiled function.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.funcs.keys().map(|s| s.as_str())
    }
}

/// Build a host-native ISA. The JIT wants non-PIC (it patches absolute call
/// targets in memory); an object file for a PIE executable wants PIC.
fn host_isa(pic: bool) -> Result<std::sync::Arc<dyn isa::TargetIsa>, NotScalar> {
    let mut flag_builder = settings::builder();
    flag_builder.set("use_colocated_libcalls", "false").unwrap();
    flag_builder.set("is_pic", if pic { "true" } else { "false" }).unwrap();
    cranelift_native::builder()
        .map_err(|e| NotScalar(format!("unsupported host: {e}")))?
        .finish(settings::Flags::new(flag_builder))
        .map_err(|e| NotScalar(format!("isa: {e}")))
}

/// Collect the top-level `fn`s, checking the per-function param cap.
fn collect_fns(prog: &Program) -> Result<Vec<(&str, &[crate::ast::Param], &[Stmt])>, NotScalar> {
    let mut fns: Vec<(&str, &[crate::ast::Param], &[Stmt])> = Vec::new();
    for s in prog {
        if let StmtKind::Fn { name, params, body, .. } = &s.node {
            if params.len() > 6 {
                return bail(format!("`{name}` has more than 6 params"));
            }
            fns.push((name, params, body));
        }
    }
    if fns.is_empty() {
        return bail("no functions to compile");
    }
    Ok(fns)
}

/// Declare then define every function into `module`. Generic over the Cranelift
/// backend (`JITModule` for in-memory JIT, `ObjectModule` for AOT `.o` emission) —
/// the one lowering path the research called for. Declaring all ids first lets
/// calls between functions (and recursion) resolve during body lowering.
fn lower_all<M: Module>(
    module: &mut M,
    prog: &Program,
) -> Result<HashMap<String, (cranelift_module::FuncId, usize)>, NotScalar> {
    let fns = collect_fns(prog)?;

    let mut ids: HashMap<String, (cranelift_module::FuncId, usize)> = HashMap::new();
    for (name, params, _) in &fns {
        let mut sig = module.make_signature();
        for _ in *params {
            sig.params.push(AbiParam::new(types::F64));
        }
        sig.returns.push(AbiParam::new(types::F64));
        // Exported linker symbol is `ruspy_<name>` so an entry called `main` never
        // clashes with the C runtime's `main`; internal calls resolve by FuncId, so
        // the `ids` map stays keyed by the original ruspy name.
        let id = module
            .declare_function(&format!("ruspy_{name}"), Linkage::Export, &sig)
            .map_err(|e| NotScalar(format!("declare {name}: {e}")))?;
        ids.insert((*name).to_string(), (id, params.len()));
    }

    let mut ctx = module.make_context();
    let mut fbc = FunctionBuilderContext::new();
    for (name, params, body) in &fns {
        for _ in *params {
            ctx.func.signature.params.push(AbiParam::new(types::F64));
        }
        ctx.func.signature.returns.push(AbiParam::new(types::F64));
        let target_config = module.target_config();

        {
            let mut fb = FunctionBuilder::new(&mut ctx.func, &mut fbc);
            let entry = fb.create_block();
            fb.append_block_params_for_function_params(entry);
            fb.switch_to_block(entry);
            fb.seal_block(entry);

            {
                let mut lc = Lowerer {
                    b: &mut fb,
                    module,
                    ids: &ids,
                    vars: HashMap::new(),
                    next_var: 0,
                };
                for (i, p) in params.iter().enumerate() {
                    let val = lc.b.block_params(entry)[i];
                    let var = lc.fresh(&p.name, ScalarTy::F64);
                    lc.b.def_var(var, val);
                }
                let terminated = lc.block(body)?;
                if !terminated {
                    let zero = lc.b.ins().f64const(0.0);
                    lc.b.ins().return_(&[zero]);
                }
            }
            fb.finalize(target_config);
        }

        let (id, _) = ids[*name];
        module
            .define_function(id, &mut ctx)
            .map_err(|e| NotScalar(format!("define {name}: {e}")))?;
        module.clear_context(&mut ctx);
    }
    Ok(ids)
}

/// Compile every top-level `fn` to native code in memory (JIT). Returns `NotScalar`
/// if any function uses a non-scalar feature (strings, arrays, structs, non-scalar
/// stdlib, float `%`, nested fns, >6 params) — the caller then falls back.
pub fn compile_program(prog: &Program) -> Result<CompiledModule, NotScalar> {
    let builder = JITBuilder::with_isa(host_isa(false)?, cranelift_module::default_libcall_names());
    let mut module = JITModule::new(builder);

    let ids = lower_all(&mut module, prog)?;
    module
        .finalize_definitions()
        .map_err(|e| NotScalar(format!("finalize: {e}")))?;

    let mut funcs = HashMap::new();
    for (name, (id, arity)) in ids {
        funcs.insert(name, (module.get_finalized_function(id), arity));
    }
    Ok(CompiledModule { _module: module, funcs })
}

/// AOT-compile a program to a **native object file** for the host platform (Mach-O
/// on macOS, ELF on Linux, COFF on Windows). Returns the raw object bytes plus the
/// exported function names. This is the "ruspy produces a platform-specific binary"
/// deliverable: link the bytes with a tiny runtime `main` to get a standalone exe.
pub fn compile_object(prog: &Program, name: &str) -> Result<(Vec<u8>, Vec<String>), NotScalar> {
    use cranelift_object::{ObjectBuilder, ObjectModule};
    let builder = ObjectBuilder::new(
        host_isa(true)?,
        name.to_string(),
        cranelift_module::default_libcall_names(),
    )
    .map_err(|e| NotScalar(format!("object builder: {e}")))?;
    let mut module = ObjectModule::new(builder);

    let ids = lower_all(&mut module, prog)?;
    let mut names: Vec<String> = ids.keys().cloned().collect();
    names.sort();

    let product = module.finish();
    let bytes = product
        .emit()
        .map_err(|e| NotScalar(format!("emit object: {e}")))?;
    Ok((bytes, names))
}

/// Per-function lowering state, generic over the Cranelift backend module.
struct Lowerer<'a, 'b, M: Module> {
    b: &'a mut FunctionBuilder<'b>,
    module: &'a mut M,
    ids: &'a HashMap<String, (cranelift_module::FuncId, usize)>,
    /// ruspy variable name -> (Cranelift variable, its scalar type).
    vars: HashMap<String, (Variable, ScalarTy)>,
    next_var: usize,
}

impl<'a, 'b, M: Module> Lowerer<'a, 'b, M> {
    fn fresh(&mut self, name: &str, ty: ScalarTy) -> Variable {
        self.next_var += 1; // bumps the temp-name counter used by `lower_logic`
        let var = self.b.declare_var(ty.clif());
        self.vars.insert(name.to_string(), (var, ty));
        var
    }

    /// Lower a block of statements. Returns whether it terminated (hit a `return`).
    fn block(&mut self, stmts: &[Stmt]) -> Result<bool, NotScalar> {
        for s in stmts {
            if self.stmt(s)? {
                return Ok(true); // rest is dead code
            }
        }
        Ok(false)
    }

    fn stmt(&mut self, s: &Stmt) -> Result<bool, NotScalar> {
        match &s.node {
            StmtKind::Var { name, value, .. } => {
                let (v, ty) = self.expr(value)?;
                let var = self.fresh(name, ty);
                self.b.def_var(var, v);
                Ok(false)
            }
            StmtKind::Assign { name, op, value, .. } => {
                match self.vars.get(name).copied() {
                    Some((var, ty)) => {
                        let (mut v, mut vty) = self.expr(value)?;
                        if let Some(binop) = op {
                            let cur = self.b.use_var(var);
                            (v, vty) = self.binop(*binop, cur, ty, v, vty)?;
                        }
                        let coerced = self.coerce(v, vty, ty)?;
                        self.b.def_var(var, coerced);
                    }
                    None => {
                        // First plain assignment declares the variable (ruspy has no
                        // `let`); a compound op on an undeclared name is an error.
                        if op.is_some() {
                            return bail(format!("compound-assign to unknown `{name}`"));
                        }
                        let (v, vty) = self.expr(value)?;
                        let var = self.fresh(name, vty);
                        self.b.def_var(var, v);
                    }
                }
                Ok(false)
            }
            StmtKind::Expr(e) => {
                self.expr(e)?;
                Ok(false)
            }
            StmtKind::Return(e) => {
                let ret = match e {
                    Some(e) => {
                        let (v, ty) = self.expr(e)?;
                        self.coerce(v, ty, ScalarTy::F64)?
                    }
                    None => self.b.ins().f64const(0.0),
                };
                self.b.ins().return_(&[ret]);
                Ok(true)
            }
            StmtKind::If { cond, then, els } => self.lower_if(cond, then, els.as_deref()),
            StmtKind::While { cond, body } => self.lower_while(cond, body),
            StmtKind::For { var, iter, body } => self.lower_for(var, iter, body),
            StmtKind::Block(b) => self.block(b),
            StmtKind::Print(_) => bail("`print` is not supported by the native backend"),
            StmtKind::Break | StmtKind::Continue => bail("break/continue not yet native"),
            StmtKind::Fn { .. } => bail("nested functions not supported"),
            StmtKind::Struct { .. }
            | StmtKind::IndexAssign { .. }
            | StmtKind::FieldAssign { .. } => bail("aggregate statement not scalar"),
            // `import` is resolved before lowering: file imports are spliced in by the
            // loader and package imports only gate name resolution in the checker. By
            // the time we reach codegen it carries no runtime effect.
            StmtKind::Import { .. } => Ok(false),
        }
    }

    fn lower_if(
        &mut self,
        cond: &Expr,
        then: &[Stmt],
        els: Option<&[Stmt]>,
    ) -> Result<bool, NotScalar> {
        let c = self.cond_value(cond)?;
        let then_b = self.b.create_block();
        let else_b = self.b.create_block();
        let merge_b = self.b.create_block();
        self.b.ins().brif(c, then_b, &[], else_b, &[]);

        self.b.switch_to_block(then_b);
        self.b.seal_block(then_b);
        let then_term = self.block(then)?;
        if !then_term {
            self.b.ins().jump(merge_b, &[]);
        }

        self.b.switch_to_block(else_b);
        self.b.seal_block(else_b);
        let else_term = match els {
            Some(e) => self.block(e)?,
            None => false,
        };
        if !else_term {
            self.b.ins().jump(merge_b, &[]);
        }

        self.b.switch_to_block(merge_b);
        self.b.seal_block(merge_b);
        // The whole `if` terminates only if both arms did (merge is then unreachable,
        // but sealed+empty is fine — nothing branches past it).
        Ok(then_term && else_term)
    }

    fn lower_while(&mut self, cond: &Expr, body: &[Stmt]) -> Result<bool, NotScalar> {
        let header = self.b.create_block();
        let body_b = self.b.create_block();
        let exit = self.b.create_block();
        self.b.ins().jump(header, &[]);

        self.b.switch_to_block(header);
        let c = self.cond_value(cond)?;
        self.b.ins().brif(c, body_b, &[], exit, &[]);

        self.b.switch_to_block(body_b);
        self.b.seal_block(body_b);
        let term = self.block(body)?;
        if !term {
            self.b.ins().jump(header, &[]);
        }
        self.b.seal_block(header); // preds: entry + back-edge, both now known

        self.b.switch_to_block(exit);
        self.b.seal_block(exit);
        Ok(false)
    }

    /// `for v in a..b { body }` desugared to a while over an induction variable.
    fn lower_for(&mut self, var: &str, iter: &Expr, body: &[Stmt]) -> Result<bool, NotScalar> {
        let (start, end, inclusive) = match &iter.node {
            ExprKind::Range { start, end, inclusive } => (start, end, *inclusive),
            _ => return bail("native `for` supports only ranges"),
        };
        let (s0, sty) = self.expr(start)?;
        let (e0, ety) = self.expr(end)?;
        // Require integer ranges (the common case); anything else falls back.
        if sty != ScalarTy::I64 || ety != ScalarTy::I64 {
            return bail("native `for` supports only integer ranges");
        }
        let iv = self.fresh(var, ScalarTy::I64);
        self.b.def_var(iv, s0);
        // Stash the end in its own variable so the loop re-reads a stable value.
        let end_var = self.fresh("__for_end", ScalarTy::I64);
        self.b.def_var(end_var, e0);

        let header = self.b.create_block();
        let body_b = self.b.create_block();
        let exit = self.b.create_block();
        self.b.ins().jump(header, &[]);

        self.b.switch_to_block(header);
        let i = self.b.use_var(iv);
        let e = self.b.use_var(end_var);
        let cc = if inclusive { IntCC::SignedLessThanOrEqual } else { IntCC::SignedLessThan };
        let c = self.b.ins().icmp(cc, i, e);
        self.b.ins().brif(c, body_b, &[], exit, &[]);

        self.b.switch_to_block(body_b);
        self.b.seal_block(body_b);
        let term = self.block(body)?;
        if !term {
            let i = self.b.use_var(iv);
            let one = self.b.ins().iconst(types::I64, 1);
            let next = self.b.ins().iadd(i, one);
            self.b.def_var(iv, next);
            self.b.ins().jump(header, &[]);
        }
        self.b.seal_block(header);

        self.b.switch_to_block(exit);
        self.b.seal_block(exit);
        Ok(false)
    }

    /// Lower `cond` to a branchable i8 (nonzero = true).
    fn cond_value(&mut self, cond: &Expr) -> Result<CraneliftValue, NotScalar> {
        let (v, ty) = self.expr(cond)?;
        match ty {
            ScalarTy::Bool => Ok(v),
            _ => bail("condition must be a boolean"),
        }
    }

    /// Convert `v: from` to `to`, matching ruspy's int->float promotion. Narrowing
    /// float->int is refused (never needed by the scalar corpus).
    fn coerce(
        &mut self,
        v: CraneliftValue,
        from: ScalarTy,
        to: ScalarTy,
    ) -> Result<CraneliftValue, NotScalar> {
        if from == to {
            return Ok(v);
        }
        match (from, to) {
            (ScalarTy::I64, ScalarTy::F64) => Ok(self.b.ins().fcvt_from_sint(types::F64, v)),
            (ScalarTy::Bool, ScalarTy::F64) => {
                let i = self.b.ins().uextend(types::I64, v);
                Ok(self.b.ins().fcvt_from_sint(types::F64, i))
            }
            (ScalarTy::Bool, ScalarTy::I64) => Ok(self.b.ins().uextend(types::I64, v)),
            _ => bail("unsupported numeric coercion"),
        }
    }

    fn expr(&mut self, e: &Expr) -> Result<(CraneliftValue, ScalarTy), NotScalar> {
        match &e.node {
            ExprKind::Int(n) => Ok((self.b.ins().iconst(types::I64, *n), ScalarTy::I64)),
            ExprKind::Float(f) => Ok((self.b.ins().f64const(*f), ScalarTy::F64)),
            ExprKind::Bool(bv) => {
                Ok((self.b.ins().iconst(types::I8, *bv as i64), ScalarTy::Bool))
            }
            ExprKind::Ident(name) => {
                let (var, ty) = *self
                    .vars
                    .get(name)
                    .ok_or_else(|| NotScalar(format!("unknown variable `{name}`")))?;
                Ok((self.b.use_var(var), ty))
            }
            ExprKind::Unary(op, inner) => {
                let (v, ty) = self.expr(inner)?;
                match op {
                    UnOp::Neg => match ty {
                        ScalarTy::I64 => Ok((self.b.ins().ineg(v), ScalarTy::I64)),
                        ScalarTy::F64 => Ok((self.b.ins().fneg(v), ScalarTy::F64)),
                        ScalarTy::Bool => bail("cannot negate a boolean"),
                    },
                    UnOp::Not => match ty {
                        ScalarTy::Bool => {
                            Ok((self.b.ins().icmp_imm_s(IntCC::Equal, v, 0), ScalarTy::Bool))
                        }
                        _ => bail("`!` expects a boolean"),
                    },
                }
            }
            ExprKind::Binary(op, l, r) => {
                let (lv, lty) = self.expr(l)?;
                let (rv, rty) = self.expr(r)?;
                self.binop(*op, lv, lty, rv, rty)
            }
            ExprKind::Logic(op, l, r) => self.lower_logic(*op, l, r),
            ExprKind::Call(name, args) => self.lower_call(name, args),
            ExprKind::Range { .. } => bail("range is not a scalar value"),
            ExprKind::Str(_)
            | ExprKind::Array(_)
            | ExprKind::Index(_, _)
            | ExprKind::Member(_, _)
            | ExprKind::Method(_, _, _)
            | ExprKind::StructLit { .. } => bail("aggregate expression not scalar"),
        }
    }

    /// Emit an arithmetic/comparison op, promoting int->float when either side is
    /// float — exactly the rule in `value::binop`.
    fn binop(
        &mut self,
        op: BinOp,
        mut lv: CraneliftValue,
        lty: ScalarTy,
        mut rv: CraneliftValue,
        rty: ScalarTy,
    ) -> Result<(CraneliftValue, ScalarTy), NotScalar> {
        // Booleans only participate in ==/!=; everything else needs numbers.
        let float = lty == ScalarTy::F64 || rty == ScalarTy::F64;
        if float {
            if lty != ScalarTy::F64 {
                lv = self.coerce(lv, lty, ScalarTy::F64)?;
            }
            if rty != ScalarTy::F64 {
                rv = self.coerce(rv, rty, ScalarTy::F64)?;
            }
        } else if lty == ScalarTy::Bool || rty == ScalarTy::Bool {
            // Comparisons on bools are allowed; arithmetic isn't.
            if !matches!(op, BinOp::Eq | BinOp::Ne) {
                return bail("arithmetic on booleans");
            }
        }

        let ins = self.b.ins();
        let out = match op {
            BinOp::Add if float => (ins.fadd(lv, rv), ScalarTy::F64),
            BinOp::Sub if float => (ins.fsub(lv, rv), ScalarTy::F64),
            BinOp::Mul if float => (ins.fmul(lv, rv), ScalarTy::F64),
            BinOp::Div if float => (ins.fdiv(lv, rv), ScalarTy::F64),
            BinOp::Rem if float => return bail("float `%` needs a libcall (fall back)"),
            BinOp::Add => (ins.iadd(lv, rv), ScalarTy::I64),
            BinOp::Sub => (ins.isub(lv, rv), ScalarTy::I64),
            BinOp::Mul => (ins.imul(lv, rv), ScalarTy::I64),
            BinOp::Div => (ins.sdiv(lv, rv), ScalarTy::I64),
            BinOp::Rem => (ins.srem(lv, rv), ScalarTy::I64),
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let v = if float {
                    ins.fcmp(float_cc(op), lv, rv)
                } else {
                    ins.icmp(int_cc(op), lv, rv)
                };
                (v, ScalarTy::Bool)
            }
        };
        Ok(out)
    }

    fn lower_logic(
        &mut self,
        op: LogicOp,
        l: &Expr,
        r: &Expr,
    ) -> Result<(CraneliftValue, ScalarTy), NotScalar> {
        // Short-circuit via a result variable written on each path.
        let logic_name = format!("__logic{}", self.next_var);
        let res = self.fresh(&logic_name, ScalarTy::Bool);
        let lc = self.cond_value(l)?;
        let rhs_b = self.b.create_block();
        let short_b = self.b.create_block();
        let merge_b = self.b.create_block();
        match op {
            // a && b: if a -> eval b else result=false
            LogicOp::And => self.b.ins().brif(lc, rhs_b, &[], short_b, &[]),
            // a || b: if a -> result=true else eval b
            LogicOp::Or => self.b.ins().brif(lc, short_b, &[], rhs_b, &[]),
        };

        self.b.switch_to_block(rhs_b);
        self.b.seal_block(rhs_b);
        let rv = self.cond_value(r)?;
        self.b.def_var(res, rv);
        self.b.ins().jump(merge_b, &[]);

        self.b.switch_to_block(short_b);
        self.b.seal_block(short_b);
        let konst = matches!(op, LogicOp::Or) as i64;
        let kv = self.b.ins().iconst(types::I8, konst);
        self.b.def_var(res, kv);
        self.b.ins().jump(merge_b, &[]);

        self.b.switch_to_block(merge_b);
        self.b.seal_block(merge_b);
        Ok((self.b.use_var(res), ScalarTy::Bool))
    }

    fn lower_call(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<(CraneliftValue, ScalarTy), NotScalar> {
        // Scalar stdlib math that maps to a single CLIF instruction.
        if let Some(res) = self.lower_intrinsic(name, args)? {
            return Ok(res);
        }
        // Otherwise it must be another compiled function.
        let &(id, arity) = self
            .ids
            .get(name)
            .ok_or_else(|| NotScalar(format!("call to non-native `{name}`")))?;
        if arity != args.len() {
            return bail(format!("arity mismatch calling `{name}`"));
        }
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            let (v, ty) = self.expr(a)?;
            vals.push(self.coerce(v, ty, ScalarTy::F64)?);
        }
        let fref = self.module.declare_func_in_func(id, self.b.func);
        let call = self.b.ins().call(fref, &vals);
        let res = self.b.inst_results(call)[0];
        Ok((res, ScalarTy::F64))
    }

    /// Lower a scalar math builtin to a CLIF instruction, all in f64. Returns
    /// `Ok(None)` if `name` isn't a supported intrinsic.
    fn lower_intrinsic(
        &mut self,
        name: &str,
        args: &[Expr],
    ) -> Result<Option<(CraneliftValue, ScalarTy)>, NotScalar> {
        let want = |n: usize| -> Result<(), NotScalar> {
            if args.len() == n {
                Ok(())
            } else {
                bail(format!("`{name}` expects {n} args"))
            }
        };
        macro_rules! f {
            ($e:expr) => {{
                let (v, ty) = self.expr(&args[0])?;
                let v = self.coerce(v, ty, ScalarTy::F64)?;
                let _ = &v;
                Some(($e(self, v), ScalarTy::F64))
            }};
        }
        let out = match name {
            "sqrt" => {
                want(1)?;
                f!(|s: &mut Self, v| s.b.ins().sqrt(v))
            }
            "abs" => {
                want(1)?;
                f!(|s: &mut Self, v| s.b.ins().fabs(v))
            }
            "floor" => {
                want(1)?;
                f!(|s: &mut Self, v| s.b.ins().floor(v))
            }
            "ceil" => {
                want(1)?;
                f!(|s: &mut Self, v| s.b.ins().ceil(v))
            }
            "round" => {
                want(1)?;
                f!(|s: &mut Self, v| s.b.ins().nearest(v))
            }
            "min" | "max" => {
                want(2)?;
                let (a, aty) = self.expr(&args[0])?;
                let a = self.coerce(a, aty, ScalarTy::F64)?;
                let (b, bty) = self.expr(&args[1])?;
                let b = self.coerce(b, bty, ScalarTy::F64)?;
                let v = if name == "min" {
                    self.b.ins().fmin(a, b)
                } else {
                    self.b.ins().fmax(a, b)
                };
                Some((v, ScalarTy::F64))
            }
            _ => None,
        };
        Ok(out)
    }
}

fn int_cc(op: BinOp) -> IntCC {
    match op {
        BinOp::Eq => IntCC::Equal,
        BinOp::Ne => IntCC::NotEqual,
        BinOp::Lt => IntCC::SignedLessThan,
        BinOp::Le => IntCC::SignedLessThanOrEqual,
        BinOp::Gt => IntCC::SignedGreaterThan,
        BinOp::Ge => IntCC::SignedGreaterThanOrEqual,
        _ => unreachable!("int_cc on non-comparison"),
    }
}

fn float_cc(op: BinOp) -> FloatCC {
    match op {
        BinOp::Eq => FloatCC::Equal,
        BinOp::Ne => FloatCC::NotEqual,
        BinOp::Lt => FloatCC::LessThan,
        BinOp::Le => FloatCC::LessThanOrEqual,
        BinOp::Gt => FloatCC::GreaterThan,
        BinOp::Ge => FloatCC::GreaterThanOrEqual,
        _ => unreachable!("float_cc on non-comparison"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cranelift_jits_and_runs_native_code() {
        assert_eq!(smoke_const(42), 42);
        assert_eq!(smoke_const(-7), -7);
    }
    #[test]
    fn jit_matches_native_f64_arithmetic_bit_for_bit() {
        // The JITed native code must equal the same computation done in Rust — the
        // three-engine differential oracle (interp == vm == jit) rests on this.
        for &x in &[0.0_f64, 1.5, -2.25, 3.14159, 100.0, -0.5] {
            let want = x * x + 2.0 * x + 1.0;
            assert_eq!(smoke_poly(x), want, "poly disagreed at x={x}");
        }
    }

    // --- AST -> native lowering: the real M14 oracle -----------------------

    use crate::interpreter::run_str;
    use crate::parser::parse_program;

    /// Run `defs` + `print <call>;` through the tree-walker and read the numeric
    /// result — the source of truth the native code must reproduce.
    fn interp_result(defs: &str, call: &str) -> f64 {
        let src = format!("{defs}\nprint {call};");
        let out = run_str(&src).expect("interpreter ran");
        out.last().expect("printed a value").parse::<f64>().expect("numeric result")
    }

    /// Compile `defs` natively and call `name(args)`.
    fn jit_result(defs: &str, name: &str, args: &[f64]) -> f64 {
        let (prog, diags) = parse_program(defs);
        assert!(!diags.iter().any(|d| d.is_error()), "parse errors: {diags:?}");
        let m = compile_program(&prog).expect("program is scalar-compilable");
        m.call(name, args).expect("call succeeds")
    }

    #[test]
    fn native_code_agrees_with_the_tree_walker() {
        struct Case {
            defs: &'static str,
            name: &'static str,
            args: &'static [f64],
            call: &'static str,
        }
        let cases = &[
            Case { defs: "fn poly(x) { return x*x + 2.0*x + 1.0; }", name: "poly", args: &[3.0], call: "poly(3.0)" },
            Case { defs: "fn fact(n) { if n <= 1.0 { return 1.0; } return n * fact(n - 1.0); }", name: "fact", args: &[6.0], call: "fact(6.0)" },
            Case { defs: "fn fib(n) { if n < 2.0 { return n; } return fib(n-1.0) + fib(n-2.0); }", name: "fib", args: &[12.0], call: "fib(12.0)" },
            Case { defs: "fn absdiff(a, b) { if a > b { return a - b; } return b - a; }", name: "absdiff", args: &[3.0, 7.0], call: "absdiff(3.0, 7.0)" },
            Case { defs: "fn clamp(x, lo, hi) { return min(max(x, lo), hi); }", name: "clamp", args: &[5.0, 0.0, 3.0], call: "clamp(5.0, 0.0, 3.0)" },
            Case { defs: "fn hyp(a, b) { return sqrt(a*a + b*b); }", name: "hyp", args: &[3.0, 4.0], call: "hyp(3.0, 4.0)" },
            Case { defs: "fn sumrange(x) { s = 0; for i in 0..10 { s = s + i; } return s; }", name: "sumrange", args: &[0.0], call: "sumrange(0.0)" },
            Case { defs: "fn countup(x) { i = 0; c = 0; while i < 5 { c = c + 2; i = i + 1; } return c; }", name: "countup", args: &[0.0], call: "countup(0.0)" },
            Case { defs: "fn band(a, b) { if a > 0.0 && b > 0.0 { return 1.0; } return 0.0; }", name: "band", args: &[3.0, -1.0], call: "band(3.0, -1.0)" },
            Case { defs: "fn bor(a, b) { if a > 0.0 || b > 0.0 { return 1.0; } return 0.0; }", name: "bor", args: &[-3.0, 4.0], call: "bor(-3.0, 4.0)" },
            Case { defs: "fn compound(x) { r = x; r += 5.0; r *= 2.0; r -= 1.0; return r; }", name: "compound", args: &[1.0], call: "compound(1.0)" },
        ];
        for c in cases {
            let native = jit_result(c.defs, c.name, c.args);
            let tree = interp_result(c.defs, c.call);
            assert_eq!(native, tree, "engines disagree on `{}`: native={native}, tree={tree}", c.call);
        }
    }

    #[test]
    fn native_recursion_and_calls_between_functions() {
        // fib defined via a helper it calls — proves cross-function native calls.
        let defs = "fn add(a, b) { return a + b; }
                    fn fib(n) { if n < 2.0 { return n; } return add(fib(n-1.0), fib(n-2.0)); }";
        assert_eq!(jit_result(defs, "fib", &[15.0]), 610.0);
        assert_eq!(jit_result(defs, "fib", &[15.0]), interp_result(defs, "fib(15.0)"));
    }

    /// Not run by default (timing). `cargo test -p ruspy --features jit --
    /// --ignored --nocapture native_speedup` prints the native-vs-interpreter ratio.
    #[test]
    #[ignore = "timing benchmark; run explicitly"]
    fn native_speedup_over_the_interpreter() {
        use std::time::Instant;
        let defs = "fn fib(n) { if n < 2.0 { return n; } return fib(n-1.0) + fib(n-2.0); }";
        let (prog, _) = parse_program(defs);
        let m = compile_program(&prog).expect("compiles");

        // Native.
        let t0 = Instant::now();
        let mut acc = 0.0;
        for _ in 0..50 {
            acc += m.call("fib", &[25.0]).unwrap();
        }
        let native = t0.elapsed();

        // Interpreter (one call — it's ~1e6 slower, so don't loop 50x).
        let t1 = Instant::now();
        let interp = interp_result(defs, "fib(25.0)");
        let tree = t1.elapsed();

        assert_eq!(acc / 50.0, interp, "value mismatch");
        let native_per = native.as_secs_f64() / 50.0;
        eprintln!(
            "fib(25): native {:>10.3?}/call | interpreter {:>10.3?}/call | speedup ~{:.0}x",
            std::time::Duration::from_secs_f64(native_per),
            tree,
            tree.as_secs_f64() / native_per
        );
    }

    #[test]
    fn non_scalar_programs_report_notscalar_for_fallback() {
        // Arrays / strings / print aren't lowered — the caller falls back to the VM.
        let (p1, _) = parse_program("fn f(a) { xs = [1, 2, 3]; return xs[0]; }");
        assert!(compile_program(&p1).is_err());
        let (p2, _) = parse_program("fn g(a) { print a; return a; }");
        assert!(compile_program(&p2).is_err());
    }

    #[test]
    fn emits_a_native_object_file_for_the_host() {
        let (prog, diags) = parse_program(
            "fn fib(n) { if n < 2.0 { return n; } return fib(n-1.0) + fib(n-2.0); }
             fn main() { return fib(10.0); }",
        );
        assert!(!diags.iter().any(|d| d.is_error()));
        let (bytes, names) = compile_object(&prog, "ruspy_program").expect("object emitted");

        assert!(bytes.len() > 64, "object suspiciously small");
        assert!(names.contains(&"fib".to_string()) && names.contains(&"main".to_string()));

        // Sanity-check the container magic for this platform's object format.
        let magic = &bytes[..4];
        let ok = magic == [0xCF, 0xFA, 0xED, 0xFE]   // Mach-O 64 (LE)
            || magic == [0xFE, 0xED, 0xFA, 0xCF]     // Mach-O 64 (BE)
            || magic == [0x7F, b'E', b'L', b'F']     // ELF
            || &bytes[..2] == [0x64, 0x86];          // COFF x86-64
        assert!(ok, "unrecognized object magic: {:02X?}", magic);
    }
}
