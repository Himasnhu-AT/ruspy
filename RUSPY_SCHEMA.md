<!-- Language schema for Ruspy, transcribed from the actual implementation in
     src/{lexer,parser,interpreter,types}/mod.rs. Written as the contract an editor
     is built against: token classes, grammar, AST, diagnostics, run pipeline.
     Where the docs and the code disagree, the CODE wins and the gap is flagged. -->

# Ruspy — Language Schema (for editor authors)

**What Ruspy is:** a small Python-flavoured language with Rust-style type annotations,
implemented as a **tree-walking interpreter** in `crates/ruspy` (~1,200 lines). File
extension **`.ruspy`**.

> ⚠️ **It is an interpreter, not a compiler.** `compiler-design.md` describes an
> *aspiration* (IR, optimization, machine-code generation, ownership, pattern matching).
> None of that exists. The real pipeline is source → tokens → AST → walk. An editor
> should say **“Run”**, not “Compile” — or show a Check/Run split (see §7).

---

## 1. Pipeline

```
source (.ruspy)
   │
   ├─ Lexer::new(&src) ──────────── src/lexer/mod.rs
   │    get_next_token() → Token          (single-pass, char iterator, no positions)
   │
   ├─ Parser::new(lexer) ─────────── src/parser/mod.rs
   │    parse() → Result<Vec<ASTNode>, String>   (recursive descent, 1-token lookahead)
   │
   └─ Interpreter::interpret(&ast, &mut host) ── src/interpreter/mod.rs
        → Result<RuspyType, String>       (walks the tree, HashMap<String, RuspyType> env)
```

There is **no separate semantic/type-check pass**. Type errors surface at run time —
which matters for an editor: you cannot offer real pre-run type diagnostics today
without adding a pass (§8).

---

## 2. Token schema — `enum Token`

Use these as your syntax-highlighting classes.

| Class | Tokens | Suggested style |
|---|---|---|
| **Literal — number** | `Number(i64)`, `FloatLiteral(f64)` | numeric color |
| **Literal — string** | `StringLiteral(String)` | string color |
| **Identifier** | `Identifier(String)` | plain text |
| **Type keyword** | `TypeInt` `TypeInt32` `TypeInt64` `TypeFloat` `TypeFloat32` `TypeFloat64` `TypeChar` `TypeStr` `TypeStr8` `TypeStr32` `TypeStr64` | type color (italic) |
| **Keyword** | `Print` `If` `Else` | keyword color (bold) |
| **Operator — arithmetic** | `Plus` `Minus` `Asterisk` `Slash` | operator color |
| **Operator — comparison** | `Eq` `NotEq` `Gt` `Lt` `GtEq` `LtEq` | operator color |
| **Assignment** | `Assign` (`=`) | operator color |
| **Punctuation** | `LParen` `RParen` `LBrace` `RBrace` `Comma` `Dot` `Colon` `Semicolon` | muted |
| **End** | `EOF` | — |

**Reserved words** (exact list the lexer recognizes — everything else is an identifier):

```
int  int32  int64  float  float32  float64  char  str  str8  str32  str64
print  if  else
```

**Comments:** `// line comment` only. **No** `/* block */`, no `#`.
**Strings:** double-quoted `"…"`. **No escape sequences** — `\n` is a literal backslash+n.
**Numbers:** `123` → `Number`; `3.14` → `FloatLiteral` (only when a digit follows the dot,
so `x.bid` correctly lexes as member access). No hex/binary/underscores/exponents.
**Whitespace/newlines:** insignificant. Statements end at `;`.

---

## 3. Grammar (EBNF, as actually implemented)

