# CigScript Update Package: "Hammer"

**Status: being delivered on `main`, one PR per seam; the manifest below carries the state.** This is the next update, packaged. Built from an
external adversarial read of v1.0.0 (first push, 8446397); everything marked
**verified** was reproduced against a release build of that commit on Linux
x86_64. Everything else is a place worth swinging at, ordered within each
section by how likely the first real user is to hit it.

The package has four parts:

- **Part A: Delivery.** What ships, in what order, and how each piece proves
  it's done.
- **Part B: The challenge.** The message that goes on the repo: break it, send
  the report, it gets fixed, it slaps.
- **Part C: The branding.** The lexicon becomes the product's voice, and every
  artifact on the repo gets rewritten into it.
- **Part D: The work.** Sections 0–11, the full hammer list, unchanged in
  substance and now framed as deliverables.

Goals this package serves, in the order stated:

1. A kernel that is too robust to break without cheating.
2. A serious tool for people learning to code.
3. A glue layer that talks to anything and chains it, with the burn model
   holding across process boundaries (the "pack").

---

# Part A: Delivery

## What this package is

One update, delivered as separable commits so a reviewer can check each seam
on its own and the property test can say which one went red. Nothing lands as
a single diff. Every deliverable below has an acceptance test; a deliverable
without a green test is not delivered.

## Package manifest

| # | deliverable | section | proves it's done when | state |
|---|---|---|---|---|
| 1 | Property test for rollback | 10 | random op sequences over random trees (symlinks included) burn, roll back, and the tree is byte-identical: content, names, modes, link-ness | delivered, `hammer/1` |
| 2 | Symlink restore | 1 | a deleted symlink comes back as a symlink, not a copy of its target | delivered, `hammer/1` |
| 3 | Interrupted-run detection | 1 | a run killed mid-burn is flagged by `doctor`, counted correctly by `runs`, and offered for `unburn` | delivered, `hammer/1` |
| 4 | `i64::MIN` literal | 2 | `-9223372036854775808` parses | delivered, `hammer/1` |
| 5 | Chain error spans | 1 | no diagnostic ever points at `file:0:0` | delivered, `hammer/1` |
| 6 | Journal durability | 1 | journal entry and snapshot are synced before the effect; benchmark attached | delivered, `hammer/1` (≈1 ms/op, `docs/BURN.md`) |
| 7 | Allocation ceiling | 1 | `"x".repeat(huge)` is a diagnostic, never an abort | delivered, `hammer/1` |
| 8 | `unburn` idempotence and pre-check | 1 | second `unburn` is a no-op; missing snapshots refuse before touching anything | delivered, `hammer/1` |
| 9 | Error-code registry as data | 8 | `ERRORS.md`, `explain`, `--json` schema and doctor's knowledge all generate from one file; CI fails on a code without an explain page | delivered, `hammer/2` (`errors/registry.toml`) |
| 10 | Doctor auto-fire | 9 | every diagnostic passes one emit hook; doctor prints verdict / why / fix / if-not; probes proven read-only | delivered, `hammer/3` (`docs/DOCTOR.md`; 14 probes) |
| 11 | Step budget and `finally` | 2, 3 | `while true {}` ends with "your loop never ends" and a line number | delivered, `hammer/5` |
| 12 | Cross-run hash check on `unburn` | 1 | rolling back over newer content refuses unless `--force` | delivered, `hammer/6` |
| 13 | Ghost filesystem for dry-run | 1, 6 | a chain whose step 2 reads what step 1 wrote passes `--dry-run` with the disk untouched | delivered, `hammer/7` (`src/burn/ghost.rs`) |
| 14 | Hop resilience + `proc.json/lines/csv` | 4 | every rule in the resilience list has a test; the deadlock and zombie cases pass | delivered, `hammer/8` (`docs/HOPS.md`, `tests/hops.rs`) |
| 15 | Pack + compensations + fold checks | 5, 6 | the four golden plans (inside / outside / cough / unburn) match; folds refuse with codes | delivered, `hammer/9`: the scope, watched hops, compensations, E604/E750-E754, owner tags; parameterised packs, cartons, the version fence, overlayfs and unattended flags stay open |
| 16 | Foreign scripts, Tier 1 then Python shim | 7 | an unchanged Python script gets plan and rollback; behaviour identical without `cig` | pending |
| 17 | Lexicon rewrite of all repo artifacts | Part C | every artifact reads in the vernacular with the manual's name attached; plain mode strips it cleanly | delivered, `hammer/4` (Parts B and C; `cig report`, templates, the wall) |
| 18 | Playground | 3 | `fs.rm` and `unburn` clickable in a browser on a fake filesystem | pending |

## Delivery order

1. Property test for rollback (section 10). Everything in section 1 gets cheap.
2. The verified bugs: symlink restore, interrupted-run detection,
   `i64::MIN` literal, chain error spans. Small, real, first-user-facing.
3. fsync, allocation ceiling, unburn idempotence and pre-check.
4. Error-code registry as data, ranges reserved, explain-coverage test
   (section 8), so everything after this lands with its codes and hints
   from day one.
5. Doctor firing on the emit hook with the first ten causes and probes
   (section 9), grown from every bug report after that.
6. The challenge messaging and the lexicon rewrite (Parts B and C), because
   from here on people are being invited in and every artifact should
   already speak the product's voice when they arrive.
7. Step budget and `finally`, because they gate the learning tool.
8. Cross-run hash check on `unburn`.
9. Ghost filesystem for dry-run, because folded automation and foreign-script
   dry-runs are both dead without it.
10. Hop resilience rules and `proc.json`/`lines`/`csv` (section 4).
11. Pack + compensations, designed together, then the fold checks (sections 5
    and 6).
12. Foreign scripts, Tier 1 first, then the Python shim as the first Tier 2
    (section 7).
13. Playground.

Everything else follows from having a kernel that a property test says is
correct and a plan that never lies about what it cannot undo.

---

# Part B: The challenge

The posture: it was built to be too robust to break without cheating, so the
invitation is to cheat. Every cheat that works becomes a fix and a regression
test; every cheat that fails becomes a row on the wall. The messaging has to
match the product's own ethic: nobody's on trial, and nobody gets to be a
smug bastard, including the author.

## README block (drop-in)

> ## Break it
>
> CigScript is built to be too robust to break without cheating. So cheat.
>
> Symlinks pointing outside the tree. Kill it mid-burn. Fill the disk. Edit a
> file under it while it runs. Yank the power. Fold something that shouldn't
> fold. If the kernel ever lies about what it can undo, or loses a file, or
> says "aww yea" when it should have said no, I want to know that day.
>
> **When it crashes:** it already wrote a report. `cig doctor` shows what's
> saved and what's unsent; say yes and it goes straight to this repo
> (redacted; you can read it first). If you'd rather not send it, attach the
> file to an issue instead.
>
> **When it's wrong without crashing** (the dangerous kind: exit 0, "file
> restored", wrong file): `cig runs` for the run id, then open an issue with
> the script, the plan, and what you expected. `cig report <run>` will bundle
> that for you once it lands.
>
> **What you get back:** a fix, a regression test with your name on it, and a
> row on the wall below. It will slap.
>
> ### The wall
>
> | cheat | result | since |
> |---|---|---|
> | effect hidden in a function, a lambda in `.map`, `proc.run` outside a burn | refused, statically and at runtime | v1.0.0 |
> | `cough` mid-burn after a write and a delete | rolled back, newest first | v1.0.0 |
> | `fs.mv` over an existing file, then `unburn` | both files back | v1.0.0 |
> | `fs.rm` on a non-empty dir, then rollback | tree and modes restored | v1.0.0 |
> | killed by a signal mid-burn | journal intact, `unburn` restored everything | v1.0.0 |
> | unbounded recursion | clean `E506` at depth 4000 | v1.0.0 |
> | `MAX + 1`, `(MAX + MAX) / 2`, `1.0 / 0.0`, `0.0 / 0.0` | diagnostics, never silent | v1.0.0 |
> | delete a symlink, `unburn` | **came back as a regular file** | fixed in this update |
> | chain dry-run where step 2 reads step 1's write | **FAILED at step 2** | fixed in this update (ghost filesystem) |
> | `"x".repeat(i64::MAX)` | **aborted the process** | fixed in this update |
> | `-9223372036854775808` as a literal | **lexer rejected it** | fixed in this update |
>
> Add a row. Bold is what you're looking for.

