// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig report <run>`: one redacted file with everything a bug report needs.
//!
//! The run record, the journal (what burned, in order, what it looked like
//! before), the unlit intents, the failing diagnostic, doctor's diagnosis
//! run again now, and the install facts doctor prints. Redacted the same
//! way crash reports are: home paths become `~`, secret-shaped values are
//! removed, and file contents are never included. This is what makes the
//! "break it" invitation true for the bug that does not crash: the
//! confident wrong answer is as easy to send as a panic.

use super::{exit, Ctx};
use cigscript::burn::journal::{Before, Journal};
use cigscript::burn::runs;
use cigscript::crash::{redact_arg, redact_text, tilde};
use cigscript::diagnostics::Diagnostic;
use cigscript::{doctor, update};
use std::path::{Path, PathBuf};

pub fn reports_dir() -> PathBuf {
    runs::home_dir().join("reports")
}

pub fn report(ctx: &Ctx, id: &str, out: Option<PathBuf>) -> i32 {
    let rec = match runs::find(id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            eprintln!("{} no run matches `{id}`", ctx.red("error[E803 usage]:"));
            eprintln!("  = hint: `cig runs` lists them; give more of the id if it is ambiguous");
            eprintln!("  = explain: cig explain E803");
            return exit::USAGE;
        }
        Err(e) => {
            eprintln!("{} {e}", ctx.red("error[E803 usage]:"));
            return exit::USAGE;
        }
    };
    let bundle = bundle(&rec);
    let text = serde_json::to_string_pretty(&bundle).unwrap_or_default();
    let to_stdout = ctx.json || out.as_deref() == Some(Path::new("-"));
    if to_stdout {
        outln!("{text}");
        return exit::OK;
    }
    let path = out.unwrap_or_else(|| reports_dir().join(format!("{}.json", rec.id)));
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!(
                "{} cannot create {}: {e}",
                ctx.red("error[E802 usage]:"),
                parent.display()
            );
            return exit::USAGE;
        }
    }
    if let Err(e) = std::fs::write(&path, format!("{text}\n")) {
        eprintln!(
            "{} cannot write {}: {e}",
            ctx.red("error[E802 usage]:"),
            path.display()
        );
        return exit::USAGE;
    }
    outln!("{}", tilde(&path));
    eprintln!(
        "{} run {} bundled: {} journal entr{}, the diagnostic, doctor's diagnosis and the install facts, redacted; read it, then attach it to an issue:",
        ctx.green("report:"),
        rec.id,
        bundle["journal"].as_array().map(|a| a.len()).unwrap_or(0),
        if bundle["journal"].as_array().map(|a| a.len()).unwrap_or(0) == 1 { "y" } else { "ies" }
    );
    eprintln!(
        "  https://github.com/{}/issues/new?template=cheat.yml",
        cigscript::config::Config::load().get("issues_repo")
    );
    exit::OK
}

fn r(s: &str) -> String {
    redact_text(s)
}

fn bundle(rec: &runs::RunRecord) -> serde_json::Value {
    let journal: Vec<serde_json::Value> = match Journal::load(&rec.dir()) {
        Ok(j) => j
            .entries()
            .iter()
            .map(|e| {
                serde_json::json!({
                    "seq": e.seq,
                    "at": e.at,
                    "reversible": e.reversible,
                    "op": r(&e.op.describe()),
                    "before": e.before.iter().map(|b| serde_json::json!({
                        "path": r(&b.path.display().to_string()),
                        "state": match &b.before {
                            Before::Absent => "absent".to_string(),
                            Before::File { bytes, sha256, .. } => format!("file, {bytes} bytes, sha256 {}", &sha256[..sha256.len().min(12)]),
                            Before::Dir { .. } => "directory".to_string(),
                            Before::Symlink { target } => format!("symlink -> {}", r(&target.display().to_string())),
                            Before::Moved { to } => format!("moved to {}", r(&to.display().to_string())),
                        },
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    let intents: Vec<serde_json::Value> = std::fs::read_to_string(rec.dir().join("intents.jsonl"))
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .map(|v| serde_json::from_str(&r(&v.to_string())).unwrap_or(v))
                .collect()
        })
        .unwrap_or_default();
    let mut diagnostic = rec.diagnostic.clone();
    if let Some(v) = diagnostic.as_mut() {
        for key in ["message", "hint", "subject", "file"] {
            if let Some(s) = v.get(key).and_then(|x| x.as_str()).map(str::to_string) {
                v[key] = serde_json::Value::String(r(&s));
            }
        }
    }
    let diagnosis = rec
        .diagnostic
        .as_ref()
        .and_then(Diagnostic::from_json)
        .and_then(|d| {
            doctor::diagnose(
                &d,
                &doctor::Context {
                    cwd: PathBuf::from(&rec.cwd),
                    source: None,
                },
            )
        })
        .map(|dx| {
            let v = serde_json::to_value(&dx).unwrap_or_default();
            serde_json::from_str(&r(&v.to_string())).unwrap_or(v)
        });
    let crashes = cigscript::crash::list().unwrap_or_default();
    serde_json::json!({
        "kind": "cigscript-report",
        "cig": cigscript::VERSION,
        "generated": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "platform": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH},
        "run": {
            "id": rec.id,
            "version": rec.version,
            "script": r(&rec.script),
            "script_sha256": rec.script_sha256,
            "args": rec.args.iter().map(|a| redact_arg(a)).collect::<Vec<_>>(),
            "cwd": r(&rec.cwd),
            "mode": rec.mode,
            "started": rec.started,
            "finished": rec.finished,
            "status": rec.status,
            "error": rec.error.as_deref().map(r),
            "burns": rec.burns,
            "irreversible": rec.irreversible,
            "rolled_back": rec.rolled_back,
        },
        "diagnostic": diagnostic,
        "diagnosis": diagnosis,
        "journal": journal,
        "intents": intents,
        "install": {
            "state_dir": tilde(&runs::home_dir()),
            "tools": {"gh": update::has_tool("gh"), "curl": update::has_tool("curl")},
            "crash_reports_unsent": crashes.iter().filter(|c| c.sent.is_none()).count(),
        },
        "note": "Redacted like a crash report: home paths are ~, secret-shaped values are removed, file contents are never included. Read it before you send it.",
    })
}
