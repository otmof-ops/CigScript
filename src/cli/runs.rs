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
        &rec.script_sha256[..rec.script_sha256.len().min(12)]
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
    if !rec.rollback_failed.is_empty() {
        outln!(
            "  rollback  {} restore{} failed:",
            rec.rollback_failed.len(),
            if rec.rollback_failed.len() == 1 {
                ""
            } else {
                "s"
            }
        );
        for f in &rec.rollback_failed {
            outln!("    {} {f}", ctx.red("failed"));
        }
    }
    if !rec.pack.is_empty() {
        outln!(
            "  pack      {}",
            rec.pack
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
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
                } else if e.restored {
                    ctx.dim("  (put back)")
                } else {
                    String::new()
                }
            );
        }
        let taken: Vec<&cigscript::burn::journal::BeforeState> =
            j.entries().iter().flat_map(|e| e.took.iter()).collect();
        if !taken.is_empty() {
            outln!(
                "  took      {} file{} the rollback moved into the tin instead of destroying; cig unburn {} --give-back puts them back:",
                taken.len(),
                if taken.len() == 1 { "" } else { "s" },
                rec.id
            );
            for t in taken {
                outln!("    {}", t.path.display());
            }
        }
    }
    exit::OK
}

/// `cig unburn <id> --give-back`: put back what the run's rollback moved
/// into the tin. What is at those paths now goes to the tin in turn.
pub fn give_back(ctx: &Ctx, id: &str) -> i32 {
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
            eprintln!("  = explain: cig explain E803");
            return exit::USAGE;
        }
    };
    let mut journal = match Journal::load(&rec.dir()) {
        Ok(j) => j,
        Err(e) => {
            eprintln!(
                "{} cannot load the journal for {}: {e}",
                ctx.red("error[E703 burn]:"),
                rec.id
            );
            eprintln!("  = explain: cig explain E703");
            return exit::SCRIPT_ERROR;
        }
    };
    if !journal.has_taken() {
        eprintln!(
            "the rollback of {} moved nothing into the tin; there is nothing to give back",
            rec.id
        );
        return exit::OK;
    }
    let _lock = match cigscript::burn::lock::acquire(&rec.pack, &rec.id) {
        Ok(l) => Some(l),
        Err(cigscript::burn::lock::Busy::Held { root, holder }) => {
            eprintln!(
                "{} the pack {} is in use by {holder}",
                ctx.red("error[E759 burn]:"),
                root.display()
            );
            eprintln!(
                "  = hint: wait for that run to finish (cig runs shows it); nothing was changed"
            );
            eprintln!("  = explain: cig explain E759");
            return exit::SCRIPT_ERROR;
        }
        Err(cigscript::burn::lock::Busy::Io(_)) => None,
    };
    let report = journal.give_back();
    if ctx.json {
        outln!("{}", serde_json::to_string(&report).unwrap_or_default());
    } else {
        for (label, text) in &report.actions {
            outln!("  {} {text}", paint_action(ctx, label));
        }
    }
    if report.clean() {
        exit::OK
    } else {
        exit::SCRIPT_ERROR
    }
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
        Err(e) if e.kind() == std::io::ErrorKind::Unsupported || rec.status == "newer" => {
            return hand_back(ctx, &rec, &e);
        }
        Err(e) => {
            eprintln!(
                "{} cannot load the journal for {}: {e}",
                ctx.red("error[E703 burn]:"),
                rec.id
            );
            eprintln!(
                "  = hint: a line before the last is damaged; cig report {} bundles the journal for a bug report",
                rec.id
            );
            eprintln!("  = explain: cig explain E703");
            return exit::SCRIPT_ERROR;
        }
    };
    if journal.truncated() {
        eprintln!(
            "{} the journal's last line was cut short (a crash while appending); the {} entr{} before it loaded and can be restored",
            ctx.yellow("note:"),
            journal.len(),
            if journal.len() == 1 { "y" } else { "ies" }
        );
    }
    if journal.is_empty() {
        eprintln!("{} burned nothing; there is nothing to unburn", rec.id);
        return exit::OK;
    }
    // A rollback that left restores failed is finished by a plain unburn:
    // the entries restored the first time are skipped, the failed ones
    // tried again. `--force` restores everything again, on purpose.
    let retry = !force && rec.rolled_back && !rec.rollback_failed.is_empty();
    if (rec.status == "unburned" || rec.rolled_back) && !dry_run && !retry {
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
    if retry && !dry_run {
        eprintln!(
            "{} {} restore{} failed last time; trying {} again and leaving the rest as it is",
            ctx.yellow("note:"),
            rec.rollback_failed.len(),
            if rec.rollback_failed.len() == 1 {
                ""
            } else {
                "s"
            },
            if rec.rollback_failed.len() == 1 {
                "it"
            } else {
                "them"
            }
        );
    }
    if !dry_run {
        // The record must be writable before anything is restored: without
        // it a second unburn could not be refused.
        if let Err(e) = rec.save() {
            if !force {
                eprintln!(
                    "{} cannot record the rollback in {}: {e}",
                    ctx.red("error[E802 usage]:"),
                    rec.dir().display()
                );
                eprintln!("  = hint: fix the permissions on that directory first, or pass --force to restore without a record; nothing was restored");
                eprintln!("  = explain: cig explain E802");
                return exit::USAGE;
            }
            eprintln!(
                "{} cannot record the rollback in {}: {e}; --force given",
                ctx.yellow("note:"),
                rec.dir().display()
            );
        }
    }
    // A run using this pack now would have the restore read as its hop's
    // writes; the run holds the lock until it is over.
    let _lock = if dry_run {
        None
    } else {
        match cigscript::burn::lock::acquire(&rec.pack, &rec.id) {
            Ok(l) => Some(l),
            Err(cigscript::burn::lock::Busy::Held { root, holder }) => {
                eprintln!(
                    "{} the pack {} is in use by {holder}",
                    ctx.red("error[E759 burn]:"),
                    root.display()
                );
                eprintln!("  = hint: wait for that run to finish (cig runs shows it), then unburn; nothing was changed");
                eprintln!("  = explain: cig explain E759");
                return exit::SCRIPT_ERROR;
            }
            Err(cigscript::burn::lock::Busy::Io(_)) => None,
        }
    };
    let problems = journal.verify();
    // A retry asks only about the paths it will touch: what the first pass
    // put back may have been edited since, on purpose.
    let changed = if retry {
        journal.changed_since_pending()
    } else {
        journal.changed_since()
    };
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
    // The compensation sees the run's own arguments, as it did in the run.
    interp.set_args(&rec.args);
    let mut entered = true;
    if rec.cwd.is_empty() {
        eprintln!(
            "{} the run's working directory is not recorded; relative paths are taken from here",
            ctx.yellow("note:")
        );
    } else if let Ok(dir) = std::env::current_dir() {
        if rec.cwd != dir.to_string_lossy() && std::env::set_current_dir(&rec.cwd).is_err() {
            entered = false;
        }
    }
    let mut runner = |c: &cigscript::burn::journal::Compensation| -> Result<(), String> {
        if !entered {
            return Err(format!("cannot enter {} to run it", rec.cwd));
        }
        interp.run_compensation(c).map_err(|e| e.message)
    };
    let mut report = if force {
        journal.rollback_again_with(&mut runner)
    } else {
        journal.rollback_with(&mut runner)
    };
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
    rec.rollback_failed = report.failed.clone();
    rec.rollback_took = report.took.len();
    rec.status = if report.clean() {
        "unburned".to_string()
    } else {
        "unburn_failed".to_string()
    };
    if let Err(e) = rec.save() {
        eprintln!(
            "{} could not record the rollback in {}: {e}",
            ctx.yellow("warning:"),
            rec.dir().display()
        );
    }
    if report.clean() {
        exit::OK
    } else {
        exit::SCRIPT_ERROR
    }
}

