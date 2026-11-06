# Compiled programs

<!-- SPDX-License-Identifier: CC-BY-4.0 -->

A **program** is a CigScript script appended to the `cig` of the same version. It is one executable that runs its script and takes no command. This is stage A of cigcompile: nothing is translated, and the script runs through the same run as `cig run`, with the same kernel, journal, plan and rollback.

## How one is made

`cigcompile build tools/tidy.cig` checks the script and writes the program. Its errors are `E1000` to `E1007` (`cig explain E1000`). Anyone writing a tool that makes programs needs the format. The end of a program's file is:

```text
... cig ...                                the runtime, as released
payload                                    len bytes of JSON
"CIGPROG1"  len (u64, LE)  sha256(payload) the trailer, 48 bytes
```

- **Payload.** One JSON record, fields in a fixed order: `format` (1), `name`, `file`, `source`, `sha256` (of `source`, hex), `cig` (the version it runs on) and `compiler`. The same inputs give the same bytes.
- **The runtime's version.** Every cig from 1.15.0 carries `\0cig-runtime-version=<version>\0` in its bytes, so a tool can tell which cig a binary is without running it (one built for another machine cannot run here).
- **Limits.** A payload is at most 16 MiB. `name` is a letter or digit, then letters, digits, `.`, `_` or `-`, 64 at most. `file`, `compiler` and `cig` are printable text with no control characters.

## Running one

```text
tidy [run flags] [--] [arguments...]
```

Leading flags are the run's. The first word that is not a flag, or everything after `--`, is the program's own. An unknown flag before that is refused (`E1052`), so a mistyped `--dry-run` never becomes a real run.

| flag | does |
|---|---|
| `--dry-run` | plans every burn and prints the plan |
| `--no-rollback` | leaves the burns in place if the program fails |
| `--max-steps N` | the step budget; `0` turns it off |
| `--json`, `--plain`, `--no-color` | output |
| `--no-doctor` | no automatic diagnosis on errors |
| `--light <chain>` | runs the program, then lights a chain it declares |
| `--chains` | lists the chains the program declares |
| `--unburn <run>` | puts back a past run of this program |
| `--explain <code>` | what an error code means |
| `--cig-program` | what the program is and what it was compiled from |
| `--help`, `-h` | the program's usage |

`--max-steps`, `--unburn`, `--explain` and `--light` also take `--flag=value`. Exit codes are those of `cig run`: `0` ok, `1` script error, `2` syntax error, `3` usage.

## Same as `cig run`, and what differs

The same: the kernel, burns, rollback, `--dry-run`, error positions (they point at `file` as compiled), and the burn protocol.

- **No check at start.** The script was checked when it was compiled, and the warnings were shown then.
- **No pin, no trial.** The program is its cig, so there is nothing to dispatch to or compare against.
- **Named runs.** `cig runs` lists a run as `<name> (program)` and shows the file it was compiled from, its sha256, its compiler and the binary. The record also carries `no_rollback`.
- **`--unburn` is for its own runs.** A run made by another script or program is refused (`E1053`); `cig unburn <run>` puts back any run. A journal written by a newer cig is not handed on: the program refuses and names `cig unburn`.
- **The undo line.** Run inside another cig's hop, a program reports `[<binary>, "--unburn", <run>]`, where `cig run` reports `[cig, "unburn", <run>]`.

## Integrity, not authenticity

The sha256s catch damage and truncation. They do not catch an edit, because anyone can recompute them, and they do not cover the runtime. Programs are unsigned until cig-compile#10. A program whose bytes, record, format or script do not match refuses to run (`E1050`). A program compiled for another cig refuses too (`E1051`).

## A program is never a general `cig`

A program's binary is `cig` for two calls only: the ashtray's background send (`crash flush --quiet`) and the box's network forwarder (`internal-forward <absolute socket> <port>`). Cig makes them with `CIG_INTERNAL_SELF` set, and the program answers that exact command line and nothing else. The marker is removed from the environment on start, and counts for nothing in a setuid, setgid or capability-raised process. A program never runs `cig update` (it never replaces itself) and never runs a repro.

## Platforms

- **Linux:** the program reads its own image through `/proc/self/exe`, so replacing or removing its path while it runs changes nothing.
- **Windows:** it reads the file `current_exe` names.
- **macOS:** running is tested in CI; signing is cig-compile#10.

## Codes

| code | means |
|---|---|
| `E1050` | a damaged program |
| `E1051` | a program for another cig |
| `E1052` | not one of the program's run flags, a missing value, or an argument that is not UTF-8 |
| `E1053` | `--unburn` of a run this program did not make |

`E1000` to `E1007` are cigcompile's. The families `E1000` to `E1799` are listed in `errors/registry.toml`.