## CONTRIBUTING block (drop-in)

> ## Reporting a cheat that worked
>
> A good report has four things: the script (or the smallest one that still
> does it), the command you ran, what the plan said, and what actually
> happened. `cig runs` gives you the run id; the run record under
> `~/.cigscript/runs/<id>/` has the journal and the snapshots, and it's fine
> to attach the lot after you've checked it for anything you'd rather keep.
>
> Crash reports are opt-in (`crash_reports=ask`) and redacted before they
> leave your machine. `cig doctor` shows you every one it's holding and
> sends only when you say so. You can read the file first; it's plain JSON.
>
> The reply you'll get is the same shape every time: what broke, why, the
> fix, and the test that now covers it. No trial.

## The report flow (what has to exist for the messaging to be true)

- [ ] `cig doctor` lists saved and unsent crash reports and sends on consent
      (exists: `crash reports 0 saved, 0 unsent`, `crash_reports=ask`,
      `issues_repo=otmof-ops/CigScript`, `curl`/`gh` as transports).
- [ ] `cig report <run>` (section 8): plan, journal, diagnostic and `doctor`
      output in one redacted file, so the non-crash bug, the confident wrong
      answer, is as easy to send as a crash.
- [ ] An issue template with the four fields above, and a `cheat` label so
      the wall can be generated from closed issues instead of maintained by
      hand.
- [ ] Every fixed cheat lands with a regression test named after the cheat
      (`symlink_restore_is_a_symlink`, `chain_dry_run_reads_ghost_write`), so
      the wall and the test suite are the same list.
- [ ] The wall never claims a cheat failed unless the test exists. The wall
      is the irreversible label applied to the project itself: honest or
      nothing.

---

# Part C: The branding

## Branding statement

> **CigScript. Nothing real happens unless you burn.**
>
> Computers spend their whole lives scrounging ciggies off each other. One
> walks over, covers its eyes, and goes "can't see any ciggies bro, where are
> they at." The other one leans on the wall and says "nar mate, never seen
> any ciggies over HERE before," and neither of them tells you where to
> point. Then something gets deleted, and it's your fault, apparently, and
> you're on trial for it.
>
> CigScript is the one that points.
>
> Every effect is a **burn**. Every burn is written down *before* it happens,
> so you can read the **plan** first and **unburn** it after. When it says no,
> it tells you which of the **four kinds of no** it is and where to look, and
> when it doesn't know, it says "I think this, because I checked that." Chains
> light from each other and put everything back when one goes out. Sticks go
> in packs, packs go in cartons, and nobody has to be taught the hierarchy.
>
> It's a language for people who are sick of piping dry-run in by hand.
> Nobody's on trial. Nobody gets to be a smug bastard. Nicotine: 0 mg.

## The voice, as rules

Taken from the lexicon's coining rules, applied to everything the product
says:

1. **Name it by what it feels like, then attach the real name.** Every
   vernacular term appears with the manual's word on first use in any
   artifact: *the fingering stage (memory training)*, *a hop (process
   boundary)*, *the four kinds of no (401, 403, 404, wrong address)*. The
   joke is the mnemonic; the real name is the receipt.
2. **It has to map cleanly.** A term that doesn't correspond to one real
   mechanism doesn't go in a doc, it goes in a tweet.
3. **It has to work in a hallway.** If you wouldn't say it out loud to a
   colleague to explain the problem, it's not a keyword.
4. **Plain mode is not a translation, it's a subtraction.** `--plain` /
   `CIG_PLAIN=1` keeps every code, span, hint and fact and removes the
   catchphrases. Same information, different first line. Schools, CI logs
   and procurement get the same tool with the jokes off.
5. **Never smug, never on trial.** No error, doc, help text or commit message
   blames the person or withholds the pointer. "Don't see any cigarettes" is
   the tool admitting its own eyes are covered; the hint is someone pointing.

## The lexicon becomes a repo artifact

- [ ] `docs/LEXICON.md`: the full lexicon (language, CLI, kernel,
      composition, errors, process words, hardware and firmware, the OS,
      scrounging a ciggy, the four kinds of no, the tone, fibre) with the
      three-column shape: **your word / what it means / the manual's name**.
      It is the source of truth for every term below; other artifacts link to
      it rather than redefining.
- [ ] The README's lexicon table gains a fourth column, **in the hallway**:
      how you'd say it out loud. `burn` → "the only place anything real
      happens." `unburn` → "put it back."
- [ ] The gap list stays in the lexicon as an open invitation: "what do you
      call…" with the manual's name in brackets, so contributors can coin
      terms the same way they report cheats.

## Rewrite plan: every artifact on the repo

Each artifact is rewritten into the vernacular under the five rules above.
None of them loses a fact; they gain a voice.

| artifact | what changes |
|---|---|
| `README.md` | Branding statement at the top. "Break it" block and the wall. Lexicon table with the hallway column. "The four kinds of no" as its own short section under the burn model table. Health note stays. |
| `docs/DESIGN.md` | New section, **The tone**: why every other tool's errors put you on trial, why the answerer is a smug bastard, and why CigScript's diagnostics are built to be the opposite of both. The post-mortem paragraph stays exactly as it is; it's the best paragraph in the repo. |
| `docs/BURN.md` | Kernel terms get their hallway versions: *snapshot (a copy taken before)*, *the journal (written before, so a kill mid-burn still has a record)*, *newest first*. "What this does not promise" is rewritten as "What it can't undo, and says so": the irreversible label explained as the honesty guarantee. |
| `docs/LANGUAGE.md` | The one-line meaning of each level written before the code: stick = one step, chain = order, pack = scope and reuse, carton = distribution. `burn unlit` described as rehearsal. Every keyword gets its manual name in brackets on first use. |
| `docs/ERRORS.md` | Generated from the registry (section 8). Every code tagged with which kind of no it is. Every entry has the hint shape: what to type next, not what went wrong. Plain-mode text shown alongside the themed text. |
| `docs/CHAINS.md` | "Each stick lit from the last." Cross-step rollback described as the feature no other task runner has. Folding, packs and cartons introduced in the level table's words. |
| `docs/STDLIB.md` | `proc.*` documented as hops, with the four kinds of no mapped to exit codes and the contract (`ok: [0, 1]`). `http.*` (when it lands) documented with reads free and writes as burns. |
| `docs/STDLIB.md` / adapters | Foreign-script doors table per language: door, instrumented, reversible. "Instrument the doors, leave the room alone" as the section's first line. |
| `CONTRIBUTING.md` | The "Reporting a cheat that worked" block. Coining rules for new terms. The rule that every fix ships with a test named after the cheat. |
| `CHANGELOG.md` | Entries in the vernacular with the real change attached: *"deleted symlinks now come back as symlinks (restore uses `symlink_metadata` and `symlink()`)."* |
| `HAMMER.md` | Becomes this package. The wall in the README is generated from its section 0 and its verified list. |
| CLI `--help` text | Every command's one-liner in hallway words with the manual's word in brackets: `unburn  put the world back (rollback)`. |
| Diagnostics (first line + hint) | The themed first line stays. Hints are rewritten to the "what to type next" shape everywhere E203 already has it. The four-kinds-of-no vocabulary used in every not-found / refused / forbidden / empty case. Plain mode strips the first line only. |
| Doctor output | The one permitted sentence, everywhere: *I think X because I checked Y and found Z.* Verdict / why / fix / if-not, in the vernacular, with the manual's name on every mechanism it names. |
| `cig explain` pages | Generated from the registry, in both modes, with the kind-of-no tag and the hallway explanation first, mechanism second. |
| Issue and PR templates | The four-field cheat report. A "coined a term" template that requires the manual's name. |
| Playground (when it lands) | Every interactive example titled in the vernacular, with the real name in the tooltip. The first example is `fs.rm` on a fake folder and the `unburn` button. |
| Release notes / `cig update` messages | Same voice, same rule: the change in hallway words, the mechanism in brackets. |

## Acceptance for Part C