/// A run journaled by a newer cig is put back by that cig: the whole
/// `unburn` is handed to its version from the humidor, or refused with
/// the command that fetches it. Nothing is parsed by guess.
fn hand_back(ctx: &Ctx, rec: &RunRecord, e: &std::io::Error) -> i32 {
    let wrote = rec.written_by();
    if cigscript::pin::dispatched() {
        eprintln!("{} {e}", ctx.red("error[E808 usage]:"));
        eprintln!("  = explain: cig explain E808");
        return exit::USAGE;
    }
    if let Some(v) = &wrote {
        if let Some(bin) = cigscript::humidor::binary(v) {
            if !ctx.json {
                eprintln!(
                    "{}",
                    ctx.dim(&format!(
                        "unburn handed to cig {v}, which wrote this journal; installed is {}",
                        cigscript::VERSION
                    ))
                );
            }
            let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
            cigscript::humidor::touch(v);
            return match cigscript::humidor::dispatch(&bin, &args) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("{} {e}", ctx.red("error[E808 usage]:"));
                    exit::USAGE
                }
            };
        }
    }
    eprintln!("{} {e}", ctx.red("error[E808 usage]:"));
    match wrote {
        Some(v) => eprintln!(
            "  = hint: cig versions add {v} puts that cig in the humidor; cig unburn {} then hands the run to it",
            rec.id
        ),
        None => eprintln!("  = hint: the record does not say which cig wrote it; cig update to the newest release and try again"),
    }
    eprintln!("  = explain: cig explain E808");
    exit::USAGE
}

/// The colour for a rollback action's label.
pub fn paint_action(ctx: &Ctx, label: &str) -> String {
    match label {
        "restored" | "compensated" | "gave back" => ctx.green(label),
        "cannot undo" | "kept" | "left" | "not undone" => ctx.yellow(label),
        "failed" => ctx.red(label),
        other => other.to_string(),
    }
}
