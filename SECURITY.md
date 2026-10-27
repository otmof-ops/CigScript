# Security policy

## Supported versions

Only the newest published release receives fixes. Please reproduce against it
before reporting: `cig update --check` tells you whether you are on it.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting on this repository
(**Security → Report a vulnerability**). Do not open a public issue for a
security problem, and do not send it through `cig crash send`, which files a
public issue. Include the version (`cig --version`), the platform, and a
script or command that demonstrates the problem. You will get an
acknowledgement within a few days and a fix or a mitigation before any public
disclosure.

## What the runtime promises and what it does not

CigScript is a local automation tool. Its kernel makes side effects explicit,
journaled and reversible where it performed them itself. It is **not a
sandbox**: a script can read whatever the user can read, and a `burn` block
can run any program. Treat a `.cig` file from someone else the way you would
treat a shell script from them. `cig run --dry-run` and `cig check` are the
tools for reading before lighting. Full detail in `docs/BURN.md`.

`proc.run` never goes through a shell; arguments are passed as given, so there
is nothing to inject into. The same holds for every place `cig` itself runs
another program (`gh`, `curl`, package managers offered by `cig doctor --fix`).

## What `cig update` trusts

- TLS to `github.com` and `api.github.com` (curl is invoked with `--proto
  =https --proto-redir =https --tlsv1.2`; `gh` uses its own stack).
- The SHA-256 sidecar published with each release. It proves the download is
  intact and identical to what CI published; it does not prove who built it.
  If the repository's GitHub account were compromised, a signed release would
  be the defence, and signing (minisign or Sigstore) is on the roadmap. Until
  then, treat `cig update` as "as trustworthy as the GitHub account".
- Nothing else: no downgrade without `--allow-downgrade`, no install into a
  directory another user can write to, and the running binary is replaced with
  a rename, never written in place. `cig` makes no network request unless you
  run `cig update` or `cig crash send`.

## What the humidor trusts

`~/.cigscript/humidor/` holds other versions of cig, for scripts pinned to
them (`# cig 1.1`) and for journals they wrote. A version arrives only through
the path `cig update` takes: the release's asset for this platform and its
`.sha256`, verified before the binary is kept, and never run from a failed
check. Where `gh` is installed the binary is also checked against the
repository's build attestation (`gh attestation verify`), which proves it
came from this repository's release workflow; `cig versions` shows each
version as `attested`, `verified` (checksum only) or `unverified` (put there
by hand). A dispatched cig runs with `CIG_DISPATCHED=1` and never dispatches
again. Releases are never deleted and tags never moved, so a pinned script
can always be lit by the cig it names; a version with a serious bug is listed
in `YANKED` at the repository root with its reason, and a range pin never
picks one.

## What the crash reporter sends

Nothing, unless you say yes. A panic writes a report under
`~/.cigscript/crashes/` and, in a terminal, asks once whether to file it.
`cig crash show <id>` prints exactly what would be sent. A report contains:

| field | contents | redaction |
|---|---|---|
| version, os, arch | e.g. `2.1.0`, `linux`, `x86_64` | none |
| command | the arguments `cig` was run with | values that look like tokens or secrets, or follow a flag named token/key/secret/password, become `<redacted>`; paths are reduced to their file name; URLs to `<url>` |
| cwd | the working directory | your home directory is written as `~` |
| timestamp | when it happened, UTC | none |
| panic message and location | the failure and the line in `cig`'s own source | home paths and token-shaped words removed; 500 characters at most |
| backtrace | frames from `cig`'s own code only | paths made relative to the crate |
| active run id | the run in progress, if any | id only; the run's journal and snapshots are never attached |
| rolled back | after a crash mid-run: whether its reversible burns were put back before the report was sent (`null` when no run was in progress) | none |

Never included: script source, environment variable values, file contents,
tokens. Filing goes through `gh` (your own credentials) or a
`CIG_GITHUB_TOKEN` read from the environment at the moment of sending and
passed to curl in a config file, never on the command line and never stored.
Set `cig config crash_reports never` to keep reports local, or `always` to
file them without the prompt (the report is still printed before it goes).
Filed reports are public and permanent; duplicates of a known crash are added
as a comment to the existing issue, at most once a day.

A crash in the middle of a run leaves its burns in place; the journal is
write-ahead, so it survives, and `cig unburn <run>` puts the reversible ones
back. Before the report prompt, cig offers to do that (`cig config
crash_rollback ask`, the default), does it without asking (`always`), or only
names the command (`never`, and whenever there is no terminal). The rollback
reads the journal on disk, never anything from the thread that crashed.

## What `install.sh` trusts

Its own text. Download it, read it, then run it; the README shows the pinned
form. It downloads a tagged release and its checksum over HTTPS, verifies
before installing, installs into your home directory, never uses `sudo`, and
asks before touching your shell configuration or running a package manager.
