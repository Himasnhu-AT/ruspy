# Ruspy syntax

## 1. declare variables:

- type defined at execution

```
a = 10
```

a with value 10

- type declared

```
a: int = 10
```

a of type int is 10

### Allowed variables:

- int: integer
- int32
- int64
- float
- float32
- float64
- char
- str
- str8
- str32
- str64

## 2. declaring function:

```
def func_name(args) {
    // function code (execution)
}
```

## 3. dictionaries

```
d = {"hp": 10, "tags": ["a", "b"]}   # keys: str, int, bool or char; insertion order is kept
d["hp"] = 7;  d["hp"] += 1;           # index read / write (reading a missing key is an error)
d.hp -= 1;    d.mp = 4;               # `d.name` is sugar for d["name"] (string keys only)
len(d);  d.len();
d.keys();  d.values();                # arrays
d.has("hp");  d.erase("hp");          # erase returns true if the key existed
d.get("x", 0);                        # default (null when omitted)
d.clear();
for k in d { print k + ":" + d[k]; }  # iterates keys in insertion order
{"a": 1} == {"a": 1};                 # structural, order-insensitive
```

Dicts have reference semantics (like arrays) and print as `{"a": 1, "b": [2, 3]}`. The VM does not
compile dictionaries; `Engine::load` reports `E0900` so the host falls back to the interpreter.

## 4. host object handles

A host can return `Value::Object(id)`: an opaque handle (an entity, a resource). It prints as
`Node#<id>`, compares by id, and `is_null(h)` is false. What you can do with it is up to the host:

```
h.field            # object_get hook
h.field = v        # object_set hook (compound ops `+=` read via object_get first)
h.method(a, b)     # object_call hook
```

Embedding (`embed::Engine`): `register_object_get/set/call(fn(&mut Ctx, id, name, ..))`. The
bare name `self` resolves to the handle set with `Engine::set_self_handle(Some(id))`; for that
handle `self.x` / `self.f()` read the script's own variables and functions first (no host call),
and fall back to the hooks otherwise. Lower-level hosts implement the `RuspyHost` methods
`object_get/object_set/object_call/is_self/resolve_name`.

## 5. vectors and colors

`vec2(x, y)`, `vec3(x, y, z)`, `Color(r, g, b [, a])` (also `Vec2/Vec3/color`, and from one array:
`vec3([1, 2, 3])`). Value types: assigning copies. `+ - * /` work componentwise between two
vectors of the same type, and `*` `/` take a number (`2 * v`, `v / 2`); `-v`; `==`.
Components: `.x .y .z` (Color: `.r .g .b .a`), `v[0]`; write with `v.x = 1`, `v.x += 1`,
`v[1] = 2` when `v` is a variable (not through an expression like `f().x = 1`).
Methods: `length() length_squared() normalized() dot(v) cross(v) distance_to(v) lerp(v, t) abs()
to_array()`. Prints `(1, 2, 3)`.

## 6. strings

`len split([sep]) join(array) strip/trim upper lower replace(a, b) find(s) (-1 if missing)
contains/has begins_with ends_with substr(from[, len]) slice(from, to) to_int to_float`,
`s[i]` (one-char str), `fmt("{} + {}", a, b)` (`{{`/`}}` escape) and `"%s %d %.2f %%" % [a, b, c]`.
`",".join(arr)` and `arr.join(",")` both work.

## 7. more arrays

`append push pop insert(i, v) remove(i)/remove_at sort reverse has find/index_of slice(a[, b])
join(sep) contains first last clear sum min max len`.

## 8. const, enum, match

```
const SPEED = 5;                 # assigning to it later is an error
enum State { IDLE, RUN, JUMP = 10, FALL }   # State.IDLE == 0 ... State.FALL == 11 (a read-only dict)
match s {
    State.IDLE => { ... }
    State.RUN | State.JUMP => { ... }   # alternatives with |
    "a" => { ... },                     # commas between arms are optional
    _ => { ... }                        # wildcard
}
```
`match`, `const` and `enum` are contextual: still fine as variable names. `match` is a statement
(no value). Arms compare with `==`. The VM does not compile `match`, dicts (so enums), strings
methods or vectors' methods/assignments; the host falls back to the interpreter on `E0900`.

## 9. coroutines (`await`)

A function whose body contains `await` is a **coroutine**. Calling it runs until the first
`await` and returns `void` (a coroutine that never awaits returns its value normally); the host
resumes it later (`Engine::take_suspended`, `Engine::resume`, `Engine::drop_coroutine`; tokens
identify coroutines, the awaited value tells the host what to wait for, the resume value is
what `await` evaluates to).

```
fn countdown(n) {
    while n > 0 {
        await wait(1.0);          # host-defined awaitable (value is passed to the host)
        n -= 1;
    }
    hit = await signal_of(target, "died");   # x = await e;  x: T = await e;  return await e;
}
```
`await` must be a whole statement (`await e;`, `x = await e;`, `x: T = await e;`,
`return await e;`) but can sit inside `if`, `while`, `for`, `match` and blocks, with
`break`/`continue` working across it. Not supported: `await` inside a larger expression
(`1 + await e`, argument lists), at top level, in `x.f = await e`; `await f()` where `f` is
itself a coroutine does not wait for `f` (it starts detached and the outer resumes at once).
Coroutines need the interpreter backend (the VM reports `E0900`). Locals survive suspension; a
resumed coroutine gets a fresh step budget. Errors after a resume carry the located diagnostic and
free the coroutine.

## 10. call stacks

A runtime error carries the script call stack: `Diag::trace` / `ScriptError::trace` (innermost
frame first; function name and line:col — the failing expression for the innermost frame, the call
site for the others) and `ScriptError::trace_text()` (`  at fn (file:line:col)` lines). Both
backends and coroutines (after a resume) record it. Builtins: `get_stack()` returns an array of
`{"function", "file", "line", "col"}` dicts (innermost first, the caller of `get_stack` on top);
`print_stack()` sends the same as text to the host's `print` function. The embed API names the
file for these with the module name passed to `Engine::load`.