```ebnf
program       = { statement } EOF ;

statement     = print_stmt
              | if_stmt
              | block
              | typed_decl          (* ident ':' *)
              | untyped_decl        (* ident '=' *)
              | expr_stmt ;

print_stmt    = "print" comparison ";" ;
if_stmt       = "if" comparison block [ "else" block ] ;
block         = "{" { statement } "}" ;
typed_decl    = IDENT ":" type "=" comparison ";" ;
untyped_decl  = IDENT "=" comparison ";" ;
expr_stmt     = comparison ";" ;

comparison    = expr { ( "==" | "!=" | ">" | "<" | ">=" | "<=" ) expr } ;
expr          = term { ( "+" | "-" ) term } ;
term          = factor { ( "*" | "/" ) factor } ;
factor        = primary { call | member } ;
call          = "(" [ comparison { "," comparison } ] ")" ;   (* identifiers only *)
member        = "." IDENT ;
primary       = NUMBER | FLOAT | STRING | IDENT | "(" comparison ")" ;

type          = "int" | "int32" | "int64"
              | "float" | "float32" | "float64"
              | "str" | "char" ;        (* NOTE: str8/str32/str64 lex but do NOT parse *)
```

**Precedence** (loosest → tightest): comparison → `+ -` → `* /` → postfix `()` `.` → primary.
All binary operators are **left-associative**. Parentheses group.

**Statement dispatch** uses one token of lookahead (`peek_next`): an `Identifier`
followed by `:` is a typed declaration, followed by `=` is an untyped one, otherwise it
is an expression statement.

---

## 4. AST schema — `enum ASTNode`

Serde-serializable (`Serialize`/`Deserialize`), so an editor can render a live tree view.

| Node | Shape | Notes |
|---|---|---|
| `Number` | `i64` | integer literal |
| `FloatLiteral` | `f64` | float literal |
| `StringLiteral` | `String` | string literal |
| `Identifier` | `String` | variable reference |
| `BinaryOp` | `(Box<ASTNode>, Token, Box<ASTNode>)` | operator kept as a `Token` |
| `VarAssign` | `(String, Box<ASTNode>)` | `x = expr;` |
| `TypedVarAssign` | `(String, RuspyType, Box<ASTNode>)` | `x: int = expr;` |
| `Print` | `Box<ASTNode>` | `print expr;` |
| `Block` | `Vec<ASTNode>` | `{ … }` |
| `If` | `(cond, then, Option<else>)` | branches are `Block`s |
| `FunctionCall` | `(String, Vec<ASTNode>)` | resolved by the **host**, not by Ruspy |
| `MemberAccess` | `(Box<ASTNode>, String)` | `obj.field`, resolved by the **host** |

---

## 5. Value schema — `enum RuspyType`

`Int(i32)` · `Int32(i32)` · `Int64(i64)` · `Float(f64)` · `Float32(f32)` · `Float64(f64)` ·
`Str(String)` · `Char(char)` · `Bool(bool)` · `Object(String)` · `Void`

**Runtime rules that will bite users — surface them in the editor:**

- Literals always evaluate to the **widest** type: an integer literal is `Int64`, a float
  literal is `Float64`, regardless of the annotation you wrote.
- **The type annotation is not enforced.** `TypedVarAssign` stores the value and ignores
  the declared type (`// Type check could be improved` — interpreter/mod.rs).
  `x: int = "hello";` runs fine.
- Arithmetic requires **exactly matching** variants. `Int64 + Float64` **panics**
  (`Incompatible types for addition`) — it does not coerce. Comparisons *do* coerce
  (everything numeric goes through `f64`).
- `+` on two `Str` concatenates. `-`, `*`, `/` on strings panic.
- Division by zero **panics** (integer and float alike).
- `if` requires a **`Bool`**, and there are **no boolean literals** — the only way to get
  one is a comparison. `if x { }` is a runtime error; write `if x == 1 { }`.

---

## 6. Host interface — how Ruspy touches the trading engine

```rust
pub trait RuspyHost {
    fn call_function(&mut self, name: &str, args: Vec<RuspyType>) -> Result<RuspyType, String>;
    fn get_member(&mut self, obj_id: &str, member: &str) -> Result<RuspyType, String>;
}
```

Ruspy itself has **no built-in functions at all** — not even `len`. Every `foo(...)` is
delegated to the host; every `obj.field` requires the receiver to be `RuspyType::Object(id)`
and is delegated too. `Interpreter::set_variable("tick", RuspyType::Object("tick".into()))`
is how a strategy gets its input.

**For the editor this is the key extension point:** autocomplete for functions and members
cannot be derived from the language — it must come from the host's registry. Design the
editor to take a host manifest:

