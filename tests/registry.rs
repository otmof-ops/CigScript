// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! The registry and the source agree, in both directions, and every code
//! has a complete page. This is the "CI fails on a code without an explain
//! page" test from the update package.

use cigscript::diagnostics::Kind;
use cigscript::errors::{all, default_code, lookup, lookup_no, lookup_probe, registry};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

const ARISES: &[&str] = &[
    "parse", "check", "dry-run", "run", "light", "unburn", "runs", "doctor", "update", "crash",
    "any",
];
const TAGS: &[&str] = &[
    "beginner-likely",
    "ci-likely",
    "resource",
    "kernel",
    "fixable-by-doctor",
    "needs-burn",
    "irreversible-involved",
    "windows",
    "macos",
    "planned",
];

/// Every `Eddd` token in the Rust sources, comments stripped.
fn emitted_codes() -> BTreeSet<String> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = BTreeSet::new();
    for entry in walkdir::WalkDir::new(&src) {
        let entry = entry.unwrap();
        if entry.path().extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = fs::read_to_string(entry.path()).unwrap();
        for line in text.lines() {
            let code_part = match line.find("//") {
                Some(i) => &line[..i],
                None => line,
            };
            let bytes = code_part.as_bytes();
            let mut i = 0;
            while i + 4 <= bytes.len() {
                if bytes[i] == b'E'
                    && bytes[i + 1..i + 4].iter().all(u8::is_ascii_digit)
                    && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
                    && (i + 4 == bytes.len() || !bytes[i + 4].is_ascii_alphanumeric())
                {
                    out.insert(code_part[i..i + 4].to_string());
                }
                i += 1;
            }
        }
    }
    for kind in [
        Kind::Lex,
        Kind::Syntax,
        Kind::Check,
        Kind::Type,
        Kind::Runtime,
        Kind::Cough,
        Kind::Burn,
        Kind::Usage,
        Kind::Internal,
    ] {
        out.insert(default_code(kind).to_string());
    }
    out
}

#[test]
fn every_emitted_code_is_registered_and_every_registered_code_is_emitted() {
    let emitted = emitted_codes();
    let registered: BTreeSet<String> = all().iter().map(|c| c.code.clone()).collect();
    let unregistered: Vec<&String> = emitted.difference(&registered).collect();
    assert!(
        unregistered.is_empty(),
        "codes used in src/ but missing from errors/registry.toml: {unregistered:?}"
    );
    let dead: Vec<&String> = registered
        .difference(&emitted)
        .filter(|c| {
            let e = lookup(c).unwrap();
            !e.has_tag("planned") && !e.retired
        })
        .collect();
    assert!(
        dead.is_empty(),
        "codes in errors/registry.toml that nothing in src/ emits (tag them `planned` or retire them): {dead:?}"
    );
}

#[test]
fn every_code_has_a_complete_page() {
    let reg = registry();
    let arises: BTreeSet<&str> = ARISES.iter().copied().collect();
    let tags: BTreeSet<&str> = TAGS.iter().copied().collect();
    for c in all() {
        let id = &c.code;
        assert!(!c.title.trim().is_empty(), "{id}: title");
        assert!(c.meaning.trim().len() > 20, "{id}: meaning too short");
        assert!(
            c.fix.trim().len() > 10,
            "{id}: fix (what to type next) too short"
        );
        assert!(
            !c.fix.trim_start().starts_with(char::is_uppercase),
            "{id}: fix reads as a sentence about the problem; write what to type next, lowercase"
        );
        assert!(
            lookup_no(&c.no).is_some(),
            "{id}: unknown kind of no `{}`",
            c.no
        );
        assert!(
            c.since.split('.').count() == 3 && c.since.split('.').all(|p| p.parse::<u32>().is_ok()),
            "{id}: since `{}` is not a version",
            c.since
        );
        for a in &c.arises {
            assert!(arises.contains(a.as_str()), "{id}: unknown arises `{a}`");
        }
        for t in &c.tags {
            assert!(tags.contains(t.as_str()), "{id}: unknown tag `{t}`");
        }
        for r in &c.related {
            assert!(
                lookup(r).is_some(),
                "{id}: related code {r} is not registered"
            );
            assert_ne!(r, id, "{id}: related to itself");
        }
        assert!(
            !c.causes.is_empty(),
            "{id}: no cause; doctor needs at least one, with probe = \"none\" if nothing can confirm it"
        );
        for (i, cause) in c.causes.iter().enumerate() {
            assert!(cause.why.trim().len() > 8, "{id} cause {}: why", i + 1);
            assert!(
                cause.remedy.trim().len() > 8,
                "{id} cause {}: remedy",
                i + 1
            );
            assert!(
                lookup_probe(&cause.probe).is_some(),
                "{id} cause {}: unknown probe `{}`",
                i + 1,
                cause.probe
            );
        }
        if c.has_tag("fixable-by-doctor") {
            assert!(
                c.causes.iter().any(|k| k.fixable),
                "{id}: tagged fixable-by-doctor but no cause is fixable"
            );
        }
    }
    // Ranges: ordered, non-overlapping, one kind each.
    let mut last_to = String::new();
    for r in &reg.ranges {
        assert!(r.from < r.to, "range {}-{}", r.from, r.to);
        assert!(
            r.from > last_to,
            "ranges overlap or are out of order at {}",
            r.from
        );
        last_to = r.to.clone();
        assert!(!r.meaning.is_empty());
    }
    // Probes: unique names, `none` present.
    let names: BTreeSet<&str> = reg.probes.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), reg.probes.len(), "duplicate probe names");
    assert!(names.contains("none"));
    // The lexicon's four kinds of no, plus none and depends.
    let nos: BTreeSet<&str> = reg.nos.iter().map(|n| n.id.as_str()).collect();
    for want in [
        "none",
        "cant-see-any",
        "never-heard-of-you",
        "these-are-mine",
        "dont-have-any",
        "depends",
    ] {
        assert!(nos.contains(want), "kind of no `{want}` missing");
    }
}

#[test]
fn generated_docs_are_current() {
    // docs/ERRORS.md and docs/diagnostic.schema.json come from the registry;
    // a stale copy is a drift the package forbids.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc = fs::read_to_string(root.join("docs/ERRORS.md")).expect("docs/ERRORS.md");
    for c in all() {
        assert!(
            doc.contains(&format!("### {} ", c.code)),
            "docs/ERRORS.md has no section for {}; run scripts/gen-errors-doc.py",
            c.code
        );
    }
    let schema: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("docs/diagnostic.schema.json")).expect("schema file"),
    )
    .expect("schema json");
    let listed: BTreeSet<&str> = schema["properties"]["code"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let registered: BTreeSet<&str> = all().iter().map(|c| c.code.as_str()).collect();
    assert_eq!(
        listed, registered,
        "docs/diagnostic.schema.json is stale; run scripts/gen-errors-doc.py"
    );
}