- [ ] Every vernacular term in every artifact resolves to a `LEXICON.md`
      entry with a manual name. A doc term without a lexicon entry fails
      review.
- [ ] `--plain` output for every diagnostic and every `doctor` verdict is a
      strict subset of the themed output: nothing added, only the first line
      and catchphrases removed. Golden-tested in both modes.
- [ ] A reader who knows only the vernacular can find the right page in a
      vendor forum or a kernel bug tracker from the bracketed names alone.
      Test it on the beginner panel (section 3).
- [ ] Nothing in any artifact blames the reader or withholds the pointer.
      Review checklist item, not a vibe.

---

# Part D: The work

## 0. What already held under abuse (don't break these)

- Effects hidden in a function, a lambda passed to `.map`, and `proc.run` were
  all refused outside a burn, statically (E303) and at runtime (E701).
- `cough` mid-burn rolled back a write and a delete, newest first.
- Dry-run touched nothing.
- `fs.mv` onto an existing file, then `unburn`: both the moved file and the
  clobbered target came back.
- `fs.rm` on a non-empty directory, then rollback: tree and file modes (600)
  restored.
- Killed by a signal mid-burn: the journal is write-ahead, so a manual
  `cig unburn` restored everything.
- Unbounded recursion: clean `E506 call depth exceeded 4000`, not a stack
  overflow.
- `MAX + 1`, `(MAX + MAX) / 2`, `1.0 / 0.0`, `0.0 / 0.0`: all diagnostics,
  never silent.
- 58/58 tests, clippy silent, zero `unwrap()` outside `#[cfg(test)]`.

Keep a regression test for each of these. They are the product, and they are
the first rows on the wall.

---

## 1. Kernel and rollback (goal 1)

### Verified bugs

- [ ] **Symlink deleted, then unburned, comes back as a regular file** (a copy
      of the target). The snapshot follows the link; the restore doesn't know it
      was one. Snapshot with `symlink_metadata`, record the link target, and
      restore with `symlink()`. Same question for `fs.cp` and `fs.mv` of a link.
      ```sh
      ln -s ../outside/target.txt link.txt
      echo 'burn { fs.rm("link.txt") }' > rm.cig && cig run rm.cig
      cig unburn <id>; test -L link.txt || echo "came back as a file"
      ```
- [ ] **Interrupted runs are invisible.** After a mid-burn kill the run sits at
      status `running` forever, `cig runs` shows it as 0 burns although two were
      journaled, and `cig doctor` says nothing. The fix exists (`cig unburn`)
      and nothing points at it. `doctor` should list runs stuck in `running`
      whose process is gone and offer the unburn; `runs` should count from the
      journal, not from a total written at exit.
      ```sh
      printf 'burn {\n fs.write_text("f1.txt","x")\n fs.write_text("f2.txt", "x".repeat(300000))\n}\n' > big.cig
      ( ulimit -f 100; cig run big.cig )   # dies with SIGXFSZ mid-burn
      cig runs; cig doctor                   # nothing flags it
      ```
- [ ] **No fsync in the journal.** Zero `sync_all`/`sync_data` calls in
      `src/burn/journal.rs`. Write-ahead survives a process kill (the OS still
      has the buffers) but not a power cut or kernel panic between the journal
      write and the effect. `sync_data` the journal entry and the snapshot
      before returning `Decision::Execute`; measure the cost, it is usually fine
      for the op sizes scripts do.
- [ ] **Allocation failure aborts the process** instead of raising.
      `"x".repeat(9223372036854775807)` prints `memory allocation of ... bytes
      failed` and aborts, which kills a burn in progress with no rollback (the
      journal saves you, but see the previous two items). Put a size ceiling on
      `repeat`, list/string building and `fs.read_*`, and turn the abort into a
      diagnostic.
- [ ] **`unburn` twice does it twice.** It prints `already rolled back`, then
      restores again anyway. Harmless today; it will not be harmless once a
      later run has touched the same files. Make the second call a no-op unless
      `--force`.
- [ ] **`unburn` with missing snapshots fails per op, no pre-check.** For one
      op that is fine; for a 14-op run it restores some and fails on others,
      which is the one state rollback must never leave. Verify every snapshot
      exists (and hashes) before restoring anything, then restore, then mark.

### Verified in chains (`cig light`)

- [ ] **Dry-run of a chain fails when a step reads what an earlier step
      wrote.** `fetch` simulates `fs.write_text("src.txt", ...)`, so `build`'s
      `fs.read_text("src.txt")` fails and the chain reports FAILED at step 2
      even though the plan is correct. Nearly every real pipeline reads what
      it just produced, so today the dry-run promise only holds for chains
      with no data flow between steps. Fix: a ghost filesystem for dry-runs;
      simulated writes go into an in-memory overlay and every `fs.read_*`,
      `fs.exists`, `fs.list` consults the overlay before the disk. The same
      overlay is what a browser playground needs.
      ```sh
      printf 'stick fetch = pack() { burn { fs.write_text("src.txt", "hello") } }\nstick build = pack() { burn { fs.write_text("out.txt", fs.read_text("src.txt").upper()) } }\nchain release { fetch, build }\n' > tasks.cig
      cig light tasks.cig release --dry-run   # FAILED at step 2: no such file
      ```
- [ ] **Chain failures point at `file:0:0`** and print line 1 with a caret
      under column 0, because the E602 diagnostic has no span. Carry the
      failing step's definition span (and ideally the `cough` site inside it)
      into the diagnostic.
- [x] **Cross-step rollback works.** A failure at step 3 rolled back steps 1
      and 2 and left the directory clean. Keep this as a golden test; it is
      the feature no other task runner has.

### Design gaps the docs already admit

- [ ] **Cross-run consistency.** `BURN.md` says rolling back an old run after
      later runs touched the same files restores old snapshots over newer
      content. The journal already stores before-hashes; compare the current
      file's hash to the *after* state the run left and refuse with a clear
      error unless `--force`. This is the first surprise a real user will file.
- [ ] **`unburn --dry-run`** is mentioned as the mitigation; make sure it
      exists, prints the same plan format as `run --dry-run`, and shows which
      files have changed since.

### Cheats still to try

- [ ] Hardlinks: delete one name, restore, check the inode count.
- [ ] Non-UTF-8 filenames, filenames with newlines, very long paths.
- [ ] Case-insensitive filesystems (macOS default): `fs.mv("a", "A")`.
- [ ] A file that changes under you *during* a run: `fs.write` snapshot, a
      `proc.run` that edits the same file, then a `cough`. Rollback will clobber
      the child's edit; decide whether that is correct and document it.
- [ ] Read-only target directory, unwritable `~/.cigscript`, home on a full
      disk, home on a different filesystem from the target (snapshot copies
      across devices).
- [ ] Two runs of the same script started in the same second: confirm the id
      hash suffix never collides, and that two concurrent runs touching the
      same file both journal correctly.
- [ ] `cig runs --prune` while a run is active.
- [ ] Restore preserves mode (verified), but check owner, mtime, xattrs, and
      whether that is the intended contract.
- [ ] `burn unlit` nested inside `burn`, and `burn` nested inside a function
      called from `burn unlit`. Confirm `unlit_depth` wins in both.
- [ ] Rollback of `Mkdir` when the directory already had content created by a
      later op in the same run (ordering is newest-first; check the created-
      parent case in the comment at journal.rs:241 with a test).

---

## 2. Language robustness (goals 1 and 2)

- [ ] **Step budget.** `while true {}` hangs until killed. Already on the
      roadmap as `--max-steps`; for a learning tool it is the first thing a
      beginner will hit, and the error should say "your loop never ends" with
      the line.
- [ ] **`i64::MIN` has no spelling.** `-9223372036854775808` fails at the lexer
      (E104) because the positive literal is read first. One-line fix in the
      parser: fold `-` into an integer literal before range-checking.
- [ ] Every arithmetic operator on `i64::MIN` and `i64::MAX`: `-`, `*`, `/`,
      `%`, unary minus, `abs`, `pow` if it exists. Overflow on `+` is verified;
      check the rest have the same E503 path.
- [ ] Float edge cases beyond division: `NaN` comparisons, `-0.0`, `inf` in
      `json.stringify`, float-to-int conversion of out-of-range values.
