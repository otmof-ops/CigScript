<!-- SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript) -->
<!-- SPDX-License-Identifier: CC-BY-4.0 -->

## What broke, or what was missing

<!-- One paragraph. Link the issue. -->

## Why

<!-- The mechanism, in the manual's words. -->

## The fix

<!-- What changed. If it touches the kernel, say what the property test says now. -->

## The test named after it

<!-- `fn symlink_restore_is_a_symlink()` and where it lives. A fix for a cheat lands with its row on the wall. -->

## Checklist

- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
- [ ] New error codes are in `errors/registry.toml` with a cause and a probe; `scripts/gen-errors-doc.py` re-run
- [ ] Doctor wording changes accepted with `CIG_UPDATE_GOLDEN=1` and read
- [ ] New vernacular has a `docs/LEXICON.md` entry with the manual's name
- [ ] Nothing here blames the reader or withholds the pointer
