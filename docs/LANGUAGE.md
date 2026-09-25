# The CigScript language

This is the contract for CigScript 2.x. `cig language` prints the short
version.

## Files and statements

A script is a `.cig` file, UTF-8, one statement per line. A `#` starts a
comment, so `#!/usr/bin/env cig` works as a shebang. A `;` separates
statements on one line. Blocks are `{ ... }`. Newlines are ignored inside
parentheses, brackets and map braces, and after a binary operator or a comma,
so long expressions wrap naturally.

Statements execute top to bottom. `pull` declarations are hoisted within their
block, so a function can be called above the line that defines it.

## Values

| type | literals | notes |
|---|---|---|
| `null` | `null` | absence; `??` and `?.` are built for it |
| `bool` | `true`, `false` | |
| `int` | `42`, `1_000`, `-7` | 64-bit; overflow is an error |
| `float` | `2.5`, `1e9` | 64-bit; prints as `2.0`, never `2` |
| `string` | `"text ${expr}"`, `'raw'` | UTF-8; indexing and `len` count characters |
| `list` | `[1, "two", [3]]` | ordered, mutable, shared by reference |
| `map` | `{name: 1, "with space": 2, [expr]: 3}` | string keys, insertion order kept, shared by reference |
| `function` | `pull f() { }`, `pack(x) => x` | closures capture their scope |
| `chain` | `chain name { a, b }` | an ordered list of steps; see `docs/CHAINS.md` |

Double-quoted strings understand `\n \t \r \0 \\ \" \$ \u{1F6AC}` and
`${expr}` interpolation. Single-quoted strings are raw: no escapes, no
interpolation, handy for regexes and Windows paths.

**Truthiness.** `null`, `false`, `0`, `0.0`, `""`, `[]` and `{}` are false;
everything else is true.

**Equality.** `==` is structural: lists and maps compare by content, `1 == 1.0`
is true, `"1" == 1` is false. Functions compare by identity.

## Declarations and assignment

```cig
roll x = 1          # variable
stick y = 2         # constant; `y = 3` is a check error
x = 5
x += 1              # also -= *= /=
list[0] = "a"
map.key = "b"
map["key"] = "c"
```

A name can be declared once per scope; shadowing in an inner scope is fine.
Blocks, loops and functions each open a scope. Assigning to a name that was
never declared is an error, not a silent global.

## Functions

```cig
pull add(a, b) {
  snuff a + b          # snuff returns; a bare `snuff` returns null
}
stick inc = pack(x) => x + 1              # expression body
stick log = pack(x) { exhale x; snuff x } # block body
```

Calls take exactly the declared number of arguments. Functions close over the
variables around them, including mutable ones, so counters and accumulators
work. Recursion is fine up to a depth of 4,000 calls, after which the run fails
with a clear error instead of crashing.

Calling a function stored in a map works as you expect: `obj.greet("x")` calls
`obj.greet` with `"x"`. There is no implicit `self`.

## Control flow

```cig
if a > 1 { } else if a < 0 { } else { }
while cond { }
for item in xs { }        # lists; maps give their keys; strings their characters
for i in 0..10 { }        # ranges are half-open
break
continue
```

`for` binds a fresh variable per iteration, so closures created inside the
loop see the value from their own iteration.

## Errors

```cig
cough "message"                       # raise with a string
cough {message: "no space", code: 28} # or a map; extra keys ride along
try {
  risky()
} ashtray err {
  exhale err.message, err.kind, err.line, err.code
}
```

`ashtray` catches everything raised inside its `try`, including runtime errors
from the library (`err.kind` is then `runtime`, `type` or `burn`). The variable
is optional. An uncaught error ends the run with exit code 1 and rolls back the
run's burns. `exit(code)` stops the script cleanly and is not catchable.

## Operators, highest precedence first

| operators | meaning |
|---|---|
| `f(x)`, `a.b`, `a?.b`, `a.m(x)`, `a[i]` | call, member, null-safe member, method, index |
| `-x` | negation |
| `* / %` | `/` on two ints stays an int when exact, otherwise gives a float |
| `+ -` | `+` also joins strings and lists; `"s" * 3` repeats |
| `a..b` | range as a list of ints |
| `== != < <= > >=` | comparisons; not chainable |
| `??` | left value unless it is null |
| `not` | |
| `and` | short-circuit, returns the deciding value |
| `or` | short-circuit, returns the deciding value |
| `>>` | pipeline: `x >> f(a)` is `f(x, a)`; `x >> f` is `f(x)` |

Member access on a missing map key is an error; use `m?.key` to get null, or
`m.get("key", default)`. Negative indexes count from the end.

## Output

`exhale a, b` prints the values separated by a space, with a newline, to
stdout. Strings print raw; everything else prints as its literal form, so
`exhale [1, "a"]` shows `[1, "a"]`. Use `log.info` and friends for commentary
on stderr, keeping stdout for the program's real output.

## Burn blocks

```cig
burn { ... }         # side effects allowed here, journaled, rolled back on failure
burn unlit { ... }   # side effects recorded as intent and never executed
```

Inside a normal burn, functions marked *burn* in `docs/STDLIB.md` may run. A
function called from inside a burn inherits the permission, so helpers can be
written once and lit by their caller. Outside a burn those calls fail before the
script starts (when the call is visible to the checker) or at the moment of the
call (when it is inside a function). See `docs/BURN.md` for the modes, the
journal and what rollback can and cannot undo.

## Chains

```cig
chain release { fetch, build, package }
stick report = light(release, {retries: 2})
```

A chain is a constant holding an ordered list of steps, each a function
value or another chain. `light(chain, opts?)` runs them in order, passing
each step's result to the next step that takes an argument, and stops at the
first failure with an error that names the step. `cig light file.cig release`
does the same from the command line, and `cig chains file.cig` lists what a
file declares. The whole story is in `docs/CHAINS.md`.

## Keywords as names

Keywords are reserved as statement starters only. After a dot and as a map
key they are ordinary names, so `err.chain`, `m.if` and `{chain: 1}` are
fine. The one place this matters is the error map a failed chain raises,
which has a `chain` key.

## Error codes

Every error `cig` reports carries a stable code, rendered as
`error[E502 runtime]: division by zero`. The catalogue, with a fix for each,
is `docs/ERRORS.md`; `cig explain E502` prints an entry. Codes are stable
across versions; a code is never reused for a different meaning.

## Script arguments

Everything after `--` on the command line is `args`, a list of strings:

```
cig run tidy.cig -- ~/Downloads --verbose
```

`args.get(0, "default")` reads optional arguments safely.

## What is deliberately not here

No classes, no imports, no `match`, no async, no random numbers, no C-style
`&& || !`, no ternary, no `finally` yet, and only one syntax for each construct. Every one of
those was tried in the 1.x line and made the language larger without making
scripts shorter. The roadmap in `docs/DESIGN.md` lists what may come back.