- [ ] String interpolation: `${` unterminated, nested quotes, `$` alone,
      escaped `\${`, a `}` inside a string inside the interpolation.
- [ ] Unicode: `.len()` on multi-byte strings (bytes or chars? document it),
      indexing into the middle of a grapheme, `.upper()` on `ß`.
- [ ] Closures capturing a `roll` that is later reassigned: by value or by
      reference? Whatever the answer, make it a golden test.
- [ ] `snuff`, `break`, `continue` from inside a `burn` inside a loop: confirm
      `burn_depth` is decremented on every exit path (the code looks right;
      test it).
- [ ] `cough` inside an `ashtray` handler that is itself inside a burn.
- [ ] Deep nesting of lists/maps in `json.parse` (stack depth), and a 100 MB
      JSON file (memory).
- [ ] Map iteration order after deletes and re-inserts: "ordered maps" must
      mean insertion order survives, or the dry-run plan stops being stable.
- [ ] `proc.run` with a timeout that fires while the child has a huge stdout:
      no deadlock, no zombie.
- [ ] Type errors: `1 + "a"`, `[1] + 1`, calling a non-function. These are
      runtime in a dynamic language, so make each one a beginner-grade
      diagnostic with a hint, and make sure `cig check` catches the ones it can
      see (literal operands).

---

## 3. The learning tool (goal 2)

- [ ] **Beginner test panel.** Give five people who have never coded the
      sixty-second tour and `tidy-downloads.cig`, and watch. Every place they
      stall is a diagnostic or a doc to fix. Do this before adding features.
- [ ] `cig explain` covers every error code that can be emitted; test that by
      enumerating the codes in `diagnostics.rs` against `ERRORS.md`.
- [ ] `finally` on `try`/`ashtray` (roadmap). Beginners write cleanup code
      first and learn why it needs `finally` second.
- [ ] REPL: multi-line input, history, tab completion of module names, and
      `burn { }` in the REPL behaving exactly like in a script (does the REPL
      journal? it should say).
- [ ] A **browser playground** built from the Rust via WASM, with a fake
      filesystem so `fs.rm` and `unburn` are safe to click. This is the single
      biggest lever for both audiences: nothing sells "your script can't hurt
      you" like pressing the button that would have.
- [ ] A "first hour" document: install, break something on purpose, undo it,
      write a chain. The README is for developers; this is for the other
      audience.
- [ ] Plain mode (Part C, rule 4) is the answer to the school-and-workplace
      question: same codes, same hints, jokes off. The keywords stay.
- [ ] Editor support: a tree-sitter grammar or at least a TextMate grammar for
      VS Code. Beginners judge a language by whether their editor colours it.

---

## 4. The universal glue layer (goal 3)

The claim: one layer that can talk to anything and chain it, so devs stop
writing Python to call bash to call Node to call a CLI, with every hop losing
the shape of the data, the meaning of the exit code, and any idea of what the
other program did. Two properties decide whether CigScript earns that role,
and they pull against each other, so they are listed separately.

**Resilient:** every boundary is where guarantees die, so the boundary is
where the logic has to be hardest. Nothing crosses a hop unchecked, nothing
fails silently, and a failure names the hop.

**Easy to talk:** if calling a tool is harder in CigScript than in bash, people
use bash. One call shape, structured data by default, and the common cases
need no ceremony at all.

### Why the boundary is where the logic must be hardest

The burn model's promise is "reversible where the kernel performed the effect
itself." A child process is the one place the kernel performed nothing, saw
nothing, and can undo nothing on its own. Inside the language you have fenced
effects, checked arithmetic, a journal and a plan; the moment a hop happens,
all of it is gone unless the hop is designed to carry it. Python glue is bad
here precisely because it is permissive: a wrong shape from step A sails
through until step C dies with a stack trace that names nobody. CigScript's
edge is the opposite instinct: refuse at the boundary, name the hop, keep the
label honest.

### Resilience rules for every hop (make each one a test)

- [ ] **Exit codes are a contract, not a boolean.** `proc.run` takes
      `ok: [0, 1]` for tools like `grep`, `diff` and `cmp` whose non-zero
      codes are answers, not failures. Anything outside the contract is a
      diagnostic that names the command, the code and the hop.
- [ ] **stdout and stderr are never merged.** Structured data comes from
      stdout only; stderr is captured and shown on failure. A tool that
      prints progress to stdout breaks the shape, and the error should say
      "expected JSON on stdout, got `Downloading...`".
- [ ] **Shape is checked at the hop that produced it.** `proc.json` fails at
      the step that emitted bad JSON, with the first 200 bytes of what it got,
      not three steps later.
- [ ] **Timeouts always exist.** Every hop has a timeout, with a default
      (`timeout_ms`) and a diagnostic that says which hop stalled and what it
      had written so far. No hop can hang a chain forever.
- [ ] **Partial output is never mistaken for output.** A child killed by
      timeout or signal produced *nothing* as far as the next step is
      concerned; its truncated stdout is attached to the error, not passed on.
- [ ] **Large output does not deadlock.** Read stdout and stderr concurrently
      (a child writing 100 MB to stderr while you wait on stdout is the
      classic hang). Test it with `yes | head -c 200M`.
- [ ] **Encoding is explicit.** Output that is not valid UTF-8 is a
      diagnostic or an explicit `bytes` value, never silently lossy. Windows
      tools that emit UTF-16 and CP-1252 are the test cases.
- [ ] **No zombies, no orphans.** A timed-out child is killed with its whole
      process group; a chain that is itself killed mid-hop does not leave the
      child running. Test with a child that spawns a grandchild.
- [ ] **Environment is deliberate.** `proc.run` gets a clean, documented
      environment plus what the script adds, not whatever the shell had.
      `PATH`, cwd, locale and `NO_COLOR` are set the same way on every
      platform, so a chain behaves the same on the laptop and in CI.
- [ ] **No shell by default.** Arguments are a list, never a string handed to
      `sh -c`. `proc.shell` can exist for the people who need pipes, but it
      is a different verb with its own warning, because that is where every
      injection bug in every glue script has ever lived.
- [ ] **The irreversible label survives the hop.** A hop with no snapshot and
      no compensation is irreversible and the plan says so. The label is the
      whole model; laundering it through a wrapper is the one bug that would
      make the tool untrustworthy.
- [ ] **Failure taxonomy with stable codes.** Distinct codes for: command not
      found, refused to start (permissions), non-zero exit outside contract,
      timeout, killed by signal, bad shape on stdout, bad encoding, outside
      the pack. A CI log should tell you which one without the message. Each
      maps to one of the four kinds of no where it fits.
- [ ] **Retries are per-hop and idempotence-aware.** A retry re-runs a hop
      against the rolled-back state, and a hop marked irreversible is never
      retried automatically.
- [ ] **Determinism holds across hops.** Same inputs, same plan. Child
      output that varies (timestamps, temp names) is the child's problem, but
      the plan must not vary because CigScript itself did something
      different.

### Making it easy to talk (the part that decides adoption)

- [ ] **One call shape for everything.** `proc.run(cmd, args, opts)` returns
      `{code, out, err}` and every other verb is sugar on it: `proc.json`,
      `proc.lines`, `proc.csv`, `proc.text`. Learn one thing, use it for every
      tool ever written.
- [ ] **Structured by default.** The moment data is parsed, it is a real
      value: a map, a list, not text. `proc.json("gh", ["pr", "list",
      "--json", "number,title"])` gives you a list of maps you can `.filter`.
      Nushell and PowerShell won their niche on exactly this.
- [ ] **Shape assertions in one line.** Something like
      `expect rows is list of {number: int, title: string}` at the boundary,
      so the contract between two programs is written down where it lives and
      checked where it matters. The checker can verify literal shapes.
- [ ] **Piping without a shell.** `proc.pipe([["cat", "big.log"], ["grep",
      "ERROR"], ["sort"]])` so the common bash idiom exists without `sh -c`.
- [ ] **The four data formats work out of the box:** JSON, CSV, line-based
      text, and environment-style `KEY=value`. That covers nearly every CLI
      in existence. YAML and TOML as subtools, not stdlib.
