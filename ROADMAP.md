# Roadmap

What the next updates hold, in the order the first operations scripts asked for them
([issue #18](https://github.com/otmof-ops/CigScript/issues/18), from a Go project whose
operations are CigScript files, and [cig-orchestrator #12 and #13](https://github.com/otmof-ops/cig-orchestrator/issues)).
Each lands with a wall row and a test named after it, like everything else.

| # | Feature | Closes |
|---|---|---|
| 1 | `proc.run(cmd, args, {stream: true})`: the child's stdout and stderr teed to the parent's stderr as they arrive, the captured value unchanged | a ten-minute build silent until it ends (#18 item 10; cig-orchestrator #12) |
| 2 | `net.fetch(url, dest, {timeout_ms, max_bytes, headers})`: a burn that writes into the pack, journaled like `fs.write_text`, simulated in a dry run as a write of unknown size; HTTPS unless told otherwise | every download a `curl` hop the dry run cannot see (#18 item 1) |
| 3 | A raw multi-line string literal (`'''…'''`, no interpolation) | unit files and config as `"\n"`-joined lists (#18 item 3) |
| 4 | `proc.jsonl`, or `proc.json` with `{lines: true}`: one checked value per non-empty line | `go test -json`, `docker inspect` streams (#18 item 4) |
| 5 | `time.parse_iso(s)` and `time.parse_duration("30d")` | a certificate's remaining life only checkable by exit status (#18 item 5) |
| 6 | `script.path`, `script.dir` (pure); `run.id` landed in 1.2.0 as `cig.run_id()` (null in a dry run), beside `cig.version()`, `cig.os()`, `cig.arch()` | a script that cannot find its templates (#18 item 6) |
| 7 | `fs.write_text(path, text, {sensitive: true})`: the before-state's hash journaled, the op irreversible and said so in the plan | an overwritten private key kept as a plain snapshot copy (#18 item 9) |
| 8 | Lighting another file's chain in-process, so a `.cig` file run from a chain shares the journal: its burns, compensations and plan become the outer run's | nested `cig run` whose compensations the outer rollback cannot reach (cig-orchestrator #13) |
| 9 | The tin in chunks: content-defined chunking (FastCDC) of the files `touches` names, so a large file edited in one place stores the chunk that changed, not the file again | a 200 MB lockfile or asset stashed whole on every run (cig-orchestrator #18 step 5; 1.2.0 stashes whole files, one object per distinct content) |
| 10 | Git blob reuse: a file whose bytes git already holds (`git hash-object`) is referenced from the repository's object store instead of copied into the tin | the tin doubling what `.git/objects` keeps (cig-orchestrator #18 step 6) |
| 11 | The hash chain: each journal line carries the hash of the one before it, `cig runs <id> --verify` walks it | a journal edited after the fact passing as the record (cig-orchestrator #18 step 7) |
| 12 | Sealed chunks and the burn protocol: a tin object written once and never rewritten, the before-bytes of an irreversible hop exchanged with the child through a declared protocol | the kernel trusting a hop's word about what it touched (cig-orchestrator #18 steps 9 and 10) |
| 13 | The box: one run's record, tin objects and compensations exported as a single file and imported on another machine for `cig unburn` there | undoing a deploy from the machine that did not run it (cig-orchestrator #18 step 11) |

Done in 1.2.0 instead of waiting: the tin for whole files (`touches`, `cig tin`, `.cigignore`;
cig-orchestrator #18 steps 1 to 4 and 8), the pin, the humidor and the trial (#22), crash
reports that name the function and put the run back (#23, #24), and the `cig.*` namespace.

Done in 1.1.2 instead of waiting: `HOPS.md` says what `sudo` does from a hop and
what to do about it (#18 item 2); `LANGUAGE.md` says a keyword cannot be a
declared name and names the error (#18 item 7); `DESIGN.md` says heavy data work
belongs in a program the script hops to (#18 item 8).

The longer list that was already here is in `docs/DESIGN.md` under "What comes next".
