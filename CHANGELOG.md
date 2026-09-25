# Changelog

All notable changes to CigScript. The format follows Keep a Changelog; the
project follows semantic versioning.

## 1.0.0 — 2026-09-26

First public release.

### Language
- One syntax: `roll`, `stick`, `pull`, `pack`, `snuff`, `exhale`, `cough`,
  `try`/`ashtray`, `burn`, `burn unlit`, `chain`, plus plain `if`/`else`,
  `while`, `for`/`in`, `break`, `continue`, `and`/`or`/`not`. Keywords may be
  used as member names and map keys.
- Values: null, bool, 64-bit int with checked arithmetic, float, UTF-8 string
  with `${}` interpolation and raw `'...'` form, list, insertion-ordered map,
  closures, chains.
- Operators: arithmetic, comparison, `??`, `?.`, `..` ranges, `>>` pipeline,
  compound assignment, indexing with negative offsets, method calls on
  strings, lists and maps.
- Errors carry a stable code, kind, message, line and any keys from a coughed
  map; everything is catchable except `exit`.

### Kernel
- Every library function declares an effect. Write, process and environment
  effects are refused outside a `burn` block, statically where visible and at
  run time always.
- `cig run` journals each burn with before-state snapshots and rolls back on
  failure; `cig unburn <id>` rolls back a finished run; `--no-rollback` opts
  out. `cig run --dry-run` simulates every burn and prints the plan without
  writing anything, not even a run record. `burn unlit` records intents.
- Run records under `$CIGSCRIPT_HOME/runs/<id>/` (default `~/.cigscript`).

### Chains
- `chain name { step, step }` declares an ordered list of sticks;
  `light(chain, opts?)` runs them with timing, retries, value passing and a
  report, stopping at the first failure. `cig chains <file>` lists them,
  `cig light <file> <chain>` runs one under the same dry-run and rollback
  rules as everything else.

### Standard library
- `fs`, `path`, `json`, `text` (regex and templates), `proc` (no shell, timeouts,
  no pipe deadlocks), `time`, `hash`, `math`, `env`, `csv`, `log`; globals
  `len`, `str`, `int`, `float`, `bool`, `type_of`, `range`, `keys`, `values`,
  `entries`, `assert`, `exit`, `stdin`, `light`; 64 methods on strings, lists
  and maps.

### Tooling
- `cig check` with name resolution, stick reassignment, `snuff`/`break`
  placement, module member typos and effect placement, with `did you mean` hints.
- Stable error codes E1xx to E9xx, `cig explain <code>`, generated `docs/ERRORS.md`.
- Crash reports: written locally on a panic, redacted, shown in full by
  `cig crash show`, filed only when you say yes; `crash_reports = ask|always|never`.
- `cig update` (SHA-256 verified, atomic swap, no downgrades, no untrusted
  install directories), `cig doctor --fix`, `install.sh` with a dependency
  preflight and no sudo, `cig config`, `--json` on every command, stable exit codes.

### Licensing
- Apache-2.0 code (canonical text), CC BY 4.0 documentation, CC0 1.0 examples,
  NOTICE, TRADEMARKS.md, a contributor licence agreement, THIRD-PARTY-NOTICES.md,
  REUSE.toml with SPDX headers, SECURITY.md.