- [ ] **Adapters as subtools, not stdlib.** `git`, `gh`, `docker`, `kubectl`,
      `cargo`, `npm`, `aws` each get a subtool that speaks the burn protocol,
      knows the tool's exit codes and output shapes, and reports effects so
      the kernel can journal them. The stdlib stays small; the adapters are
      cartons anyone can write and ship.
- [ ] **The burn protocol, one page.** An env var (`CIG_DRY_RUN=1`), an
      effects manifest on a documented fd or stdout, a version field. A tool
      that speaks it participates in the plan and rollback like a native op.
      `cig` itself is the first tool to speak it (a `.cig` calling `cig`), and
      the second is one of your own subtools, so the protocol is designed by
      using it.
- [ ] **Both directions.** The embedding API (roadmap) lets Python, Node or
      Rust call CigScript for the transactional part of their own workflow,
      so adoption does not require a rewrite in either direction. A C ABI and
      one language binding is enough to prove the shape.
- [ ] **Network hops are explicit effects.** `http.get` is a read and stays
      free; `http.post`/`put`/`delete` are effects, need a burn, and are
      irreversible unless the script supplies a compensation. The label is
      honest for the network too.
- [ ] **Compensations (the saga pattern).** `burn { ... } unburn { ... }` for
      what the kernel cannot see: `git push`, an API call, a database write.
      The journal records the compensation, `cig unburn` runs it in reverse
      order, the plan shows `compensated` rather than `irreversible`.
- [ ] **Errors read like a person explaining the hop.** "step 3 (deploy):
      `kubectl apply` exited 1 (contract: 0); stderr: `... forbidden`" tells a
      beginner and a CI log the same thing. This is where "easy to learn" and
      "universal glue" are the same feature. In the vernacular: which of the
      four kinds of no, and where to point.
- [ ] **Imports** (roadmap), restricted to the script's tree, so a project's
      adapters and shape assertions live in one file and every chain shares
      them. Without imports, glue scripts copy-paste, which is the mess this
      is meant to end.

### Things to hammer once the hops exist

- [ ] A child that writes JSON to stdout *and* progress to stderr: parses.
- [ ] The same child with progress on stdout: named diagnostic, first bytes
      shown, no partial value passed on.
- [ ] A child that exits 1 as an answer (`grep` no match) with and without
      `ok: [0, 1]`.
- [ ] `yes | head -c 200M` on stderr while stdout is awaited: no deadlock.
- [ ] A child that ignores SIGTERM: timeout still ends it (SIGKILL after a
      grace period), grandchildren included.
- [ ] Output in UTF-16 (a Windows tool) and invalid UTF-8: explicit, never
      lossy.
- [ ] A hop that is a `.cig` script calling `cig` with `CIG_DRY_RUN=1` set:
      the inner plan appears in the outer plan.
- [ ] Compensation runs on `unburn` after `git push` to a throwaway remote,
      and the plan showed `compensated` beforehand.
- [ ] `proc.run("sh", ["-c", userInput])`: the checker warns; `proc.shell`
      exists for the honest cases and carries its own warning.
- [ ] Kill the chain mid-hop, then `cig runs`: the run is flagged, the child
      is gone, `unburn` restores what the kernel did and lists the hop as
      "unknown outcome, irreversible".

---

## 5. The pack (goal 3, the design you described)

The script declares the environment it lives in; the kernel hashes it before,
compares it after, and refuses anything outside it. Decisions to make and
things to test once it exists:

- [ ] **Syntax.** `pack { "~/Downloads", "./build" }` at the top of the script
      reads well and keeps the theme. Decide whether globs are allowed and
      whether `~` expands.
- [ ] **Refusal outside the pack is an error**, not a warning, for both native
      ops (kernel, at runtime) and literal paths (checker, before the run). The
      plan prints the pack alongside the ops.
