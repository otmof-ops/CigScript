# Hops: running other programs

<!-- SPDX-License-Identifier: CC-BY-4.0 -->

A **hop** (the lexicon's word for a process boundary) is the one place the
kernel performed nothing, saw nothing and can undo nothing on its own.
Inside the language you have fenced effects, checked arithmetic, a journal
and a plan; the moment a hop happens all of it is gone unless the hop is
designed to carry it. So the boundary is where the logic is hardest: nothing
crosses a hop unchecked, nothing fails silently, and a failure names the hop.

## One call shape

```cig
stick r = proc.run("git", ["status", "--porcelain"])     # {code, out, err, duration_ms, ...}
stick prs = proc.json("gh", ["pr", "list", "--json", "number,title"])   # a list of maps
stick files = proc.lines("git", ["ls-files"])             # a list of strings
stick rows = proc.csv("sqlite3", ["-csv", "-header", "db", "select * from t"])
stick conf = proc.kv("git", ["config", "-l"])             # KEY=value lines as a map (a leading `export` is dropped)
stick text = proc.text("date", ["+%F"])                   # stdout as one string
```

Every verb is sugar on `proc.run` and takes the same options. The parsing
verbs read **stdout only**, enforce the contract `ok: [0]` unless told
otherwise, and fail at the hop that produced a bad shape, quoting the first
200 bytes of what they got, so a wrong shape from step A never sails through
to die in step C.

| option | meaning |
|---|---|
| `cwd` | working directory for the child; it must exist (`E550` names it otherwise) |
| `env` | a map of variables added to the child's environment |
| `clean_env` | start from a small documented environment (`PATH`, `HOME`, `USER`, `LANG`, `TERM`, `TMPDIR`, `TZ` and the Windows equivalents) instead of inheriting |
| `timeout_ms` | wall-clock limit; ten minutes by default |
| `grace_ms` | after SIGTERM, how long before SIGKILL; 500 ms by default |
| `stdin` | text fed to the child's stdin |
| `touches` | globs, relative to the pack root, of the files the child may change (`"*"` the root's own files, `"**"` everything under it, `"src/**"` a tree, `"package.json"` one file). Each existing file under them is kept in the tin before the child runs, so a file it modifies or deletes comes back on rollback, byte for byte and hash-verified, with its mode and modification time, where it would otherwise be listed as irreversible. A symbolic link comes back as a link with or without `touches`. Needs a `pack { }` (`E757`) |
| `ok` | the exit-code contract: an int or a list of ints; `null` for none. It wins over `check` whatever the order of the keys |
| `check` | `true` means the contract `[0]`, `false` none; ignored when `ok` is given |
| `encoding` | `utf-8` (strict, the default), `lossy`, `latin1`, `utf-16` |
| `header`, `sep` | for `proc.csv` |

Every child gets `NO_COLOR=1` (its output is captured, colour is noise) and
`CIG_RUN_ID` when there is one. The environment is otherwise inherited plus
what `env` adds, so a chain behaves like the shell it replaced; `clean_env`
is there for the chain that must behave the same on the laptop and in CI.

## The rules, each with a test in `tests/hops.rs`

**Exit codes are a contract, not a boolean.** `grep` says 1 for "no match",
`diff` says 1 for "different"; those are answers. `proc.run` alone passes
any code through in `.code`. The parsing verbs, and `proc.run` under `check`
or `ok`, refuse anything outside the contract with `E553`, naming the
command, the code, the contract and stderr's last line; the whole result is
the ashtray value.

```
error[E553 runtime]: proc.text: `kubectl apply -f deploy.yml` exited 1 (contract: 0); stderr: `Error from server (Forbidden)`
  = hint: if that code is an answer, not a failure, say so: {ok: [0, 1]}; read err in the ashtray value otherwise
```

**stdout and stderr are never merged.** Structured data comes from stdout;
stderr is captured separately and shown on failure. A tool that prints
progress to stdout breaks the shape, and `E556` says so: *expected JSON on
stdout, got `Downloading...⏎[1]`*.

**Timeouts always exist.** Every hop has one. `E554` says which hop stalled,
how much it had written to each stream, and quotes the start of it.

**Partial output is never mistaken for output.** A child killed by the
timeout or by a signal (`E555`, with the signal's number and name) produced
*nothing* as far as the next step is concerned. Its truncated output is in
the message, not in a value.

**Large output does not deadlock.** Both streams are read concurrently; a
child filling stderr while stdout is awaited is the classic hang, and the
test does it with 20 MB each way. `stdin` is written from its own thread
beside the readers, so a child that echoes 8 MB straight back does not hang
either.

**Encoding is explicit.** Output that is not valid UTF-8 is `E557`, with the
byte offset, never a silent replacement. `encoding: "lossy"` asks for
replacement characters; `"latin1"` for single-byte text; `"utf-16"` for a
Windows tool (a byte-order mark is honoured, little-endian assumed without).

**`sudo` cannot ask for a password here, and `E554` is what you see.** A child's stdin is `/dev/null` unless `stdin` is given, and the child is its own process group, not the terminal's foreground group; `sudo` reads the password from `/dev/tty`, and from a background group that read stops the process until the hop's timeout. Run `sudo -n` so it refuses instead of waiting, have the operator run `sudo -v` first, or configure the command in sudoers.

**No zombies, no orphans.** Every child runs in its own process group. On
timeout the group gets SIGTERM, a grace period, then SIGKILL, grandchildren
included; a child that ignores SIGTERM still ends. A Ctrl-C at the terminal
reaches cig, not a child in its own group, so SIGINT, SIGTERM and SIGHUP are
forwarded to every live child group before cig ends the default way.

**On Windows the process tree is a Job Object.** There is no process group
to signal, so every hop's child is put in a job with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: what the child starts joins the job,
a timeout terminates the job (the grandchild holding the pipe included, so
`timeout_ms` ends the task when it says), and cig itself dying closes the
last handle and takes the tree with it. One difference from Unix follows:
what a hop leaves running in the background ends with the hop.

**No shell by default.** Arguments are a list, never a string handed to `sh
-c`, so there is nothing to inject into. `proc.shell(command)` exists for the
honest cases, by name, with the warning in its name. `proc.which(name)`
resolves a bare name through PATH and a name with a slash against the
current directory, exactly as `proc.run` will; a directory given as the
command is refused as one (`E552`), not as a permissions problem. `cig check` warns
(`E308`) when a shell is smuggled through `proc.run("sh", ["-c", built])`
with a command string built at run time.

**The irreversible label survives the hop.** A hop is journaled as
irreversible and the plan says so; `proc.pipe` and `proc.shell` are one hop
each. Nothing launders it.

**Failure taxonomy with stable codes.** `E550` hop failed (other), `E551`
command not found, `E552` refused to start, `E553` exit outside the
contract, `E554` timed out, `E555` killed by a signal, `E556` bad shape on
stdout, `E557` bad encoding. A CI log tells you which without the message;
each has its kind of no where one fits.

**Retries are per-hop and idempotence-aware.** A chain retry
(`light(chain, {retries: n})`) first rolls back what the failed attempt
burned, so the retry runs against the state the step started from, and the
journal marks the undone ops so `cig unburn` skips them. A step whose failed
attempt ran a hop is **not** retried on its own, because the world may
already have changed; `{retry_irreversible: true}` says you know.

**Determinism holds across hops.** Same inputs, same plan. A child's output
may vary; the plan does not, because CigScript itself did nothing different.

## How a program is found

One resolver answers for `proc.run`, `proc.which`, every `proc.pipe` stage
and the shell behind `proc.shell`, so what `proc.which` names is what starts.

- A name with a path separator is a path, made absolute against the hop's
  `cwd` (`./gradlew` with `cwd: "api"` is `api/gradlew`), on every platform.
  Windows itself would resolve it against the parent's directory.
- A bare name is searched on `PATH`, never in the current directory. On
  Unix the first executable file wins. On Windows each directory is tried
  with each `PATHEXT` extension in order (`.COM;.EXE;.BAT;.CMD` by default),
  so `npm` is `npm.cmd` and `node` is `node.exe`; a name that already ends
  in one of them is taken as spelled. A `.bat` or `.cmd` found this way is
  started by its full path, which Rust runs through `cmd.exe` with safe
  argument quoting and refuses arguments it cannot quote.
- `bash` and `sh` on Windows mean a shell: System32's `bash.exe` (the WSL
  launcher) and `wsl.exe` are never chosen, whatever the order of `PATH`.
  With no other shell on `PATH` the hop fails as `E551` and the hint names
  Git for Windows.
- `proc.shell` runs its line with `sh -c` everywhere an `sh` is found, Git
  for Windows and MSYS2 included; without one it runs `cmd /d /s /c` and
  the line reaches cmd exactly as written, in one pair of quotes that `/s`
  strips. The plan line says which (`run sh -c …` or `run cmd /d /s /c …`),
  so a dry run is honest about the shell.

## Pipelines without a shell

```cig
stick errors = proc.pipe([["cat", "big.log"], ["grep", "ERROR"], ["sort", "-u"]])
exhale errors.out, errors.stages   # every stage's command and exit code
```

Stages are connected stdout to stdin, run in one process group under one
timeout, and every stage must exit 0 (the last one honours `ok`); a failure
names the stage (`E553: stage 2 (grep ERROR) exited 1`), and a missing
program names its stage (`E551`). `stdin` feeds the first stage. A stage
that died of SIGPIPE because the stage after it stopped reading (`yes | head
-1`) is not a failure: its entry in `.stages` says `closed_early: true` and
`signal: 13`, and whatever the consumer did next is what counts.

## Inside a pack: the hop's write set

With `pack { "./build" }` declared, the kernel lists the pack's files (mtime
and size) before a hop and again after, and journals the difference: files
the child created are reversible (`hop created`; rollback removes them),
files it modified or deleted are irreversible with the detail, and anything
the child changed in its working directory outside the pack is reported as
`E752` and never laundered. `BURN.md` has the rules.

## Compensations

`burn (s) { ... } unburn { ... }` is the undo for what no pack can reverse
(`git push`, an API call). The compensation runs from the journal alone, so
it may use only its state map `s`, modules and builtins (`E754` otherwise);
hops inside the block show as `compensated` in the plan. `LANGUAGE.md` has
the shape.

## What a hop cannot do yet

The burn protocol (a child that speaks `CIG_DRY_RUN=1` and reports its own
effects so the kernel can plan and journal them), adapters for `git`, `gh`,
`docker` and friends as subtools, and shape assertions at the boundary are
the next seams of `HAMMER.md` (sections 4 and 7). Until then a hop is honest about being opaque: the plan
shows it, the label says irreversible, and the ghost filesystem does not
pretend to know what it would have written.
