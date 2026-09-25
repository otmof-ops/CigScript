// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig runs` and `cig unburn`.

use super::{exit, Ctx};
use cigscript::burn::journal::Journal;
use cigscript::burn::runs::{self, RunRecord};

pub fn runs(ctx: &Ctx, id: Option<String>, prune: Option<usize>) -> i32 {
    if let Some(keep) = prune {
        return prune_runs(ctx, keep);
    }
    let all = match runs::list() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{} cannot list runs: {e}", ctx.red("error:"));
            return exit::USAGE;
        }
    };
    match id {
        Some(id) => show(ctx, &id, &all),
        None => {
            if ctx.json {
                outln!("{}", serde_json::to_string(&all).unwrap_or_default());
                return exit::OK;
            }
            if all.is_empty() {
                eprintln!(
                    "no runs recorded yet (state dir: {})",
                    runs::runs_dir().display()
                );
                return exit::OK;
            }
            outln!(
                "{:<24} {:<12} {:>5} {:>4}  {}",
                "run",
                "status",
                "burns",
                "irr",
                "script"
            );
            for r in &all {
                let status = match r.status.as_str() {
                    "ok" => ctx.green(&r.status),
                    "rolled_back" | "unburned" => ctx.yellow(&r.status),
                    "running" => ctx.dim(&r.status),
                    _ => ctx.red(&r.status),
                };
                outln!(
                    "{:<24} {:<12} {:>5} {:>4}  {}",
                    r.id,
                    status,
                    r.burns,
                    r.irreversible,
                    r.script
                );
            }
            exit::OK
        }
    }
}

fn show(ctx: &Ctx, id: &str, all: &[RunRecord]) -> i32 {
    let rec = match runs::find(id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            eprintln!(
                "{} no run matches `{id}` ({} recorded)",
                ctx.red("error:"),
                all.len()
            );
            return exit::USAGE;
        }
        Err(e) => {
            eprintln!("{} {e}", ctx.red("error:"));
            return exit::USAGE;
        }
    };
    let journal = Journal::load(&rec.dir()).ok();
    if ctx.json {
        let obj = serde_json::json!({
            "run": rec,
            "journal": journal.as_ref().map(|j| j.entries().to_vec()),
        });
        outln!("{}", serde_json::to_string(&obj).unwrap_or_default());
        return exit::OK;
    }
    outln!("{}", ctx.bold(&rec.id));
    outln!(
        "  script    {}  (sha256 {})",
        rec.script,
        &rec.script_sha256[..12]
    );
    if !rec.args.is_empty() {
        outln!("  args      {}", rec.args.join(" "));
    }
    outln!("  cwd       {}", rec.cwd);
    outln!("  mode      {}", rec.mode);
    outln!("  started   {}", rec.started);
    outln!("  finished  {}", rec.finished.as_deref().unwrap_or("-"));
    outln!("  status    {}", rec.status);
    if let Some(e) = &rec.error {
        outln!("  error     {e}");
    }
    outln!(
        "  burns     {} ({} irreversible)",
        rec.burns,
        rec.irreversible
    );
    outln!("  dir       {}", rec.dir().display());
    if let Some(j) = journal {
        outln!(
            "  journal   {} entr{}",
            j.len(),
            if j.len() == 1 { "y" } else { "ies" }
        );
        for e in j.entries() {
            let tag = if e.reversible {
                "reversible  "
            } else {
                "irreversible"
            };
            outln!("    {:>4}  {tag}  {}", e.seq, e.op.describe());
        }
    }
    exit::OK
}

fn prune_runs(ctx: &Ctx, keep: usize) -> i32 {
    let all = match runs::list() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{} cannot list runs: {e}", ctx.red("error:"));
            return exit::USAGE;
        }
    };
    let mut removed = 0;
    for r in all.iter().skip(keep) {
        if std::fs::remove_dir_all(r.dir()).is_ok() {
            removed += 1;
        }
    }
    if ctx.json {
        outln!(
            "{}",
            serde_json::json!({ "removed": removed, "kept": all.len().min(keep) })
        );
    } else {
        eprintln!(
            "removed {removed} run record{}, kept {}",
            if removed == 1 { "" } else { "s" },
            all.len().min(keep)
        );
    }
    exit::OK
}

pub fn unburn(ctx: &Ctx, id: &str, dry_run: bool) -> i32 {
    let mut rec = match runs::find(id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            eprintln!("{} no run matches `{id}`", ctx.red("error:"));
            return exit::USAGE;
        }
        Err(e) => {
            eprintln!("{} {e}", ctx.red("error:"));
            return exit::USAGE;
        }
    };
    let mut journal = match Journal::load(&rec.dir()) {
        Ok(j) => j,
        Err(e) => {
            eprintln!(
                "{} cannot load the journal for {}: {e}",
                ctx.red("error:"),
                rec.id
            );
            return exit::USAGE;
        }
    };
    if journal.is_empty() {
        eprintln!("{} burned nothing; there is nothing to unburn", rec.id);
        return exit::OK;
    }
    if rec.status == "unburned" || rec.rolled_back {
        eprintln!("{} {} was already rolled back", ctx.yellow("note:"), rec.id);
    }
    if dry_run {
        outln!(
            "{}",
            ctx.bold(&format!(
                "would restore {} entr{} from {}",
                journal.len(),
                if journal.len() == 1 { "y" } else { "ies" },
                rec.id
            ))
        );
        for e in journal.entries().iter().rev() {
            let tag = if e.reversible {
                "restore "
            } else {
                "cannot undo"
            };
            outln!("  {:>4}  {tag}  {}", e.seq, e.op.describe());
        }
        return exit::OK;
    }
    let report = journal.rollback();
    if ctx.json {
        outln!("{}", serde_json::to_string(&report).unwrap_or_default());
    } else {
        for r in &report.restored {
            outln!("  {} {r}", ctx.green("restored"));
        }
        for r in &report.irreversible {
            outln!("  {} {r}", ctx.yellow("cannot undo"));
        }
        for r in &report.failed {
            outln!("  {} {r}", ctx.red("failed"));
        }
    }
    rec.rolled_back = true;
    rec.status = if report.clean() {
        "unburned".to_string()
    } else {
        "unburn_failed".to_string()
    };
    let _ = rec.save();
    if report.clean() {
        exit::OK
    } else {
        exit::SCRIPT_ERROR
    }
}
