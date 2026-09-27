# Changelog

All notable changes to CigScript. The format follows Keep a Changelog; the
project follows semantic versioning.

## Unreleased

Delivered from the "Hammer" update package (`HAMMER.md`), one pull request per
seam; the package's manifest carries the state of each deliverable.

### Language
- **`finally`.** `try { } ashtray e { } finally { }`, or `try { } finally { }`
  with no `ashtray`: the block runs whatever happened (a normal exit, a caught
  or uncaught error, `snuff`, `break`, `continue`, `exit()`); its own signal
  wins, otherwise the pending one carries on with its payload. `finally` is
  still usable as a map key or member name, like every keyword.
- **The step budget.** Every statement and loop iteration is a step; ten
  million by default (`--max-steps N` on `run` and `light`, `CIG_MAX_STEPS=N`,
  `0` disables). A loop that never ends now ends: `E515` *your loop never
  ends*, pointing at the `while`, and doctor reads the condition (`true`, so
  only a break can end it). Burns before the budget ran out are rolled back.

### Kernel
- **The ghost filesystem.** In a dry-run the pretend writes live in an
  in-memory overlay and every read consults it before the disk (`fs.read_*`,
  `fs.exists`, `fs.is_*`, `fs.size`, `fs.modified_ms`, `fs.list`, `fs.glob`,
  `json.load`, `csv.read`, `hash.sha256_file`). A chain whose step 2 reads
  step 1's write now passes `--dry-run` with the disk untouched. A simulated
  delete or move leaves a tombstone: a later read is `E520`, naming the op
  that removed it, and doctor points at the plan. A simulated `fs.cp`,
  `fs.mv` or `fs.rm` of a path that exists nowhere fails in the plan as it
  would for real. `burn unlit` never enters the ghost.

### The pack and compensations
- **`pack { "./build" }`** declares the scope every native write must stay
  in: literal paths outside it are refused by the checker (`E750`), computed
  ones by the kernel (`E751`), the plan prints the pack, and one pack per
  script, first (`E753`). Hops inside a pack are watched: the files a child
  created are journaled as reversible and removed on rollback, files it
  modified or deleted are listed as irreversible with the detail, and
  writes outside the pack are reported (`E752`), never hidden.
- **`burn (s) { } unburn { }`**: the compensation for what the kernel cannot
  see. Journaled with its state map when the block completes; run on
  rollback here or by `cig unburn` later, from the journal alone (so it may
  use only its state, modules and builtins: `E754`). Hops inside show as
  `compensated`, not `irreversible`.
- The plan tags every op with the chain and step that own it
  (`[release/build]`); a chain that lights itself is `E604` with the loop.

### Hops
- **The boundary holds.** `proc.run` keeps one call shape and gains the
  rules: exit codes are a contract (`ok: [0, 1]`, `check` is `[0]`) and a
  code outside it is `E553` naming the command, the code, the contract and
  stderr's last line; every child runs in its own process group and a
  timeout kills the group, SIGTERM then SIGKILL after `grace_ms` (`E554`,
  with how much it had written); a child killed by a signal is `E555`, not
  a `-1` result; output is decoded explicitly (`encoding`: strict UTF-8,
  `lossy`, `latin1`, `utf-16`) and bad bytes are `E557`; a program that is
  not there is `E551`, one that may not be executed `E552`. Children get
  `NO_COLOR=1` and `CIG_RUN_ID`; `clean_env` starts from a documented
  minimum.
- **Parsing verbs.** `proc.text`, `proc.lines`, `proc.json`, `proc.csv` and
  `proc.kv` read stdout only under the contract `[0]` and fail at the hop
  with `E556`, quoting the first 200 bytes of a bad shape.
- **`proc.pipe`** connects stages without a shell, one group, one timeout,
  every stage checked and the failing stage named. **`proc.shell`** is the
  shell, by name; `cig check` warns (`E308`) when one is smuggled through
  `proc.run("sh", ["-c", built])`.
- **Retries are idempotence-aware.** A chain retry rolls back what the
  failed attempt burned first (the journal marks those ops undone and
  `unburn` skips them), and a step whose attempt ran a hop is not retried
  unless `retry_irreversible: true`.
- `E603` is retired in favour of `E553`; the ashtray value is unchanged.

### Fixed
- A deleted or overwritten symlink now rolls back as a symlink with its
  original target, dangling or not, including links inside a restored
  directory. Writing through a link records the file the bytes land in, so
  that file is restored too. In 1.0.0 the journal could not snapshot a link at
  all and refused the burn with `E702`.
- A run whose process died mid-burn was invisible: status `running`, zero
  burns, nothing from `doctor`. It is now shown as `interrupted`, counted from
  the journal, reported by `cig doctor` (`E706`) and offered for `cig unburn`
  under `--fix`.
- `-9223372036854775808` (`i64::MIN`) parses; the magnitude without the sign
  is `E104` with a hint.
- A chain failure points at the step as written, not at the `light()` call,
  and never at `file:0:0` when lit from the CLI; the message also names the
  line the underlying error was raised on.
