# Ruspy → a full-fledged compiled language — Roadmap

Goal (user): make ruspy a proper/complete programming language with a real compiler
that builds platform-specific, hard-to-reverse-engineer binaries (MQL-style), runs at
native speed, and ships a rich trading-strategy stdlib.

## Honest scope framing (read first)

- **"Sub-nanosecond execution" is physically impossible** and won't be promised. One CPU
  instruction at ~4 GHz is ~0.25 ns; a single memory load is ~1–100 ns. The achievable and
  correct target is **native machine-code speed** — a compiled strategy tick in the
  **tens-of-nanoseconds to low-microseconds** range, i.e. C/Rust-class, versus today's
  tree-walking interpreter. The real backend decision (Cranelift AOT+JIT vs LLVM vs custom
  VM) comes from the research team.

- **"Hard to reverse-engineer, like MQL"**: MQL5 compiles to bytecode (`.ex5`) for a
  proprietary VM — that indirection, not magic, is its protection. The realistic plan is a
  **two-track backend**: (1) a fast **native** path for the author's own backtests, and
  (2) a **protected bytecode/VM format** (call it `.rux`) for distribution, with honest
  obfuscation (VM indirection, symbol stripping, const/string encryption, control-flow
  flattening). No binary is *unbreakable*; the goal is raising cost, and the doc will say
  exactly what that buys.

- **"Complete language"**: today ruspy has no functions, loops, booleans, unary minus,
  else-if, `%`, `&&`/`||`, arrays, or methods; type annotations are parsed then ignored;
  and lex/type errors **panic** (RUSPY_SCHEMA.md §7–§8). Closing those is the first,
  highest-leverage work and makes ruspy visibly more real before the heavy backend.

## Done

- [x] **M0 — Workspace member.** ruspy now builds + tests under the workspace (was vendored
  but unbuilt). 13 ruspy tests run under `cargo test`; workspace green.

## Pipeline (target)

```
source(.ruspy)
  → lexer (with spans)         [today: no positions, panics]
  → parser → AST (with spans)  [today: partial, no fns/loops]
  → type checker → typed AST   [today: annotations ignored, panics]
  → IR
  → native backend (Cranelift AOT+JIT)      → platform binary   [fast path]
  → bytecode + VM (.rux, obfuscated)         → protected artifact [distribution]
  + trading stdlib / prelude bound to the host backtest engine (crates/backtest_rs)
```

## Milestones

_Pending the research-team synthesis (arxiv + web). This section is filled from the
`ruspy-compiler-research` workflow's phased plan, then executed one milestone at a time,
each committed + tested, existing tests kept green._

See also: `RUSPY_SCHEMA.md` (current language contract) and `RUSPY_TODO.md` (live checklist).
