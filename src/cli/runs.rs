// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig runs` and `cig unburn`.

use super::{exit, Ctx};
use cigscript::burn::journal::{Before, Journal};
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
                    "interrupted" => ctx.red(&r.status),
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
                ctx.red("error[E803 usage]:"),
                all.len()
            );
            eprintln!("  = hint: `cig runs` lists them; give more of the id if it is ambiguous");
            eprintln!("  = explain: cig explain E803");
            return exit::USAGE;
        }
        Err(e) => {
            eprintln!("{} {e}", ctx.red("error[E803 usage]:"));
            eprintln!("  = explain: cig explain E803");
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
            outln!(
                "    {:>4}  {tag}  {}{}",
                e.seq,
                e.op.describe(),
                if e.undone {
                    ctx.dim("  (undone by a retry)")
                } else {
                    String::new()
                }
            );
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

pub fn unburn(ctx: &Ctx, id: &str, dry_run: bool, force: bool) -> i32 {
    let mut rec = match runs::find(id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            eprintln!("{} no run matches `{id}`", ctx.red("error[E803 usage]:"));
            eprintln!("  = hint: `cig runs` lists them; give more of the id if it is ambiguous");
            eprintln!("  = explain: cig explain E803");
            return exit::USAGE;
        }
        Err(e) => {
            eprintln!("{} {e}", ctx.red("error[E803 usage]:"));
            eprintln!("  = explain: cig explain E803");
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
    if (rec.status == "unburned" || rec.rolled_back) && !dry_run {
        if !force {
            eprintln!(
                "{} {} was already rolled back; doing it again would overwrite whatever happened since",
                ctx.red("error[E705 burn]:"),
                rec.id
            );
            eprintln!("  = hint: pass --force only if you mean to restore the old snapshots again");
            eprintln!("  = explain: cig explain E705");
            return exit::SCRIPT_ERROR;
        }
        eprintln!(
            "{} {} was already rolled back; --force given",
            ctx.yellow("note:"),
            rec.id
        );
    }
    let problems = journal.verify();
    let changed = journal.changed_since();
    // Who changed it since: a later run that touched the same path, if any.
    let later_runs: Vec<runs::RunRecord> = runs::list()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.id > rec.id && r.id != rec.id)
        .collect();
    let by_run = |path: &std::path::Path| -> Option<String> {
        later_runs.iter().find_map(|r| {
            Journal::load(&r.dir()).ok().and_then(|j| {
                j.touched_paths()
                    .iter()
                    .any(|(p, _)| p == path)
                    .then(|| r.id.clone())
            })
        })
    };
    if changed.known && !changed.changes.is_empty() && !dry_run {
        if !force {
            eprintln!(
                "{} cannot roll back {}: {} path{} changed since the run finished; restoring would overwrite that newer content",
                ctx.red("error[E704 burn]:"),
                rec.id,
                changed.changes.len(),
                if changed.changes.len() == 1 { "" } else { "s" }
            );
            for c in &changed.changes {
                let who = by_run(&c.path)
                    .map(|id| format!("; changed by run {id}, unburn that one first"))
                    .unwrap_or_default();
                eprintln!(
                    "  - {}: the run left {}, now {}{who}",
                    c.path.display(),
                    c.was.describe(),
                    c.now.describe()
                );
            }
            eprintln!("  = hint: look at the files, then pass --force if the old content is what you want; cig unburn {} --dry-run shows the changed-since column", rec.id);
            eprintln!("  = explain: cig explain E704");
            return exit::SCRIPT_ERROR;
        }
        eprintln!(
            "{} {} path(s) changed since the run; --force given, restoring over them",
            ctx.yellow("note:"),
            changed.changes.len()
        );
    }
    if !problems.is_empty() && !dry_run {
        if !force {
            eprintln!(
                "{} cannot roll back {}: {} problem{} found before touching anything",
                ctx.red("error[E703 burn]:"),
                rec.id,
                problems.len(),
                if problems.len() == 1 { "" } else { "s" }
            );
            for p in &problems {
                eprintln!("  - {p}");
            }
            eprintln!("  = hint: nothing was restored; pass --force to restore what can be and list what cannot");
            eprintln!("  = explain: cig explain E703");
            return exit::SCRIPT_ERROR;
        }
        eprintln!(
            "{} {} problem(s) with the journal; --force given, restoring what can be",
            ctx.yellow("note:"),
            problems.len()
        );
    }
    if dry_run {
        for p in &problems {
            outln!("  {} {p}", ctx.yellow("problem:"));
        }
        if !problems.is_empty() {
            outln!(
                "  {} a real unburn would refuse [E703]; --force restores what it can",
                ctx.yellow("note:")
            );
        }
        outln!(
            "{}",
            ctx.bold(&format!(
                "would restore {} entr{} from {}",
                journal.len(),
                if journal.len() == 1 { "y" } else { "ies" },
                rec.id
            ))
        );
        if !changed.known {
            outln!(
                "  {} this run predates after-state records; changed-since is unknown",
                ctx.dim("note:")
            );
        } else if !changed.changes.is_empty() {
            outln!(
                "  {} {} path{} changed since the run finished; a real unburn would refuse [E704] unless --force",
                ctx.yellow("note:"),
                changed.changes.len(),
                if changed.changes.len() == 1 { "" } else { "s" }
            );
        }
        for e in journal.entries().iter().rev() {
            let tag = if e.compensation.is_some() {
                "compensate"
            } else if e.reversible {
                "restore "
            } else if e.compensated {
                "compensated"
            } else {
                "cannot undo"
            };
            let touched: Vec<&std::path::PathBuf> = e
                .before
                .iter()
                .flat_map(|b| {
                    let mut v = vec![&b.path];
                    if let Before::Moved { to } = &b.before {
                        v.push(to);
                    }
                    v
                })
                .collect();
            let flagged: Vec<String> = changed
                .changes
                .iter()
                .filter(|c| touched.contains(&&c.path))
                .map(|c| {
                    let who = by_run(&c.path)
                        .map(|id| format!(" by run {id}"))
                        .unwrap_or_default();
                    format!("{}{who}", c.path.display())
                })
                .collect();
            let column = if !changed.known {
                ctx.dim("  changed since: unknown")
            } else if flagged.is_empty() {
                ctx.dim("  unchanged since")
            } else {
                ctx.yellow(&format!("  changed since: {}", flagged.join(", ")))
            };
            outln!("  {:>4}  {tag}  {}{column}", e.seq, e.op.describe());
        }
        return if problems.is_empty() {
            exit::OK
        } else {
            exit::SCRIPT_ERROR
        };
    }
    // Compensations run in a fresh interpreter, effects allowed, nothing
    // journaled: they are the undo. They run from the run's directory.
    let kernel = cigscript::burn::Kernel::ephemeral(cigscript::burn::Mode::Run);
    let mut interp = cigscript::interp::Interp::new(kernel);
    let mut entered = true;
    if let Ok(dir) = std::env::current_dir() {
        if rec.cwd != dir.to_string_lossy() && std::env::set_current_dir(&rec.cwd).is_err() {
            entered = false;
        }
    }
    let mut report = journal.rollback_with(&mut |c| {
        if !entered {
            return Err(format!("cannot enter {} to run it", rec.cwd));
        }
        interp.run_compensation(c).map_err(|e| e.message)
    });
    if !entered && !report.actions.is_empty() {
        report.failed.push(format!("could not enter {}", rec.cwd));
    }
    if ctx.json {
        outln!("{}", serde_json::to_string(&report).unwrap_or_default());
    } else {
        for (label, text) in &report.actions {
            outln!("  {} {text}", paint_action(ctx, label));
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

/// The colour for a rollback action's label.
pub fn paint_action(ctx: &Ctx, label: &str) -> String {
    match label {
        "restored" | "compensated" => ctx.green(label),
        "cannot undo" => ctx.yellow(label),
        "failed" => ctx.red(label),
        other => other.to_string(),
    }
}