- A rollback that failed half-way through restoring a file could remove the
  live file first. Files and directories are now staged beside the path and
  swapped in.
- `fs.exists` sees a dangling symlink, so `if fs.exists(p) { fs.rm(p) }` works.

### Added
- A property test for rollback: random op sequences (writes, appends,
  deletes, moves, copies, mkdirs) over random trees with regular files, modes
  and live and dangling symlinks; the tree after rollback must be identical to
  the tree before, content, names, modes and link-ness included.
- Journal durability: snapshot, snapshot directory and journal entry are
  synced to disk before the effect. Measured at about 1 ms per journaled op
  (see `docs/BURN.md`).
- An allocation ceiling (256 MiB, `CIG_MAX_ALLOC` to change it): `repeat`,
  `*` on strings, padding, and `fs.read_text`/`fs.read_lines`/`json.load`/
  `csv.read` of an oversized file are refused with `E514` instead of aborting
  the process.
- **Unburn knows what changed since.** A finished run records the state of
  every path it touched (`after.json`); `cig unburn` compares it with the
  disk and refuses with `E704` when a file changed since, naming the later
  run that touched it when there is one, so runs are put back newest first.
  `--force` overrides; `--dry-run` shows a *changed since* column beside each
  entry; a run from before after-state records says "unknown" and is allowed.
- `cig unburn` checks every snapshot and moved file before touching anything
  (`E703`), refuses a second rollback of the same run (`E705`), and takes
  `--force` for both; `--dry-run` lists the problems it found.
- Reserved error-code sub-ranges: E51x runtime, E52x ghost filesystem, E55x
  hops, E70x kernel and journal, E75x folds and packs, E80x usage, E85x
  updater and doctor. New codes: `E514`, `E515`, `E520`, `E703`–`E706`.
- `run.json` records the pid of the run.
- The error-code registry is data: `errors/registry.toml` is the one place a
  code is defined, with its family and kind, which of the four kinds of no it
  is (the lexicon's vocabulary), where it arises, its ranked causes each with
  a named read-only probe and a remedy, and its related codes. `cig explain`
  renders it, `--json explain` prints the nodes, `cig explain --schema` prints
  the JSON Schema for a diagnostic, and `docs/ERRORS.md` and
  `docs/diagnostic.schema.json` are generated from it. `tests/registry.rs`
  fails when the source emits a code the registry lacks, when the registry
  holds a code nothing emits (unless tagged `planned`), when a page is
  incomplete, or when the generated docs are stale; the explain pages are
  golden-tested so wording cannot regress silently.
- Doctor fires on the emit hook. Under every reported diagnostic, doctor
  walks the code's ranked causes, runs the read-only probe for each (stat,
  mode bits, PATH, the journal, the run records; never a write, never the
  network) and says the one sentence it is allowed: *I think X, because I
  checked Y and found Z*, then the fix, then what to check if that is not it.
  Nothing confirmed: one line. Nothing to check: silence. 80 ms budget.
  `--no-doctor` or `CIG_DOCTOR=0` gives the bare diagnostic, byte for byte.
  In `--json` the diagnosis is a field. `cig doctor <run>` diagnoses a stored
  run's error again, read-only (the run record now keeps the diagnostic).
  `tests/corpus/` pins the words for every probe. `docs/DOCTOR.md`.
- `--plain` / `CIG_PLAIN=1`: the same codes, spans, hints and evidence
  without the banner or the hallway phrases; the manual's name stands in.
  Golden-tested as a strict subtraction of the themed output.
- Diagnostics carry a `subject` field in `--json`: the path, program or run
  id the message is about.
- `cig check --deny-warnings` for CI: exit 2 when there are warnings.
- **The wall, and the invitation to break it.** The README's "Break it"
  section and the wall: every cheat tried so far, what happened, and the
  test named after it; `tests/docs.rs` refuses a row whose test does not
  exist, and `tests/wall.rs` holds the rows nothing else covered.
- **`cig report <run>`: the bundle you attach.** One redacted JSON file
  (home paths `~`, secret-shaped values removed, file contents never
  included) with the run, the journal and its before-states, the intents,
  the diagnostic, doctor's diagnosis run again, and the install facts. The
  bug that does not crash is now as easy to send as a panic.
- Issue templates for a cheat that worked (four fields) and a coined term
  (the manual's name required), a pull-request template that asks for the
  test named after the cheat, and the `cheat` and `coined-a-term` labels.
- **The voice, everywhere.** The branding statement and the four kinds of
  no on the README, a hallway column in the lexicon table, the tone
  section in `docs/DESIGN.md`, hallway names with the manual's word attached
  in `BURN.md`, `LANGUAGE.md`, `CHAINS.md`, `STDLIB.md` (hops) and the CLI
  `--help` (`unburn: put the world back the way it was, newest op first
  (rollback)`). Hints reshaped to what to type next where they described
  the problem instead. `docs/LEXICON.md` is the source of truth.
- The usage layer carries its codes: `E801` cannot read script, `E802` state
  directory unavailable, `E803` no such run, and `E805` (no `gh` or `curl` at
  all) is told apart from `E806` (the transport failed).

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
