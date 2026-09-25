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

Never included: script source, environment variable values, file contents,
tokens. Filing goes through `gh` (your own credentials) or a
`CIG_GITHUB_TOKEN` read from the environment at the moment of sending and
passed to curl in a config file, never on the command line and never stored.
Set `cig config crash_reports never` to keep reports local, or `always` to
file them without the prompt (the report is still printed before it goes).
Filed reports are public and permanent; duplicates of a known crash are added
as a comment to the existing issue, at most once a day.

## What `install.sh` trusts

Its own text. Download it, read it, then run it; the README shows the pinned
form. It downloads a tagged release and its checksum over HTTPS, verifies
before installing, installs into your home directory, never uses `sudo`, and
asks before touching your shell configuration or running a package manager.
