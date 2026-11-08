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
