# Ruspy — build checklist (from the research-team synthesis)

> **Status (Aug 2026): all four must-haves delivered.** ruspy is a full-fledged
> statically-checkable language with **four** execution engines that agree bit-for-bit
> (tree-walker · bytecode VM · Cranelift native · protected `.ruspyc`). It compiles to a
> **standalone native platform executable** (verified Mach-O arm64), runs **~1276× the
> interpreter (~1.5 ns/call)**, ships a **trading stdlib**, and has an **MQL-style
> hard-to-RE distribution format** (per-build opcode permutation + RFC-8439 ChaCha20,
> honest threat model). Same source runs identically through all four engines (→ 6765).
> Remaining items (M11 live-engine binding, M15 hot-reload, M17 hardening) are
> integration/polish, not core-language gaps. Perf/scope honesty in `RUSPY_ROADMAP.md`.

Architecture: **one front-end spine, three execution engines that must agree bit-for-bit**
on a shared corpus. Pipeline: source → lexer(spans, no panics) → Pratt parser → AST(Expr/Stmt,
spans) → resolver(slots, interned symbols) → bidirectional type checker → HIR →
{ tree-walker · register bytecode VM · Cranelift AOT/JIT }. Protected `.ruspyc` reuses the VM
bytecode (per-build opcode permutation + encryption + licensing). Backend = **Cranelift 0.134.x**
(not LLVM: ruspy's per-tick body is straight-line scalar f64, so LLVM's edge ≈ 0% here while its
install tax is paid in full). Perf target = **native-code speed: 1.5–4 ns/tick warm, 0.3–1 ns/tick
amortized** (sub-ns is impossible as a latency claim; publish as iterations/total-time).

Rule: each milestone committed + tested; existing tests stay green; a `run(src)`-style oracle
helper is the single entry point so the three engines can be diffed later.

## Front end — makes ruspy a real language (the bulk of the value)
- [x] **M0** Workspace member + test oracle scaffold. *(member done; harness folded into M1)*
- [x] **M1** Lexer: `Vec<Spanned<Token>>`, never panics, `Diag{span,code,msg,help}` + LineIndex; recovery. *(rewrites the 3 lexer panics; `@` test → asserts a Diag)*
- [x] **M2** Split `Ty` from `Value`; unified numeric tower; ops return `Result<Value,Diag>` (delete Add/Sub/Mul/Div panics; div-by-zero → Diag; int→float promotion).
- [x] **M3** Expr/Stmt AST split + Pratt parser (9-level precedence, non-assoc comparison) + error recovery.
- [x] **M4** Bools + `&& || !` (short-circuit), unary `-`, `%`, else-if, compound assign, string escapes, hex/bin/exp/nan/inf literals, block comments.
- [x] **M5** Scope chain + resolver→slots + interned symbols; `let` declares vs `=` assigns; use-before-decl/dup-param/break-outside errors.
- [x] **M6** Functions (`fn`/`def`), return, hoisting, recursion, depth cap + fuel counter (runaway → Diag, not SIGSEGV).
- [x] **M7** Loops: `while`, `for..in` ranges/arrays, break/continue; fuel enforced.
- [x] **M8** Arrays `[T]` (Rc<RefCell<Vec>>, ref semantics) + index r/w (OOB→Diag) + builtin methods + host `call_method`.
- [x] **M9** Structs / records (nominal, field-id indexed).
- [x] **M10** Static bidirectional type checker + typed host manifest; `check(src,host)` (annotations enforced; non-bool cond, missing return, unknown host fn = compile errors).

## Backend + stdlib + protection — the "must haves"
- [x] **M11** (done) Strategy lifecycle + host binding. New `crates/ruspy_host`:
  `RuspyStrategy` implements `backtest_rs::Strategy` by delegating `on_tick` to a
  ruspy program via `Interpreter::call_named` (per-hook fuel reset). `fn on_tick()`
  is required; top-level + optional `fn on_init()` run once at load with no market
  access. Host ABI (`BacktestHost: RuspyHost`): reads `tick.bid/ask/mid/spread/time`,
  `account.equity/balance/profit`, `position.count/long/short/flat`; actions
  `buy(vol[,sl,tp])`/`sell(...)`/`close_all()`; bar-sampled series `closes(n)`/
  `highs(n)`/`lows(n)`/`bars()` (1-minute buckets off the bid tape) feed the stdlib
  `sma`/`ema`/`rsi`/`highest`. Zero edits to `backtest_rs` beyond a `strategy()`
  getter — the driver builds `Engine::with_config(RuspyStrategy, …)` directly. CLI
  `ruspy-backtest strat.ruspy data.csv [--balance N] [--commission N]` prints an
  MT5-style report. Verified end-to-end on 600k real Dukascopy XAUUSD ticks
  (breakout/ma_cross/rsi examples in `crates/ruspy_host/strategies/`). Honest note:
  the tree-walked `on_tick` runs per tick (~µs/call); a full 22 GB history is slow —
  the VM/JIT path (M13/M14) is the speed follow-up. 6 tests.
