# Changelog

All notable changes to CigScript. The format follows Keep a Changelog; the
project follows semantic versioning.

## 1.6.0 — unreleased

- **The parcel** (#86). `cig parcel <run>` writes `<run>.cigparcel`: the
  run's record, journal and after-state, every snapshot, tin object, recipe,
  chunk and sealed object its journal refers to, the bytes of every blob
  git held for it (carried whole), and a manifest naming each file's
  SHA-256, the cig that wrote it, the pack's roots and the working
  directory. `cig unparcel <file> --root name=path ...` checks every file
  against the manifest before it writes anything (a changed byte refuses the
  whole parcel), needs every root mapped to a place here (`--show` lists
  them; `--cwd` when the working directory cannot be told from the roots),
  refuses sealed objects this machine's key does not open unless
  `--allow-sealed-missing`, places the objects in the tin and the run among
  the runs, and records in the run's record where it came from and how its
  paths map. The journal is never rewritten, so its chain still verifies;
  `cig runs <id>` says `imported`, and `cig unburn <id>` works here with the
  usual drift check against this machine's files. `E762` names a refusal.
- **The burn protocol** (#89). Every hop a real run starts gets
  `CIG_JOURNAL`, a file it may append JSON lines to: `write`, `delete` and
  `create` say what it changed (with the hash of the bytes before, and where
  it kept a copy of them), `compensate` says how to undo it (a command, no
  shell). The kernel never trusts a hop's word about bytes it can check: a
  reported path outside the pack is refused (`E752`), a `create` the scan
  saw before the hop is dropped, a claim about a file the tin kept is
  checked against the tin, and a change not under `touches` becomes
  reversible only when the tool kept the bytes and they hash as claimed
  (`(reported by the hop)` in the record); the rest stays irreversible with
  the reason (`E761`). A reported compensation is journaled like a block's
  and runs on rollback and `cig unburn`, from the directory it names, with
  `CIG_RUN_ID` set. cig is the first speaker: a `cig run` a hop started
  appends its own undo, `cig unburn <its run>`, so the parent's rollback
  puts it back through its own exact journal. `docs/PROTOCOL.md` is the
  contract, with the lines in Python, Node and bash.

## 1.5.0 — 2026-10-28

- **The tin in chunks** (#82). A file of 64 KiB or more under `touches` is
  cut into content-defined chunks (FastCDC: 16 KiB at least, 64 KiB on
  average, 256 KiB at most), each an object named by its own hash, and the
  file becomes a *recipe* (`tin/recipes/<sha256>`): the ordered chunks, the
  hash and the size of the whole. A one-byte change stores the chunk it
  landed in, not the file again; an insert moves one boundary. A restore
  checks every chunk against its name and the whole against the hash the
  journal recorded, and refuses (writing nothing) on a missing or corrupt
  chunk. `cig tin` reports what is stored next to what the kept files would
  take; `gc` keeps every chunk a live recipe names. What a rollback
  displaces into the tin is chunked the same way, so a hop's output that is
  the original with one change shares every other chunk with it. Objects a
  1.4 cig wrote (large files kept whole) restore as they always did.
- **Sealed: the tin encrypted at rest for secrets** (#83). A file the tin
  keeps that is a secret is sealed: XChaCha20-Poly1305 in the STREAM
  construction (no crypto of our own), a megabyte a segment, every segment
  authenticated and the last one marked, so a byte flipped, a tail cut off
  or another key refuses the restore and writes nothing. Sealed objects are
  named by a random id and never shared, so two equal secrets stay two
  objects that tell nothing about each other; the journal keeps the
  plaintext hash, so the bytes are checked on the way back. What counts as a
  secret: a built-in list of names (`.env*`, `*.pem`, `*.key`, `id_rsa*`,
  `credentials*`, `*secret*`, `.npmrc`, `kubeconfig`, `*.kdbx`, and more), a
  pack root's `.cigseal` (the same shape as `.cigignore`), a hop's
  `touches_sealed: [...]`, and `fs.write_text(p, t, {sensitive: true})`;
  the `seal` setting widens it to `all` or turns it off with `never`. The
  key lives in the OS keyring (Secret Service, Keychain, Credential
  Manager), made on first use; where there is none, in
  `~/.cigscript/tin/seal.key`, readable by its owner alone (`seal_key =
  file` makes that the rule). A run's own snapshots of secrets are sealed
  the same way, inside tree snapshots too, and what a rollback displaces
  from a sealed file stays sealed. The plan says `(sealed)`; `cig tin`
  counts sealed objects apart; `cig tin verify` opens every one (`--quick`
  looks at the shape); `cig tin gc --sealed-orphans` clears objects the key
  cannot open any more.
- **Git blob reuse** (#87). A file under `touches` that git already holds,
  tracked and clean against the index (the bytes on disk hash to the blob
  the index names), is referenced from the repository's object store
  instead of copied into the tin: the journal records the repository, the
  blob and our own SHA-256, and the restore reads the blob back through
  `git cat-file`, checks it against that hash and refuses other bytes. The
  run pins every blob it refers to under `refs/cig/runs/<id>` (one tree
  naming them all) so a `git gc` after a rebase cannot take them; `cig runs
  --prune` drops the pin with the record. A dirty or untracked file goes to
  the tin as before; a repository the run cannot pin in is copied too. Git
  is only ever read, the pin aside. `(git)` in the record names the keeper.
- **The hash chain** (#88). Journal format 2: the first line carries the
  hash of the run's id and every later line the hash of the line before it
  as written; `run.json` records the head when the record is saved.
  `cig runs <id> --verify` walks the chain, compares the head with the
  record and checks every snapshot and tin object a rollback would need.
  `cig unburn` refuses a journal whose chain is broken (`E760`) unless
  `--force`. A format 1 journal is read as it always was and gains no
  chain. This is tamper evidence, not secrecy: whoever can write the
  journal can recompute the chain, unless the head is kept somewhere they
  cannot write.

## 1.4.3 — 2026-10-28

- **Changes outside `touches` are reported once per folder** (#90). A hop
  that deleted a thousand-file folder it had not named produced a thousand
  `E757` lines, one per file, and buried the two that mattered. The files
  under one top-level folder are now one line: how many, the smallest
  directory that covers them, the one glob that would have kept them, and a
  few of them as examples; `cig runs <id>` lists every one. A lone file is
  still named itself.
- **`cig unburn <id> --give-back` can be previewed** (#91). `--give-back
  --dry-run` lists each file the rollback moved into the tin, with what the
  tin holds for it, what is at the path now and whether that is still what
  the rollback left, and changes nothing. A path changed again since the
  rollback is refused without `--force`, as a rollback over a later edit is
  (`E704`), and the preview says so.

## 1.4.2 — 2026-10-28

- **A burn can carry a name** (#80): `burn ("deploy") { } unburn { }`, or
  `burn (s, name) { }` with a state map, any expression that is a string.
  The plan, the run record and a rollback name the burn by it instead of by
  its line: `deploy is NOT undone: its compensation failed: …`, where a tool
  written in CigScript used to be quoted by a line of its own source.
  Without a label the line stands in, worded once (`the burn at line 12 was
  not undone: its compensation failed: …`).
- **A `touches` glob written with a leading `?` may match nothing without a
  word** (cig-orchestrator #48): `?yarn.lock` is a file the project may not
  have yet, not a typo; the `?` is not part of the glob, and a file created
  under it is kept like any other.
- Each warning about a hop's watch is printed once; 1.4.0 said the ones from
  before the hop twice.

## 1.4.1 — 2026-10-28

- **A link a hop makes is the pack's entry, wherever it points.** 1.4.0 judged
  a hop-created link by where it led, so one pointing out of the pack was
  reported as a write outside the pack (`E752`) and left in place on
  rollback. It is judged by where it sits: rollback removes it, and what it
  points to is never touched.

## 1.4.0 — 2026-10-28

Found by two passes over everything 1.3.0 and 1.3.1 added, the second an
outside one over hop watching and the tin; every fix has a test on the
platforms that can run it. The minor number moves for three things a user
can see: `cig unburn <id> --give-back`, the pack lock (`E759`) and the
`watch_cap` setting.

- **A rollback removes nothing that existed before the run** (#71). Hop
  watching reads a pack up to a cap; past it a file first seen after the hop
  could not be told from one the scan never reached, and rollback removed
  it. Now a scan that stopped at the cap says so up front, journals nothing
  as created, and asks the disk before it calls a file deleted. The cap is
  the `watch_cap` setting (50,000 entries). A link that leads out of the
  pack is a door, not a part of it: nothing behind it is walked or
  journaled, what a hop writes through it is reported as outside the pack
  (`E752`), and a rollback never removes a path whose real place is
  outside the pack.
- **A rollback destroys nothing it did not put there** (#72). Hop watching
  cannot tell the hop's bytes from a save made while it ran, so a file a
  hop entry's rollback removes or rewrites goes to the tin first, the
  record says so, and `cig unburn <id> --give-back` puts it back (and moves
  what is there now to the tin in turn, so the step is as reversible as the
  rollback was). A file the hop left that changed again after the hop ended
  is left alone, with `--force` for the old bytes on purpose. Two runs on
  one pack would roll back each other's work: a run takes the pack's lock,
  and a second run (or an unburn) on the same or an overlapping pack is
  refused with `E759`, naming the holder. A cig a hop starts (`cig run
  inner.cig` as a task) shares its parent's pack, since the parent waits
  for it; every hop now sees the run's id as `CIG_RUN_ID`.
- **The tin cannot be poisoned** (#73). A file changing while it was being
  stashed left an object that did not hash to its name, and every later
  stash of that content reused it. The copy is hashed as it is written and
  named by what arrived; an object already there is trusted only when it
  hashes to its name, and replaced when it does not.
- **`cig tin gc` during a hop keeps what the hop stashed** (#74). The run
  lists the objects it is about to depend on in its directory before any
  exists under its name, and `gc` reads that list.
- **A folder a hop replaced with a file comes back as the folder** (#75),
  with what was in it, the file gone to the tin; **an empty folder a hop
  deleted comes back** (#76), with its mode.
- **A `touches` that matches nothing is said** (#77): before the run, when
  the whole list covers no file, with the nearest glob that would (one glob
  among several matching nothing is ordinary: a lockfile the project may
  have); in the dry run, as a count beside each glob; and after the run,
  naming the glob that would have kept a file the hop changed outside every
  glob.
- **Links.** A dangling link is watched like any other, so a hop that
  deletes or retargets one is undone; a write through a link to a file
  outside the pack is reported (`E752`) instead of going unseen.
- **Hops.** The timeout covers the hop's output, not only its child: a
  process the child left behind holding the pipes held the hop forever;
  now the hop ends at its deadline (`E554`, naming the stream), the rest of
  its process group with it. `proc.pipe` feeds `stdin` from a thread and
  starts its clock before the first stage, so a first stage that never
  reads its input cannot stop the timeout. On Windows a hop's child joins
  its job object before it runs its first instruction, so what it starts at
  once is in the job too, and a refusal to put it there is said once. The
  resolver is the only word on what starts: a bare name it cannot find is
  `E551`, never a search of the current directory through an empty `PATH`
  entry; with no `PATH` at all the platform's default search is used, and
  on Windows the system directories are searched after `PATH`, as
  `CreateProcess` does, and a Store app's execution alias resolves.
- **Run records.** On Windows a run's start time comes from the kernel, not
  from a PowerShell started for every run (a dry run asks for none); a
  record from another boot with this process's pid no longer passes for
  alive; `run.json` and the tin's objects are synced to disk when written;
  the record carries the pack and how many files the rollback moved into
  the tin, and `cig runs <id>` lists them, with `(put back)` beside the
  entries a rollback restored.

- **A copy onto its own name in another case emptied the file** (#66). The
  case-only exception 1.3.1 added to the same-file guard let `fs.cp` through
  as well, and a copy truncates its destination before it reads its source.
  The exception now belongs to `fs.mv` alone, and it asks the directory, not
  the link count: the destination's spelling must be absent from the
  listing, which is what a case-insensitive filesystem looks like. So a
  directory renames across case too (#67), and a symlink is no longer
  renamed over its own target (#68).
- **Rolling back a case-only move deleted the file.** The move back cleared
  the source name first, which on APFS and NTFS is the file itself, and the
  destination's snapshot, the same file again, was restored after it. A move
  between two spellings of one name is journaled as the move alone now, and
  moved back without clearing anything.
- **A hop's own `PATH` chooses its program again** (#69). Since 1.3.0 the
  resolver searched the parent's `PATH`, so `{env: {PATH: ...}}` started the
  parent's binary with no error. It searches the `PATH` the child will see.
  On Unix it also asks what `execvp` asks, whether this process may run the
  file, instead of reading the mode bits; reads a relative `PATH` entry from
  the hop's working directory; and keeps the name the script typed as
  `argv[0]`. On Windows a path program without an extension is completed
  through `PATHEXT` before the bare file is tried, so `./gradlew` starts
  `gradlew.bat`, and the C runtime's quoting is used only for a native
  `.exe` or `.com`.
- **A partial rollback is finished by a plain `cig unburn`** (#70). The
  entries a rollback put back are stamped in the journal, so the next
  `cig unburn <id>` retries what failed and leaves the rest as it is; the
  record lists the failures after a manual unburn too. `--force` restores
  everything again, as before. A compensation that fails leaves the ops it
  covered standing, and the report says `not undone` of them instead of
  `compensated`.
- **E758 reads every component** of a path, in either separator, and knows
  `com0`, `lpt0`, the superscript digits, `conin$` and `conout$`; the kernel
  refuses a computed device name with or without a `pack`.
- **Windows restores.** A read-only file is swapped without the wait meant
  for a held-open one, and a failed swap leaves no staging file behind; a
  relative symlink target is read from the link's own directory when a
  restore decides between a file link and a directory link.
- **macOS durability.** `sync_all` already is `F_FULLFSYNC` on Apple
  targets, so 1.3.0 added nothing there; the fallback a filesystem that
  refuses it needs, `fsync`, is now a real one.
- `ps` is asked with `LC_ALL=C` and `TZ=UTC` on macOS and the BSDs, so a
  run's start time reads the same from any shell; `clean_env` keeps the
  Windows variables programs expect (`TEMP`, `LOCALAPPDATA`, `PROGRAMFILES`
  and the rest); `install.ps1` no longer ends the caller's shell on an error
  under `irm | iex`, takes a path with brackets literally, and writes the
  user `PATH` through the registry so `%VAR%` entries stay unexpanded.

## 1.3.1 — 2026-10-27

- **`cig unburn` after a run on Windows.** A directory's fingerprint read
  sizes and times from the directory listing, which NTFS refreshes only when
  a file's last handle closes, so a run that had just finished could read as
  changed a moment later and `unburn` refused it (`E704`, the same entry
  count on both sides). The fingerprint and the tin's scan read each file
  itself now. Found by cig-orchestrator's suite. (#64)
- **An apostrophe reaches an MSYS program.** Git's `bash`, `sh` and `perl`
  parse their own command line, where a bare `it's` loses its quote and `*`
  globs; an argument with such a character is quoted the way the C runtime
  reads it, which both parsers agree on. (#63)
- **A case-only rename is a rename.** `fs.mv("readme.md", "README.md")` on a
  case-insensitive filesystem (APFS, NTFS) was refused as a move onto itself;
  it renames now, and only two names for one file that differ in more than
  case (hard links) stay refused. (#62)

## 1.3.0 — 2026-10-27

- **One resolver finds a hop's program.** `proc.run`, `proc.which`, every
  `proc.pipe` stage and the shell behind `proc.shell` agree on what starts:
  a name with a separator is a path against the hop's `cwd` on every
  platform; a bare name is searched on `PATH` with `PATHEXT` on Windows,
  so `npm` is `npm.cmd`; System32's `bash.exe` (the WSL launcher) and
  `wsl.exe` are never chosen for `bash` or `sh`. (#42)
- **`proc.shell` on Windows** runs under `sh -c` when an `sh` is on PATH
  (Git for Windows, MSYS2) and otherwise under `cmd /d /s /c` with the line
  reaching cmd as written; the plan line names the shell. (#43)
- **Windows: a file another program holds open comes back.** A restore
  that cannot rename over a held-open file (an editor, a scanner, an
  indexer without delete sharing) retries for a moment, then writes the
  content back in place and says so; when even that is refused the run
  stays `rollback_incomplete` with the program's hold named and the
  `cig unburn` that finishes the job. (#44)
- **Windows: a timeout ends the whole process tree.** Every hop's child is
  put in a Job Object (`KILL_ON_JOB_CLOSE`); a timeout terminates the job,
  grandchildren holding the pipes included, and cig dying takes the tree
  with it, the way the process group does on Unix. (#45)
- **Windows device names are refused up front.** `con`, `prn`, `aux`,
  `nul`, `com1` to `com9` and `lpt1` to `lpt9`, with any extension, are
  devices on Windows: a literal one is `E758` in `cig check`, an error on
  Windows and a warning elsewhere (the script will not run there), and the
  kernel refuses a computed one on Windows before it is journaled, where
  1.2 failed mid-burn with `E702 Incorrect function`. (#48)
- **The tin keeps what a file is.** A symbolic link a hop retargets,
  replaces or deletes comes back as the link it was, with or without
  `touches` (`(link kept)` in the record); a `chmod` by a hop is seen,
  although it changes neither size nor time, and put back where the file
  was in the tin; a modified file comes back with the modification time it
  had, from the tin and from a native snapshot alike. A read-only file
  under `touches` is stashed like any other. (#41)
- **A dead run never reads as `running` on a reused pid.** The record keeps
  when its process started (`/proc` ticks on Linux, `ps -o lstart` on macOS
  and the BSDs, the creation time on Windows), and a run counts as alive
  only when a cig with that start time holds the pid. (#46)
- **Durability says what each platform does.** Every flush on macOS is
  `F_FULLFSYNC`, which reaches the drive where `fsync` stops at its cache;
  on Windows the directory that gained a snapshot or an entry is flushed
  through a backup-semantics handle where NTFS allows it. BURN.md has the
  table. (#47)
- **Linux assets are static musl builds** (`x86_64`, `aarch64`, and `armv7`
  as `linux-arm`): they run on any glibc, on Alpine, and on Android under
  Termux, where 1.2.1's glibc 2.39 binary would not start. Windows on ARM
  (`windows-aarch64.exe`) and FreeBSD x86_64 (built in a VM) join the
  matrix, and `workflow_dispatch` builds the whole matrix without
  publishing. (#40, #49)
- **`install.ps1`.** A PowerShell installer for Windows: picks the x86_64
  or ARM64 asset, checks the checksum and the attestation, installs to
  `%LOCALAPPDATA%\Programs\cig` and offers to add it to the user PATH;
  `irm … | iex` reads its options from `CIG_*` variables. (#49)
- **Attestations checked at install.** `install.sh` and `cig update` run
  `gh attestation verify` when `gh` is installed and say `attested`; the
  checksum stays the floor. `install.sh` maps `armv7l` and `armv6l`, knows
  FreeBSD, and says so on Android. (#49)
- **`cig update` with a `gh` that is not logged in** falls back to the
  public API through curl instead of failing. (#35)
- **The suite runs where cig runs.** CI tests Ubuntu and macOS on both
  architectures, Windows x86_64 and ARM, Alpine as root (musl, BusyBox),
  and FreeBSD in a VM; the Ubuntu job also runs the suite in release mode,
  where inlining changes the frames a crash report keeps. The portability
  probe lives in `tests/portability/` and runs on every OS in the matrix,
  a `FAIL` line failing the job. The two tests that lock a directory with
  `chmod` step aside as root and say so; `ps -ww` reaches past the BSDs'
  line cut; the symlink rollback has a Windows port. (#50)
- **Docs.** LANGUAGE.md describes the pack as it is (scope, since 1.1),
  not as planned; LEXICON.md says what of the foreign script has landed and
  what the burn protocol still adds; one word per idea for the tin's next
  steps, in LEXICON.md and the roadmap: the box is the overlay, the parcel
  is the export, sealed is encrypted at rest, pressed is write-once. (#51,
  #52)

## 1.2.1 — 2026-10-27

- **A read-only file is snapshotted like any other.** 1.2.0 opened every
  snapshot for writing to flush it (the Windows fix of #26), which Unix
  refuses when the copy carries a read-only mode: a tree holding a tin
  object or a `.git/objects` entry could not be removed or replaced under a
  burn (`E702`, permission denied). The flush now opens read-only on Unix
  and lifts the read-only attribute around the flush on Windows.
- **Release assets for Intel macOS build again.** The release job asked for
  the retired `macos-13` runner and waited on it forever; 1.2.0 shipped
  without `cig-v1.2.0-macos-x86_64`. It asks for `macos-15-intel` now.
- **The read-only rediagnosis test fingerprints files, not directories.**
  On Windows a directory's modified time moved under it between the run
  and the check, twice in four runs; the property it guards (`cig doctor
  <run>` writes nothing) is about files, and a difference now names them.

## 1.2.0 — 2026-10-27

The pin, the humidor, the tin, and crash reports that put the run back.

- **Crash reports name the function.** A release build keeps its symbol
  names (`strip = "debuginfo"`, about 0.7 MB more binary), so the backtrace
  in a report reads `cigscript::interp::…` instead of `<unknown>`. The
  report keeps every frame of cig's own code, never the panic hook itself,
  the one `std` or `core` frame our code called into when the panic came
  from a library, and a closure inlined into a `std` frame under LTO.
  `CIG_INTERNAL_PANIC=eval` and `CIG_INTERNAL_PANIC=after-burn` join `=1`
  as the reporter's test hooks. (#23)
- **A crash mid-run offers to put its burns back.** The crash prompt reads
  the run's journal from disk and, before asking about the report, asks
  whether to roll the reversible burns back (`crash_rollback = ask`, the
  default); `always` does it unasked, `never` and a run without a terminal
  print the `cig unburn <run>` line instead. The report records
  `rolled_back`. (#24)
- **The pin.** A script says which cig it was written for: `# cig 1.1`
  under the shebang (any 1.1.x; `1.1.2` exactly; `>=1.1.2`), or a
  `.cig-version` file for a whole project. When the installed cig does not
  satisfy it, `run`, `--dry-run`, `check`, `light` and `chains` are handed
  to that version from the humidor and the report says so; with no such
  version at hand the run is refused (`E807`), never lit on the wrong cig.
  `cig pin <file>` writes the pin. (#22)
- **The humidor.** `~/.cigscript/humidor/<version>/cig`: other releases,
  fetched on demand and verified against their checksum (and the release
  attestation where `gh` is installed) before they are ever run. `cig
  versions` lists the shelf, `add`, `remove` and `prune` tend it; a
  `YANKED` file in the repository keeps a range pin off a bad version. (#22)
- **The since table and the trial.** Every builtin and clause knows the
  release it arrived in: `cig check` warns (`E309`) when a pinned script
  uses something newer than its pin allows, and `cig pin` infers the
  lowest version a script needs. Before a run on a cig a script has not
  been proved on, its plan here and its plan on its last good version (the
  newest run that went through, from the run history) are compared op by
  op: identical plans run here, different plans run on the proved version
  with the difference printed, and a run that fails here after burns that
  all came back is tried once more on the last good version. Never after an
  irreversible op. `trial = auto | always | never`, `--trial`,
  `--no-trial`. (#22)
- **`cig.version()`, `cig.os()`, `cig.arch()`, `cig.run_id()`.** A script
  can read the cig it runs under and the id of its own run, so a tool
  written in CigScript (cig-orchestrator) checks the cig it was handed and
  prints its own `cig unburn` line. (#22, cig-orchestrator FINDINGS)
- **Records know their format.** `run.json`, `after.json` and the first
  line of `journal.jsonl` carry `format: 1`. A newer cig reads older
  formats; an older cig refuses a newer one by its number (`E808`) and
  `cig unburn` hands the run to the cig that wrote it. (#22)
- **The tin.** A hop can say which files it may change (`proc.run(cmd,
  args, {touches: ["src/**"]})`); their before-bytes are kept outside the
  project, one object per distinct content, before the child runs, and a
  file it modified or deleted comes back on rollback, byte for byte and
  hash-verified, where it was listed as irreversible before. A same-size
  edit that puts the mtime back is seen by the hash. `cig tin [du|verify|gc]`.
  A `.cigignore` at a pack root, and the usual build and dependency trees,
  are never watched. `touches` without a pack is `E757`. (cig-orchestrator #18)

## 1.1.2 — 2026-10-24

Found by building [cig-orchestrator](https://github.com/otmof-ops/cig-orchestrator),
a tool written in CigScript that runs a project's existing scripts.

- Hop watching journals the directories a script creates inside the pack,
  not only the files, so a rollback or `cig unburn` removes `build/out/`
  instead of leaving it empty.
- `fs.glob("*")` no longer lists an empty string (the directory itself);
  `fs.glob("./scripts/*")` matches what `fs.glob("scripts/*")` does and keeps
  the `./` on its results; a pattern without `**` walks no deeper than its
  own components, so `*` at the top of a project no longer reads all of
  `node_modules`.
- A `;` may follow a block statement: `if c { }; x`, `burn { }; x`.

## 1.1.1 — 2026-09-28

The hardening round: 1.1.0 attacked from every side, the failures that
reproduced fixed, each with a row on the wall and a test named after it.

- Kernel: `after.json` fingerprints directories (names, kinds, sizes,
  mtimes), so a file added to a directory the run created is a change and
  `unburn` refuses with `E704` instead of removing the directory with the
  file in it. The pack check resolves symlinks before judging a path; a link
  inside the pack that points outside is outside, and the refusal says where
  the write would have landed. A journal whose last line a crash cut short
  loads up to the cut; `unburn` says so and restores what came before.
- Hops: stdin is written from its own thread beside the two readers (a child
  echoing 8 MB back deadlocked). SIGINT, SIGTERM and SIGHUP are forwarded
  to every live child process group before cig ends (a Ctrl-C left the child
  running, orphaned in its own group). `proc.pipe` no longer fails a
  pipeline because an upstream stage died of SIGPIPE when its consumer closed
  early (`yes | head -1`); the stage reports `closed_early` and `signal`. A
  missing or non-directory `cwd` is `E550`, naming the directory, not `E551`
  blaming the command.
- Language: brackets, parentheses and blocks nested deeper than 5,000 levels
  are `E208`, a diagnostic, instead of a stack overflow; a value that
  contains itself prints `…` at depth 512 instead of overflowing; a UTF-8
  byte-order mark at the top of a file is skipped instead of `E103`.
- Diagnostics: a source line longer than 120 characters is shown as a window
  around the caret with `…` at the cuts.
- Kernel, second round: `fs.cp` and `fs.mv` refuse a destination that is the
  source itself (the copy truncated the file to nothing and reported success)
  or lies inside it. A move the operating system refused (into its own
  subdirectory, onto a non-empty directory, a trailing slash) is marked
  undone in the journal instead of being "moved back" by rollback over the
  untouched source. A cross-device move is rolled back by copying, and the
  only copy is kept when a move back fails. A file with other hard links is
  restored in place, so the other names see the old bytes too. Setuid,
  setgid and sticky bits survive the snapshot and the restore, and a restored
  directory tree keeps the mode of its root. A named pipe, socket or device
  is refused (`E702`) instead of hanging the snapshot. What the automatic
  rollback could not restore is recorded on the run (`rollback_incomplete`,
  listed by `cig runs <id>`), and `cig unburn` checks it can record itself
  before restoring anything. A damaged or renamed run record no longer
  hides the run: it is listed as `damaged`, its journal is found where it
  is, and it can be unburned. A dead run's pid reused by another program no
  longer reads as `running`.
- Compensations: the `compensated` stamp goes on a block's ops only when
  the block completes and its compensation is recorded; a block that fails
  half-way, or a process that dies inside one, leaves them honestly
  irreversible. A block left by `snuff`, `break`, `continue` or `exit()`
  completed its effects and records its compensation (1.1.0 dropped it
  silently, so the undo never ran). A deferred `cig unburn` gives the
  compensation the run's `args`.
- Pack and ghost: a link inside the pack is followed when a hop's writes
  are watched; the dry run resolves a symlinked directory to the same ghost
  entry as its target; every directory a nested write creates is listed;
  an empty pack root is refused (`E755`) instead of silently meaning the
  whole working directory.
- Language: `-9223372036854775808 / -1` is `E503`, not a crash; `int()`
  refuses a float or numeric string that does not fit instead of
  saturating; floats past sixteen digits print in exponent form so they
  still read as floats; a value that contains itself is refused by
  `json.stringify` and compared without recursing forever; a redeclaration
  reached at run time is a check-family error with the checker's exit code,
  and `args` is a prelude name a script may shadow; a string that runs past
  the end of its line is `E101`; more than 10,000 chained operators are
  `E208`; an invalid `CIG_MAX_STEPS` is `E800`.
- Hops: `ok` and `check` resolve the same whatever their order (the
  contract could vanish depending on key order); an empty `ok` list and an
  empty `sep` are refused; a directory given as the command is named as
  one; `proc.kv` reads `export KEY=value`; `proc.which` resolves a name
  with a slash the way `proc.run` does.
- Chains: `light(steps, {names: [...]})` names the steps of a chain built at
  run time; the narration, the report and the error used to say `step 3`.
- `fs.append_text` creates the parent directory, as `fs.write_text` always
  did; a log under a directory that did not exist yet failed for real after
  a dry run that saw no problem.
- Plan: `env.set` and `env.unset` are labelled irreversible, as `BURN.md` and
  `STDLIB.md` have always said; 1.1.0 labelled them reversible although the
  kernel restores nothing for them.
- CLI: a malformed invocation exits 3 like every other usage problem; a
  script larger than 64 MiB, or a device like `/dev/zero`, is refused
  (`E801`) instead of read forever, and `E801` now carries the path for
  doctor; `cig crash send` honours `crash_reports = never`; the update
  downgrade refusal is a coded `E806`.

- Docs: the narrative pages (`DESIGN.md`, `SMOKE.md`, `LANGUAGE.md`,
  `CHAINS.md`, `BURN.md`, `DOCTOR.md`, `SCIENCE.md`, the README's burn-model
  table and the `cig language` legend) say what 1.1.0 does; the last claims
  from before the Hammer update (a dry run that cannot read its writes, a
  self-lighting chain that fails at depth 32, "no `finally` yet", packs as
  something that was cut) are gone.

## 1.1.0 — 2026-09-27

The "Hammer" update: built from an external adversarial read of 1.0.0 and
delivered from its own package (`HAMMER.md`), one pull request per seam
(#1 to #9); the package's manifest carries the state of each deliverable.
Sixteen of its eighteen deliverables ship here; foreign scripts and the
browser playground are the next update, as the package itself frames them.

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
