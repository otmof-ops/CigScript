# Contributing to CigScript

Bug reports, fixes, examples and documentation are welcome. This page is
short because the tooling does most of the checking.

## Before you start

- **Bugs:** open an issue with the script (or a minimal version of it), the
  command you ran, and the full error, including its code (`E502` and so on).
  If `cig` crashed, `cig crash show <id>` has everything we need; send it with
  `cig crash send <id>` or paste it into the issue.
- **Features:** open an issue first and say what problem it solves. The
  language is kept deliberately small; `docs/DESIGN.md` explains why and lists
  what is already planned.
- **Security problems:** do not open a public issue. See `SECURITY.md`.

## The contributor agreement

CigScript uses a Contributor License Agreement, not a DCO. Before a pull
request from you can be merged, you sign the agreement once:

- individuals: [`.github/CLA/ICLA.md`](.github/CLA/ICLA.md)
- companies whose employees contribute: [`.github/CLA/CCLA.md`](.github/CLA/CCLA.md)

You keep your copyright. The agreement gives the project a licence to use,
relicense and distribute your contribution, a patent licence for what you
contribute, and your confirmation that the work is yours to give. It exists so
the project can, for example, offer a commercially licensed edition or move to
a newer licence later without tracking down every past contributor. Signing
is a comment on your first pull request:

> I have read the CLA Document and I hereby sign the CLA

A bot records it; you will not be asked again.

## The workflow

1. Fork, branch from `main`, make the change.
2. Every source file carries SPDX headers; `REUSE.toml` maps the rest.
   Code is `Apache-2.0`, documentation is `CC-BY-4.0`, examples are `CC0-1.0`.
   New files get the header of their kind (copy one from a neighbour).
3. Run the gate locally, exactly as CI does:

   ```
   cargo fmt --all --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```

   Behaviour changes need a test. Kernel changes need a test that inspects the
   file system after a rollback, not just an exit code. New library functions
   need an entry in `docs/STDLIB.md`, which is generated:
   `python3 scripts/gen-stdlib-doc.py target/debug/cig`. New error codes go in
   `src/errors.rs` and `docs/ERRORS.md` is regenerated the same way.
4. Open the pull request against `main`. Describe what changed and why; link
   the issue. One logical change per pull request.

## Style

Rust: rustfmt and clippy decide. Prose: plain words, short sentences, no
hype. Error messages say what happened, why, and what to do next, in that
order, and get a stable code.

## AI assistance

CigScript itself was built with AI assistance under human direction, and
that is fine here too. Say so in the pull request when a substantial part of
a contribution was generated, and make sure you have actually read and
understood what you are submitting: the CLA's statement that the work is
yours to contribute applies to it.
