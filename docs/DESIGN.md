# Design notes: why version 2 looks like this

This is the record of the decision to rebuild CigScript from scratch in
September 2026, what the prototype line had become, what the best version of
the idea is, and what was cut to get there. Version 1.0.0 is the first
public release; the prototypes and the two private rebuild rounds that
preceded it were never published.

## What the prototypes were

Between 3 and 7 January 2026 the project went from v0.1 to v1.0.2: a Rust
workspace of about 23,000 lines, nine source-to-CigScript transpilers, a plugin
ABI with C, C++ and Unreal bindings, a "creative graph" subsystem, a tutorial
engine, a Tauri desktop app, a runtime registry naming a hundred languages, and
a separate "CigMini" interpreter. The concept documents alongside it describe
three "nicotine levels" of the same language, a cigarette-themed standard
library, and a Kernel specification for state directories, logging and CLI
conventions.

Reading the code rather than the documents told a different story. The parser
accepted three syntaxes for the same statement (`fn`, `roll pull`, `filter main`)
and none of them worked end to end: function parameters were stored under one
name and loaded under another, so every call with arguments failed; top-level
statements were dropped whenever a function existed; every `burn` block failed
validation because the validator's builtin list did not include the builtin the
parser emitted; there was no comment syntax, no assignment, no floats, no lists,
no maps and no closures. Rollback zipped the kernel's own log directory and
restored that, never a user file. Rollback also deleted its snapshot before
reading it. Division by zero panicked the VM. The carton installer had a
zip-slip path traversal. Three hundred lines of kernel code for previews,
capabilities and run history had no callers.

None of that is a criticism of the idea. It is what happens when a language is
grown by adding features faster than any of them are finished.

## The idea worth keeping

Strip the sprawl and one idea remains that no mainstream scripting language
offers: **side effects are syntactically fenced, previewable and reversible**.
Python scripts rot because effects are everywhere and nothing records them. A
language where `burn { }` is the only place the world changes can give you a
dry run that is trustworthy, a journal that is complete, and a rollback that
actually restores files, all without the script author doing anything.

The 1.x concept documents already said this in their best moments: "Burn is the
boundary", "nothing real happens unless you burn", "dry-run that shows what
burns would do", "snapshot hooks so dangerous scripts can be rolled back". The
rebuild takes those sentences literally and builds nothing else until they are
true.

## Decisions

**One syntax.** Every construct has exactly one spelling. The themed words are
kept only where CigScript adds a concept the reader needs a word for: `roll`
and `stick` (mutable and immutable bindings), `pull` and `pack` (named and
anonymous functions), `snuff` (return), `exhale` (print), `cough` (raise),
`ashtray` (catch), `burn` and `unlit`. Structural words (`if`, `while`, `for`,
`and`, `or`, `not`, `true`, `null`) stay plain because renaming `if` makes a
language larger without making it better. Booleans are `true`/`false`, not
`lit`/`unlit`, because `unlit` already means a burn class.

**A real value model.** Ints, floats, strings, ordered maps, lists, closures.
Structural equality. Checked arithmetic. Errors, never panics, for anything a
script can do.

**Tree-walking interpreter, no bytecode.** The 1.x pipeline (AST → typing → IR
→ verified CFG → bytecode → VM) was most of its code and the source of most of
its bugs. For scripts of a few hundred lines a direct evaluator is fast enough,
a tenth of the size, and every error can point at a source span.

**Effects as data.** Each library function carries an `Effect`. The checker and
the interpreter both consult it, so the burn rule has one definition. Effectful
functions describe what they are about to do as an `Op`; the kernel decides
whether to execute or simulate and journals the before-state first. Adding a
new effectful function is one table entry plus one `Op`.

**Rollback that restores user files**, with the honest limits written down:
processes are irreversible, and a dry run cannot read what it did not write.

**Determinism where it is cheap.** Maps keep insertion order, directory
listings and globs are sorted, there is no random number generator, and the run
record carries the script's hash and arguments. Time and the environment are
inputs, not effects.

**A checker, not a type system.** `cig check` resolves names lexically and
knows which calls burn. It catches the mistakes people actually make (typos,
assigning to a constant, a `snuff` outside a function, an effect outside a
burn) in milliseconds, without annotations.

**Tests that check the world, not the exit code.** The 1.x shell scripts
printed pass rates for tests that never ran. The 2.x suite runs the real binary
against golden outputs, and its kernel tests assert on the file system after a
rollback.

## What was cut, and where it went

| 1.x feature | decision |
|---|---|
| nine transpilers | dropped; they produced code the parser could not read |
| plugin ABI, C/C++/Unreal bindings | dropped for 2.0; a stable Rust API is the precondition, and that comes first |
| Tauri desktop app | dropped; the CLI's `--json` output is the integration surface |
| CigMini | absorbed: the whole language is now what CigMini was meant to be |
| packs, cartons, modules, `use pack` | dropped; single-file scripts for 2.0, imports on the roadmap |
| creative graph (cgx), tutorials, runtime registry, automation, repair engine | dropped |
| three lexicon levels | dropped; one syntax |
| `ashtray` as a defer block | changed to `try`/`ashtray` catch; scoped cleanup can return as `finally` |
| kernel session directory with JSONL events | kept as the run record |
| burn classes: normal, unlit, outside | kept normal and unlit; `outside with <tool>` was never more than a label |
| dry-run and check as ephemeral sessions | kept: neither leaves state behind |
| sticks as saved invocations | covered by closures: `stick later = pack() => f(1)` |
| the error identity "Don't see any cigarettes" | kept as the first line of every failure |

## The product layer

The second private round added what a language needs to be *used* rather than admired:
chains as the automation layer (the "chain smoking" the 1.x concept notes
joked about, taken literally: sticks lit one after another), stable error
codes with `cig explain`, a crash reporter that never sends without consent,
a self-updater that verifies before it installs, an installer with a
dependency preflight, and a licensing set decided from the Codex law store
rather than from habit. The kernel did not change; every one of those
features sits beside it.

## Authorship

CigScript was written with AI assistance (Claude, driving the HIVEMIND
runtime) under Jay Taylor's direction: the language design, the scope
decisions, what was cut and what was kept are his; the code was generated,
reviewed and tested under that direction. That is recorded here because the
licence is honest only if the authorship is, and because contributors are
asked to make the same disclosure (see `CONTRIBUTING.md`).

## Roadmap

In rough order of value:

1. Signed releases (minisign or Sigstore) so `cig update` can verify who built
   a binary, not only that it arrived intact.
2. `finally` on `try`/`ashtray`, for cleanup that must run either way.
3. A step budget (`--max-steps`) so a runaway loop fails instead of hanging.
4. `cig fmt`, a formatter, once the grammar has been stable for a while.
5. Imports of other `.cig` files, restricted to the script's directory tree.
6. `match` on values and simple patterns.
7. Chains with declared dependencies between steps, and parallel steps where
   the burns are independent.
8. A stable Rust embedding API, then a C ABI over it.