- [ ] **Child processes.** The parser cannot see what `cargo build` writes, so
      for `proc.run` the pack is the write set by definition: hash the pack
      before the child, hash after, journal the diff. Anything the child wrote
      *outside* the pack is the interesting case: detect it (a second, cheaper
      scan of the child's cwd is a good default) and refuse or label it.
- [ ] **Cost.** Hashing a pack that contains `node_modules` on every child call
      is the failure mode. Hash lazily (mtime+size first, content only on
      change), and cache across ops in the same run.
- [ ] **Overlay upgrade (Linux).** Run children inside an overlayfs with user
      namespaces so the write set is observed instead of declared. Keep the
      declared pack as the portable path and the overlay as the upgrade.
- [ ] **The network boundary.** No pack reverses `git push`. That is what
      compensations are for; the pack and the compensation are two halves of
      the same feature and the docs should say so in one sentence.
- [ ] Test: a script with a pack, a child that writes inside it, a child that
      writes outside it, a `cough` after each, and `unburn` after a success.
      Golden-test the plan output for all four.

---

## 6. Layered automation: chains → packs → cartons (goal 3, the plan)

The intent: build automation chains, fold tested chains into a pack you can
light from anywhere, fold packs into cartons, and keep folding until the
top-level chain reads like a sentence and every word under it is something
that already works. Automation gets what code has always had: functions into
modules into packages, instead of a folder of `deploy2_final.sh`.

The rule that makes it trustworthy: **a fold is a checkpoint.** CigScript
refuses to fold anything that does not pass `cig check`, with a stable error
code, so a mess can never be promoted upward.

### What each level means (write this line in LANGUAGE.md before the code)

| level  | it adds        | it owns                                   | it cannot do              |
|--------|----------------|-------------------------------------------|---------------------------|
| stick  | one step       | its own burns                             | order                     |
| chain  | **order**      | steps that light from each other, fail together, roll back together | scope, reuse from elsewhere |
| pack   | **scope + reuse** | a declared environment (paths), one journal for every chain inside, parameters | shipping |
| carton | **distribution** | a versioned set of packs with a manifest, `cig >=` requirement, its subtools | nothing local |

1.x had packs and cartons and they were cut because they were bigger folders.
They come back only if each row above stays a different noun.

### Fold-time checks (all refusals, all with error codes)

- [ ] **Parse.** Anything that fails `cig check` cannot be folded. The floor.
- [ ] **Shape.** Arity and parameter names match at every call site: a pack
      declared `pack(binary)` is lit with exactly one argument, and a chain
      that lights a pack passes what the pack declares. Error names the pack,
      the caller and the mismatch.
- [ ] **Scope.** Every literal path in the folded chains sits inside the
      pack's declared environment. Computed paths are the kernel's job at
      runtime, but literals are caught here, before anything is lit.
- [ ] **Irreversibility propagates up.** If any folded chain contains an
      irreversible op without a compensation, the pack carries the label, and
      so does every carton that contains the pack. A fold never launders
      "irreversible" into "reversible" by wrapping it.
- [ ] **No cycles.** A pack that lights itself through three cartons is a
      fold error, not a runtime hang.
- [ ] **Version fence.** A carton declares `cig >= x.y`; folding a carton into
      a runtime older than that refuses with a code that `doctor` recognises
      and can fix with `cig update`.
- [ ] Reserve a code block for fold errors (`E8xx` or whatever is free) and
      give each of the above its own number, so a CI log tells you which
      kind of broken without reading the message.

### Plan and rollback through folds

- [ ] **Folds never hide from the plan.** `--dry-run` at the top prints every
      op that would burn, all the way down, tagged with the path that owns it
      (`carton/pack/chain/step`). `--depth N` collapses for reading; the
      default is the full truth.
- [ ] **One journal per pack.** Every chain lit inside a pack burns into the
      same run record, so `cig unburn <run>` undoes the whole pack as one
      transaction and the cross-run problem (section 1) cannot happen
      between chains that share a pack.
- [ ] **Unburn unfolds in reverse.** Newest-first across chain boundaries,
      not just within one chain. Golden-test a pack of three chains where the
      third fails: all three roll back, in reverse, and the directory is
      byte-identical to before.
- [ ] **Dry-run needs the ghost filesystem** (section 1, chains). Without it,
      the second chain in a pack reads what the first one only pretended to
      write, and dry-run of any folded automation is dead on arrival. Build
      the overlay first; it gates this whole section.
- [ ] Partial-failure semantics are a decision, not a default: when chain 2
      of 3 fails inside a pack, does the pack roll back chain 1 (transaction)
      or keep it (checkpoint)? Transaction is the on-brand answer; document
      it, and make `{continue_on_error: true}` the explicit way to opt out.

### Automation of automation (unattended runs)

- [ ] A chain that lights other chains on a schedule or a file watch has no
      human reading the plan. Two things stand in for the human: the pack's
      scope, which refuses anything outside its declared paths, and the
      irreversible label, which should be able to say "this fold contains an
      irreversible op without a compensation, so it will not light unattended
      unless `{attended: false, allow_irreversible: true}`."
- [ ] Retries (`light(release, {retries: 2})`) interact with rollback: a
      retry after a failed burn must run against the rolled-back state, not
      the half-burned one. Test it with a step that fails once then passes.
- [ ] Concurrency: two scheduled packs that share a file. Either packs
      declare disjoint scopes and the fold check proves it, or the journal
      needs a lock per path. Decide before parallel chains land.
- [ ] Every unattended light writes the same run record a human run does, so
      `cig runs` is the audit log and `cig unburn` works on a run nobody
      watched.

### Reuse and discovery

- [ ] `cig chains` lists chains today; it should list packs and cartons too,
      with what each takes (`stamp(binary)`), what it touches (the scope), and
      whether it is fully reversible, so a pack is usable from its listing
      alone.
- [ ] Packs are parameterised (`pack(binary)` already is); keep the interface
      stable across versions, because a pack that changes its arguments
      breaks every chain that folded it. Same promise as a function
      signature in a library.
- [ ] A carton is the unit `cig update` and a future registry understand.
      Its manifest is the burn protocol's contract applied to a whole
      package: what it needs, what it touches, what it can undo.
- [ ] `cig init` writes a starter pack for the current repo, not just a
      starter `tasks.cig`.

### Things to hammer once it exists

- [ ] Fold a chain that fails `cig check`: refused, correct code, nothing
      written.
- [ ] Fold a pack with a literal path outside its scope: refused before run.
- [ ] Light a carton whose pack lights a pack that reads the first pack's
      output: dry-run passes on the ghost filesystem, real run succeeds,
      unburn restores everything, plan shows every op with its owning path.
- [ ] Kill the process mid-carton: `doctor` flags it, `unburn` restores all
      packs.
- [ ] Deep fold (carton → pack → pack → chain → chain): plan depth, error
      spans pointing at the right file and line at every level (see the
      `file:0:0` bug), and `--depth 1` reading as a sentence.
- [ ] Property test from section 10 extended to random fold trees.

---

## 7. Foreign scripts: keep your work, gain the safety (goal 3, the next update)

The intent: people plug their existing Python, bash, Node, Ruby and PowerShell
scripts into CigScript and get the plan, the scope and the rollback around
them, without rewriting anything and without the host language losing an
ounce of expressive power. The pitch flips from "rewrite your automation" to
"keep everything you have; from now on it runs inside a plan and can be
undone."

Design principle: **never touch the language, only its effect surface.**
Every language changes the world through a small set of doors. The
per-language work is knowing those doors; the integration is a shim that sits
on them. The moment safety costs expressiveness, people route around it.

### One mechanism, N adapters

- [ ] A foreign script is a **hop**. `light("deploy.py")` is `proc.run` plus a
      runtime adapter that knows how to find the interpreter, pass arguments,
      set the environment and read the exit code. Every resilience rule in
      section 4 applies to every language at once because there is only one
      seam.
- [ ] Adapters are cartons, not stdlib: `python`, `bash`, `node`, `ruby`,
      `pwsh` ship separately, version independently, and anyone can write
      one for a language you did not think of.
- [ ] Interpreter discovery is explicit and diagnosable: which `python` was
      used, from where, with which version, printed in the plan and in the
      error when it is missing. "python not found" must say where it looked.
- [ ] Arguments cross the seam as a list, never a string. Values that are
      not strings (maps, lists) cross as JSON on argv or stdin with a
      documented convention per adapter.

### Two tiers of integration

**Tier 1, black box.** Any script in any language runs as a hop inside a pack.
The pack's scope is hashed before and after; the diff is journaled; rollback
restores it. Zero integration in the script, works for languages nobody has
written an adapter for, does not know about dry-run.

- [ ] Scope diff journals every file the script created, changed or deleted
      inside the pack. Anything outside the pack is detected by a cheaper
      second scan of the script's cwd and refused or labelled.
- [ ] Dry-run of a black-box hop is honest: the plan shows the hop as
      "opaque: will run, effects inside the pack are reversible, outside
      unknown" rather than pretending to know.
- [ ] Network and process effects by a black-box script are, by definition,
      irreversible and labelled.

**Tier 2, white box.** A tiny `cig` module per language, one line at the top
of the user's script, nothing else changes. It instruments the language's own
effect APIs so the unchanged script participates in the plan and the rollback
like a native step.

- [ ] The shim reads `CIG_DRY_RUN` and, when set, simulates the effect doors
      instead of performing them, so a Python script's `shutil.move` shows up
      in the plan without moving anything.
- [ ] The shim reports effects to the kernel through the burn protocol
      (section 4): an effects manifest on a documented fd or stdout, with
      the path, the kind of effect and whether the shim can undo it.
- [ ] The shim never changes semantics outside the doors: same return values,
      same exceptions, same performance when not under `cig`. A script must
      behave identically with and without `import cig` when run directly.
- [ ] The shim is small and readable enough that a suspicious engineer can
      audit it in ten minutes, because that is exactly who will read it
      before letting it near a deploy script.

### Effect surfaces per language (the careful part)

- [ ] **Python:** `open` (write modes), `os.remove/rename/mkdir/rmdir/
      makedirs`, `shutil.*`, `pathlib.Path.write_*/unlink/rename/mkdir`,
      `subprocess.*`, `os.system`, `tempfile`. Instrument via monkeypatching
      in `import cig`, or an import hook. Watch for C extensions that write
      files directly (they bypass the shim; label the hop accordingly).
- [ ] **bash/sh:** there is no surface, everything is an effect. Tier 1 only,
      plus a `cig-sh` wrapper that exposes `cig_dry_run` as a function so
      scripts can guard their own destructive lines. Do not attempt to parse
      bash.
- [ ] **Node:** `fs` (sync, promise and callback variants, all three),
      `fs/promises`, `child_process`, and the popular wrappers (`fs-extra`).
      Instrument via a preload (`--require`) so the user's file is untouched.
- [ ] **Ruby:** `File`, `FileUtils`, `Dir`, `Kernel#system`, backticks,
      `Open3`. Instrument via `-r cig`.
- [ ] **PowerShell:** `Remove-Item`, `Move-Item`, `Copy-Item`, `Set-Content`,
      `Out-File`, `New-Item`, `Start-Process`. Instrument via a module that
      wraps the cmdlets, and treat `-WhatIf` as a native ally: it is the
      closest thing any other ecosystem has to your dry-run.
- [ ] For every language, a table in the docs: door, instrumented (yes/no),
      reversible (yes/no/with compensation). The table is the contract; the
      user reads it before trusting the shim.

### What the integration must never do

- [ ] No CigScript dialect of Python, no "allowed subset," no rewriting the
      user's source. Instrument the doors, leave the room alone.
- [ ] No silent scope expansion: a shim that sees a write outside the pack
      reports it and lets the kernel refuse; it never quietly widens the box.
- [ ] No laundering: a foreign hop is exactly as reversible as its manifest
      proves, and not one bit more. A Tier 1 hop that touched the network is
      irreversible in the plan even if the script "usually" cleans up.

### Things to hammer once it exists

- [ ] A Python script that writes, moves and deletes inside the pack under
      Tier 1: diff journaled, `unburn` restores byte-for-byte.
- [ ] Same script under Tier 2 with `CIG_DRY_RUN=1`: plan lists every effect,
      disk untouched, exit code 0.
- [ ] Same script run *without* `cig` at all: behaviour identical to before
      the shim existed (golden output, timing within noise).
- [ ] A Python script using a C extension that writes files: hop labelled
      "partially observed," diff still catches the file via Tier 1.
- [ ] A Node script mixing `fs.writeFileSync`, `fs.promises.writeFile` and a
      callback write: all three in the manifest.
- [ ] A bash script that `rm -rf`s inside the pack: rolled back by scope
      diff; outside the pack: refused or labelled, never silent.
- [ ] A script that spawns another script in a different language: the
      inner hop's manifest appears nested in the outer plan.
- [ ] Interpreter missing, wrong version, or a venv not activated: three
      distinct error codes, each saying where it looked.
- [ ] Windows: paths, line endings, PowerShell execution policy, and a
      Python installed from the Store. This is where foreign-script support
      usually dies quietly.

---

## 8. Error codes and quality of life (throughout development)

Error codes and small conveniences are the interface between the tool and
the person under pressure. They are cheap to add and expensive to change, so
this section is a standing list to keep open through every update.

### Error code discipline

- [ ] **Stable, reserved ranges, written down.** E1xx lex, E2xx syntax, E3xx
      check, E5xx runtime, E6xx cough, E7xx burn (as now), and reserve the
      rest before you need them: E8xx folds and packs, E9xx hops and foreign
      scripts, and a range for the updater and doctor. A code never changes
      meaning; a retired code is never reused.
- [ ] **Every code has an `explain`.** Test it: enumerate the codes in
      `diagnostics.rs`, assert each has an `ERRORS.md` entry and a `cig
      explain` page. CI fails on a new code without one.
- [ ] **Every diagnostic has a hint when a fix is knowable**, in the shape
      the E203 hint already has: what to type next, not what went wrong.
      Golden-test the full text so wording cannot regress silently.
- [ ] **Every diagnostic has a span.** No more `file:0:0` (chains). Errors
      that arise from a fold or a hop point at the step's definition and,
      where possible, the inner site too.
- [ ] **Every code knows which kind of no it is** (Part C): wrong address,
      never heard of you, these are mine, don't have any over here, or none
      of the four. The tag is in the registry and shows in `explain`.
- [ ] **A plain mode.** `--plain` or `CIG_PLAIN=1` keeps the codes and the
      hints and drops the catchphrases, for logs, classrooms and the
      procurement person. Same information, different first line.
- [ ] **Machine-readable everywhere.** `--json` on every command that can
      fail, with the code, span, message and hint as fields, so editors and
      CI can consume diagnostics without parsing prose.
- [ ] **Warnings are not errors and both are countable.** `cig check` exits
      0 with warnings, non-zero with errors, and `--deny-warnings` exists for
      CI.

### New codes the roadmap needs (reserve now)

- [ ] Fold refusals: won't parse, wrong shape (arity/params), literal path
      outside scope, cycle, version fence, irreversible-without-compensation
      in an unattended fold.
- [ ] Hop failures: command not found, refused to start, exit outside
      contract, timeout, killed by signal, bad shape on stdout, bad
      encoding, wrote outside the pack, interpreter missing / wrong version /
      venv not active.
- [ ] Kernel and journal: file changed since snapshot (`unburn` refusal),
      snapshot missing, run already rolled back, run interrupted (from
      `doctor`), journal on a different filesystem, home not writable.
- [ ] Ghost filesystem: read of a path that a simulated op deleted, so a
      dry-run can tell you "step 3 reads a file step 2 would have removed."
- [ ] Resource ceilings: step budget exceeded, allocation ceiling exceeded,
      output too large, both with the limit in the message and the flag to
      raise it.

### Quality of life, in rough order of how often it saves someone

- [ ] `cig init`: a starter `tasks.cig` (later a starter pack) for the
      current repo, with a `check` chain that already works.
- [ ] `cig unburn --dry-run`: the same plan format as `run --dry-run`,
      with a "changed since" column.
- [ ] `cig runs` filters and columns: by script, by status, `--failed`,
      `--interrupted`, `--since`, and an `irreversible` column so you can see
      at a glance what cannot be undone.
- [ ] `cig doctor --fix` for the boring things: prune, flag and offer
      `unburn` for interrupted runs, repair a missing snapshot directory,
      install shell completions.
- [ ] Shell completions for bash, zsh, fish and PowerShell, including chain
      and pack names from the file under the cursor.
- [ ] An editor grammar (tree-sitter first, TextMate for VS Code) and a
      language server later: colouring, `explain` on hover, jump to the
      pack a chain lights.
- [ ] `cig fmt`: one canonical layout, so diffs of `.cig` files are about
      meaning and review is easier.
- [ ] `cig watch <file> <chain>`: re-light on change, with the same plan and
      rollback, for the inner loop.
- [ ] `--depth N` on plans, and a `--why <op>` that prints the fold path
      (`carton/pack/chain/step:line`) that produced an op.
- [ ] Timings everywhere: per step (chains have it), per op, per hop, and a
      total, with the slowest highlighted when a chain takes more than a few
      seconds.
- [ ] A progress line for long hops (bytes read, seconds elapsed) so a
      41-second `cargo build` is not a blank terminal.
- [ ] `cig diff <run>`: show what a run changed, as a diff, from the journal.
      This is the "lab notebook" promise made visible.
- [ ] `cig update --rollback`, and `cig update --check` folded into
      `doctor`, so an update is never a surprise and never a trap.
- [ ] A `cig >= x.y` line at the top of a script, so the error for an old
      runtime is "this script wants a newer cig" with the command to fix it.
- [ ] `NO_COLOR`, `--color=never`, and a plain-ASCII mode for terminals that
      cannot draw the stick.
- [ ] `cig explain` accepts a run id and a step, not just a code: "explain
      why step 3 failed" reads the journal and the diagnostic together.
- [ ] Environment variables for every flag people will set in CI
      (`CIG_DRY_RUN`, `CIG_PLAIN`, `CIG_MAX_STEPS`, `CIG_NO_UPDATE_CHECK`),
      documented in one table.
- [ ] Reproducible bug reports: `cig report <run>` bundles the plan, the
      journal, the diagnostic and `doctor` output, redacted the same way
      crash reports are, into one file to attach to an issue. This is what
      makes the Part B challenge true for non-crash bugs.

### Things to hammer for this section

- [ ] The code-coverage test: every emitted code has an explain page.
- [ ] Golden tests for every diagnostic in both themed and plain mode.
- [ ] `--json` output validated against a schema in CI.
- [ ] Completions generated from the CLI definition, never hand-written, so
      they cannot drift.
- [ ] `cig fmt` is idempotent: formatting twice changes nothing.

---

## 9. Doctor: automatic diagnosis and the error-code web

The intent: when an error code fires, `doctor` fires with it, and instead of a
code and a caret the person gets a diagnosis: why the tool thinks this
happened, what it checked to reach that conclusion, and exactly what needs to
happen to fix it. Behind that sits a deep, nuanced web of error codes, where
a code is not a number but a node with causes, checks and remedies attached.

Two principles keep this from becoming noise or danger:

**Doctor diagnoses automatically; it never fixes automatically.** Firing on
an error runs read-only probes and prints a diagnosis. Any change to the
world still goes through the ask-before-fix step doctor already has, and a
fix that needs a burn shows up as a plan. A doctor that "helpfully" repairs
things is the one bug the burn model exists to prevent.

**Doctor only claims what it verified.** Every "I think" comes with "because
I checked X and found Y." Guesses are labelled guesses, ranked, and shown
with what would confirm them. In the lexicon: doctor is the one that points,
and it is never the smug bastard.

### The web: codes as nodes, not numbers

Build the code system as **data**, one registry that every command reads,
never as `if` statements scattered through the code. From that registry you
generate `ERRORS.md`, the `explain` pages, the `--json` schema and doctor's
own knowledge, so they cannot drift apart.

Each code carries:

- [ ] **Identity:** code, family (lex, syntax, check, runtime, cough, burn,
      fold, hop, journal, ghost, resource, update), one-line meaning, and a
      subcode where the family needs nuance (`E9xx` hop failures split into
      not-found / refused / exit-outside-contract / timeout / signal /
      bad-shape / bad-encoding / outside-pack / interpreter-missing).
- [ ] **Which kind of no** (Part C), where one applies.
- [ ] **Where it can arise:** parse, check, dry-run, run, unburn, fold,
      doctor, update. The same code in a dry-run and a real run may need a
      different diagnosis.
- [ ] **Causes, ranked:** an ordered list of known reasons this code
      appears, each with a read-only **probe** that confirms or rules it out
      (file exists? permission bits? which interpreter on PATH? journal
      status of the last run? disk free? path inside the pack?).
- [ ] **Remedies, per cause:** the exact action, as a command where one
      exists (`cig unburn <id>`, `chmod`, `cig update`), as a code change
      with a before/after snippet where it is a script fix, or as "ask a
      human" where it is neither. Each remedy says whether doctor can do it,
      whether it needs a burn, and whether it is reversible.
- [ ] **Relations:** codes that commonly precede or follow this one (a
      timeout that led to an interrupted run; a fold refusal that explains a
      later scope error), so doctor can say "this is probably the result of
      the E9xx three seconds earlier."
- [ ] **Tags:** beginner-likely, ci-likely, platform-specific (Windows,
      macOS), fixable-by-doctor, needs-burn, irreversible-involved.
- [ ] **Stability:** a code never changes meaning; a retired one is never
      reused; the registry is versioned with the runtime so old run records
      still decode.

### Firing model

- [ ] **Hook, not wrapper.** Every diagnostic passes through one emit point;
      doctor subscribes there. No command has to remember to call it.
- [ ] **Budget.** Auto-diagnosis has a time budget (tens of milliseconds for
      the common cases, a hard cap for the expensive probes) so an error
      never becomes slower than the run that produced it. Expensive probes
      run only under `cig doctor <run>` or `--deep`.
- [ ] **Read-only, always.** Probes read files, stat paths, list PATH, read
      the journal. They never write, never spawn the failing command again,
      never touch the network (except an explicit update check the user
      opted into).
- [ ] **Offline first.** Diagnosis needs nothing from the internet. The
      crash-report pipeline is separate and stays opt-in.
- [ ] **Quiet when it has nothing.** If no probe confirms a cause, doctor
      prints one line ("no known cause matched; `cig explain E503` for the
      general case") rather than a wall of speculation.
- [ ] **Off switch and CI mode.** `--no-doctor`, `CIG_DOCTOR=0`, and in
      `--json` the diagnosis is a field, not prose.
- [ ] **Re-diagnose later.** `cig doctor <run>` re-runs the diagnosis
      against the stored run record, so an error seen in CI can be
      investigated on a laptop.

### Output shape (the intuitive part)

Layered, so a beginner reads the first two lines and an engineer reads all
of it. Same shape for every code, so people learn to scan it once.

- [ ] **Verdict, one line.** What went wrong in plain words with the code:
      "step 3 (deploy) could not run `kubectl`: not found on PATH [E9xx]".
- [ ] **Why doctor thinks so.** The confirmed cause and the evidence:
      "I looked for `kubectl` in the 7 directories on PATH; none had it. A
      `kubectl` exists at `~/bin/kubectl` but `~/bin` is not on PATH for
      this run because `proc.run` uses a clean environment."
- [ ] **Fix, exactly.** The command or the code change, ready to run or
      paste: "add `env: {PATH: ...}` to the `proc.run` on line 14, or light
      with `--inherit-path`." When doctor can do it: "run `cig doctor --fix
      <run>` to apply (reversible, no burn needed)."
- [ ] **If that is not it.** The next ranked causes, each with the one
      check that would confirm it, so a wrong first guess costs the person
      seconds, not a rabbit hole.
- [ ] **Context that changes the answer.** Platform, `cig` version, whether
      this was a dry-run, whether an earlier error in the same run is the
      real culprit, whether the same code appeared in previous runs of this
      script ("third time this week; the fix from run 2024... was X").
- [ ] **Details on demand.** `--verbose` shows every probe run and its
      result, so an engineer can audit the reasoning instead of trusting it.
- [ ] **Plain mode and themed mode** share every word except the first line.

### Nuance to build in

- [ ] Multiple candidate causes, ranked by probe results, never a single
      confident guess when the evidence is thin.
- [ ] Beginner-aware phrasing chosen by tag, not by guessing the user:
      beginner-likely codes get the "what to type next" phrasing first;
      ci-likely codes get the machine-readable line first.
- [ ] Cross-run memory: doctor reads the run history for the same script
      and surfaces repeats and past fixes.
- [ ] Chain and fold awareness: the diagnosis names the owning path
      (`carton/pack/chain/step:line`) and, for interrupted runs, offers the
      unburn with what it would restore.
- [ ] Foreign-script awareness: when a hop fails, doctor knows the adapter
      (which Python, which venv, which Node) and diagnoses the interpreter
      before the script.
- [ ] Platform awareness: the same code on Windows gets Windows remedies
      (execution policy, path separators, Store Python).
- [ ] Honest limits: for codes with no known probe (an arbitrary `cough`
      from user code), doctor says so and shows the `cough` site and the
      values in scope at the time, which is the most useful thing it can
      truthfully offer.

### Things to hammer

- [ ] **A corpus of failing scripts,** one or more per code, with the
      expected verdict, cause and fix as golden output in both modes. This
      is the accuracy test; it grows with every bug report.
- [ ] **Probes are read-only,** proven by running the whole corpus with the
      working directory read-only and the journal directory mounted
      read-only: any write is a failure.
- [ ] **Latency:** every auto-diagnosis under its budget, measured in CI.
- [ ] **Registry coverage:** every emitted code exists in the registry, has
      at least one cause with a probe or an explicit "no probe" entry, an
      explain page, and a golden test. CI fails otherwise.
- [ ] **False positives:** deliberately construct errors where the first
      ranked cause is wrong, and check the "if that is not it" list contains
      the right one.
- [ ] **Relations:** a timeout followed by an interrupted run produces one
      diagnosis that links them, not two unrelated ones.
- [ ] **Off switch:** `--no-doctor` output is byte-identical to today's
      diagnostics.
- [ ] **`--json`** diagnosis validates against the schema, and the schema
      is generated from the registry.

---

## 10. Test infrastructure (makes everything above cheap)

- [ ] **Property test for rollback.** Generate random sequences of ops
      (write, append, delete, move, copy, mkdir, over random small trees,
      including symlinks and existing targets), burn them into a temp dir,
      roll back, assert the tree is byte-identical to before (content, names,
      modes, link-ness). One test, every spot at once. It would have found the
      symlink bug in seconds and it will find the next three.
- [ ] Same property for `unburn` after a successful run, and for "kill the
      process at a random point mid-burn, then unburn" (spawn `cig` as a child,
      SIGKILL it at a random delay).
- [ ] `cargo-fuzz` on the lexer, parser and `json.parse`. Interpreters live and
      die on this and it costs an afternoon.
- [ ] Golden tests for every diagnostic's full text, so the beginner-facing
      wording cannot regress silently.
- [ ] CI matrix: Linux, macOS, Windows (paths, symlinks, case-insensitivity
      all differ). The README promises macOS and Windows binaries; the tests
      have to run there first.
- [ ] `cargo deny` / `cargo audit` in CI, and a pinned MSRV check (README says
      1.82+).
- [ ] A benchmark for the journal: ops/second with and without fsync, so the
      durability fix has a number attached.
- [ ] Every regression test is named after the cheat it covers, so the wall
      (Part B) and the suite are the same list.

---

## 11. Adoption (so the goals reach anyone)

- [ ] **The wedge is chains.** Nobody rewrites their Python to try CigScript;
      plenty of people would replace a Makefile with a `tasks.cig` next to
      existing code, because a build script with dry-run and rollback is the
      place devs are most scared today. A `cig init` that writes a starter
      `tasks.cig` for the current repo makes the first minute free.
- [ ] **The challenge is the marketing** (Part B). "Break it" on the README
      is the pitch, the wall is the proof, and every fixed cheat is a release
      note people will read.
- [ ] The playground (section 3) serves this audience too: paste a bash
      script's logic, see the plan.
- [ ] Signed releases (roadmap item 1). `cig update` verifies integrity but
      not origin; a self-updating binary without signatures is the first thing
      a security-minded reviewer will refuse.
- [ ] Homebrew tap and a `cargo binstall` entry, so install is one line on
      every platform.
- [ ] Issues for the verified bugs, with the repro scripts above, so the
      commit history starts telling the story that a single commit cannot,
      and so the wall has its first "fixed in" rows with links.