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

## What the reporter sends

Nothing, unless you say yes. cig reports its own bugs, never your script's:
a panic, an internal error (`E900`), or a tripwire (`E902`: after the work
it re-checked one of its promises, a restored path against the journal, the
journal read back, a tin object, an applied box file, and found it broken).
Every report is written to `~/.cigscript/ashtray/` first and sent only
under the `reports` setting: `ask` (the default) shows the report once, in a
terminal, and remembers the answer; `always` files without asking and says
so in one line; `never` keeps reports local; under `CI` with nothing set,
`never`, so no pipeline files public issues by surprise. `cig crash show
<id>` prints exactly what would be sent. A report contains:

| field | contents | redaction |
|---|---|---|
| kind, origin | `crash`, `internal`, `tripwire` or `report`; `cig`, or the tool of the family that handed it in | none |
| version, os, arch | e.g. `2.1.0`, `linux`, `x86_64` | none |
| command | the arguments `cig` was run with | values that look like tokens or secrets, or follow a flag named token/key/secret/password, become `<redacted>`; paths are reduced to their file name; URLs to `<url>` |
| cwd | the working directory | your home directory is written as `~` |
| timestamp | when it happened, UTC | none |
| message and location | the failure and the line in `cig`'s own source, or the tripwire's name and what it found | home paths and token-shaped words removed; 500 characters at most; a path the finding names is written as its placeholder (`root0/d1/f3.txt`), never as yours |
| backtrace | frames from `cig`'s own code only (a panic) | paths made relative to the crate |
| active run id | the run in progress, if any | id only; the run's journal and snapshots are never attached |
| details | what the tripwire compared, or the `cig report` bundle | the same placeholders and redaction |
| repro | the smallest script that reproduced the finding in a sandbox | placeholder paths, generated bytes of the same size, stub hops that do what the journal saw; attached only when it reproduced; see below |
| rolled back | after a crash mid-run: whether its reversible burns were put back before the report was sent | none |

**The repro carries none of your data.** It is built from the journal: every
path becomes a stable placeholder that keeps only what a filesystem bug may
depend on (depth, extension, a leading dot, a capital letter, a character
outside ASCII), every file's bytes are generated from the recorded hash
(equal files stay equal, so a deduplication bug survives), every hop
becomes a stub that performs the file operations the journal saw, and the
command the hop ran is never copied. It is run in a sandbox first; one that
does not show the same finding is not attached, and the report says so.

Never included: script source, environment variable values, file contents,
tokens, the commands hops ran. Filing goes through `gh` (your own
credentials) or a `CIG_GITHUB_TOKEN` read from the environment at the
moment of sending and passed to curl in a config file, never on the command
line and never stored; where neither exists, a prefilled link is printed and
the report waits in the ashtray. Filed reports are public and permanent.
One issue per bug: a repeat updates a stats block in the issue (count,
versions, OSes, first and last seen, smallest repro), and comments only when
it brings a new version, a new OS or a smaller repro. When the issue is
closed and a release's notes name it, `cig update --check` tells the person
who hit it, once.

A crash in the middle of a run leaves its burns in place; the journal is
write-ahead, so it survives, and `cig unburn <run>` puts the reversible ones
back. Before the report prompt, cig offers to do that (`cig config
crash_rollback ask`, the default), does it without asking (`always`), or only
names the command (`never`, and whenever there is no terminal). The rollback
reads the journal on disk, never anything from the thread that crashed.

Another tool of the family (cig-orchestrator) hands its reports to cig with
`cig crash submit --origin <tool> --repo owner/name --file report.json`:
they join the ashtray under the same setting, redaction and routes, and go
to the repository the tool named.

## What `install.sh` trusts

Its own text. Download it, read it, then run it; the README shows the pinned
form. It downloads a tagged release and its checksum over HTTPS, verifies
before installing, installs into your home directory, never uses `sudo`, and
asks before touching your shell configuration or running a package manager.