```json
{
  "objects": { "tick": ["bid", "ask", "time"], "account": ["balance", "equity"] },
  "functions": [
    { "name": "buy",  "args": ["volume"], "returns": "Void" },
    { "name": "sell", "args": ["volume"], "returns": "Void" }
  ]
}
```

`DefaultHost` rejects everything, so a bare editor should ship a manifest or clearly show
“no host connected → `buy(...)` will fail”.

---

## 7. Diagnostics — what an editor can actually show

This is the sharpest constraint on editor design, so read it before drawing the error UI.

| Stage | Failure mode | Recoverable? | Has line/col? |
|---|---|---|---|
| Lexer | unexpected char, unterminated string | **panics** | ❌ no position tracked |
| Parser | `Expected token X, found Y` | ✅ `Err(String)` | ❌ token only, no span |
| Parser | `Invalid type: …`, `Unexpected token in factor: …`, `Method calls not fully supported` | ✅ `Err(String)` | ❌ |
| Interpreter | `Undefined variable: x`, `Condition must evaluate to boolean`, `Function 'f' not found` | ✅ `Err(String)` | ❌ |
| Types | incompatible arithmetic, division by zero | **panics** | ❌ |

**Consequences for the UI, stated plainly:**

1. **No squiggles under the offending token** — the parser stops at the first error and
   reports it as a bare string with no span. Show errors in a **console/problems panel**,
   not inline, until §8 lands.
2. **One error at a time.** No error recovery, so there is no “5 problems” list.
3. **A panic kills the process.** If the editor runs the interpreter in-process, a
   division by zero takes the app down. Run it in a **worker thread with
   `catch_unwind`**, or a subprocess. Design the Run panel assuming a crash is possible
   and render it as a red "Interpreter crashed" state rather than a hang.
4. Because there is no type-check pass, **“Check” and “Run” are the same action** today.
   If you want a Check button that doesn't execute, it can only do lex+parse.

---

## 8. Gaps worth knowing before you design around them

Things `Readme.md` / `ruspy.md` / `compiler-design.md` imply but the code does **not** have:

| Claimed | Reality |
|---|---|
| `def func_name(args) { }` (in `ruspy.md`) | **No `def` token, no function-definition parsing.** User-defined functions do not exist. |
| Ownership, borrowing, pattern matching, IR, optimizer, codegen | Aspirational only. |
| `str8` / `str32` / `str64` types | Lexed but `parse_type` rejects them → `Invalid type`. |
| Type safety from annotations | Annotations are parsed and then ignored. |
| Loops (`while` / `for`) | Do not exist. |
| Booleans `true` / `false` | No literals; only produced by comparisons. |
| `else if` chains | Only `else { block }` — the else branch must be a block. |
| Method calls `obj.method()` | Explicitly rejected: *"Method calls not fully supported in this parser version yet"*. |
| Unary minus (`-5`) | Not in `factor` — `-5` fails to parse. Write `0 - 5`. |
| Modulo, logical `&&`/`\|\|`/`!` | Absent. |

**Highest-value additions if the editor should feel real** (roughly ascending cost):
1. **Positions in the lexer** (line/col per token) → inline squiggles. *Small change, huge UX payoff.*
2. **Replace lexer/type panics with `Result`** → the editor stops needing crash isolation.
3. Unary minus, `true`/`false`, `else if`, `%`.
4. Multiple diagnostics via error recovery (skip to next `;`).
5. `while` loops, then `def` functions.

---

## 9. Reference program

```ruspy
// Sample Ruspy program demonstrating variables, printing and BODMAS calculations

x: int64 = 10;
y: int64 = 5;

result: int64 = (x + y) * 2 / (5 - 2);
print result;            // 10

complex: int64 = x * (y + 3) - 2;
print complex;

message: str = "Hello, Ruspy!";
print message;

if x > y {
    print "x wins";
} else {
    print "y wins";
}
```

Run it: `cargo run -p ruspy -- examples/sample.ruspy` (`-d` for debug logging).
Output goes through the `log` crate at `info` level — the editor should capture
`log`/stdout rather than expecting a return value.

> **Note:** `ruspy` is vendored and is **not currently a workspace member** — it is not
> built or tested by `cargo test` at the repo root. Wiring it in is a prerequisite for
> an editor that runs it in-process.