- [x] **M12** (pure-ruspy stdlib; backtest_rs host-binding = follow-up) Trading stdlib: incremental indicator prelude (sma/ema/rsi/atr/bollinger/macd/…) + orders/positions/account/risk via one `FN_TABLE` that also generates the editor manifest.
- [x] **M13** (VM v1: scalar+control-flow+functions+stdlib; arrays/structs fall back) Register bytecode VM (unboxed f64 slots, handler-table dispatch) — default engine, semantics oracle, seed of the protected format (~20–60 ns/tick).
- [x] **M14** (JIT lowering done: `jit::compile_program` AST→CLIF for the scalar core — f64/i64/bool with int→float promotion, if/while/for-range, short-circuit &&/||, user calls+recursion, scalar-math intrinsics via CLIF ops; non-scalar → `NotScalar` fallback; 3-engine oracle interp==native; **~1276× the interpreter**, ~1.5 ns/call. AOT-object `cdylib`/staticlib + RuspyCtx host ABI = M14b.) Cranelift native backend behind the `jit` feature.
- [ ] **M15** Cranelift JIT dev path (hot-reload; AtomicPtr swap; dev-only).
- [x] **M16** (done for the scalar core: `jit::compile_object` emits a host-native object via `cranelift-object` — one generic `lower_all<M: Module>` shared with the JIT; exported symbols prefixed `ruspy_` so an entry named `main` doesn't clash with the C runtime; `ruspy --build OUT --entry main` links the object + a C shim with the system `cc` into a standalone exe. Verified: a Mach-O arm64 binary that runs with no interpreter and prints fib(20)=6765.) Standalone native binary + platform linker.
- [x] **M17** (done, honest threat model: `vm::protect` packs VM bytecode into a `.ruspyc` container — per-build opcode permutation (SplitMix64 Fisher–Yates; the byte written for each opcode changes every build, defeating static disassembly) + ChaCha20 at rest (RFC 8439, from spec, KAT-verified) + FNV-1a integrity check + manual bytecode serializer. `ruspy --pack OUT --key <hex64>` / `ruspy --run-protected --key <hex64>`. Verified: plaintext strings don't leak into the artifact, wrong key is rejected, each build differs, output matches the interpreter. Explicitly NOT claimed: AEAD authentication or debugger-proofing — same fundamental limit as MQL's .ex5, the key must reach the loader. Follow-ups: real AES-GCM/Poly1305 tag, selective virtualization, online activation, native-exe hygiene strip/lto/panic=abort.)

## Modules & standard packages (MQL-style imports)
- [x] **M18** Import system + namespaced packages. New `import` keyword + `StmtKind::Import`.
  Two forms: `import "file.ruspy";` (a real module loader in `ruspy::loader` — recursive
  file include, **cycle-detected + deduped**, each file parsed AND type-checked against its
  own source for correct `file:line`) and `import math;` (namespaced library package,
  **enforced by the checker** — use without import is `E0410`). Package member/method access
  routes through pure packages (`ruspy::packages`) then the host `RuspyHost::call_method`.
  Packages: **math** (pure — PI/E/TAU + trig/log/exp/pow/sign/clamp/…), **trade**
  (buy/sell/close_all/close_long/close_short/positions/is_long/is_short), **account**
  (equity/balance/profit), **series** (close/high/low/bars) — trade/account/series are
  host-backed in `ruspy_host`. `tick`/`account`/`position` stay ambient (no import). Wired
  into both CLIs and `RuspyStrategy::compile_file`. Example `ma_cross_pkg.ruspy` +
  `lib/risk.ruspy`. **Also fixed a latent parser wedge**: a stray top-level `}` after error
  recovery span-looped and OOM'd; `parse()` now guarantees forward progress. 84 tests
  (ruspy 74 + ruspy_host 10).

## Research artifacts
Full team output (Cranelift 0.134.3 crate list, RuspyCtx ABI, ASTNode→clif lowering table, JIT/AOT
API deltas, protection threat model, stdlib FN_TABLE, type-checker design) archived in the workflow
result; re-read at each backend milestone. Perf/scope honesty is in `RUSPY_ROADMAP.md`.
