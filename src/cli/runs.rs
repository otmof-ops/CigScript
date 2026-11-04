// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig runs` and `cig unburn`.

use super::{exit, Ctx};
use cigscript::burn::journal::{Before, Journal};
use cigscript::burn::runs::{self, RunRecord};

pub fn runs(
    ctx: &Ctx,
    id: Option<String>,
    prune: Option<usize>,
    verify: bool,
    forget: Option<String>,
) -> i32 {
    if let Some(keep) = prune {
        return prune_runs(ctx, keep);
    }
    if let Some(gone) = forget {
        // The user says the run is gone on purpose: the ledger records it,
        // and the missing-run note stops.
        return match cigscript::burn::ledger::append("forgotten", &gone, "", "forgotten", 0) {
            Ok(()) => {
                eprintln!(
                    "{} {gone} will not be noted as missing again",
                    ctx.green("ok:")
                );
                exit::OK
            }
            Err(e) => {
                eprintln!("{} could not write the ledger: {e}", ctx.red("error:"));
                exit::SCRIPT_ERROR
            }
        };
    }
    let all = match runs::list() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{} cannot list runs: {e}", ctx.red("error:"));
            return exit::USAGE;
        }
    };
    match id {
        Some(id) => show(ctx, &id, &all, verify),
        None => {
            if ctx.json {
                outln!("{}", serde_json::to_string(&all).unwrap_or_default());
                return exit::OK;
            }
            note_missing_runs(ctx, &all);
            // The layers an interrupted run never applied are no one's:
            // cig was not there to apply them, so they go (the ones kept
            // on purpose stay).
            for r in all.iter().filter(|r| r.is_interrupted()) {
                let n = cigscript::burn::overlay::drop_unapplied(&r.dir());
                if n > 0 {
                    eprintln!(
                        "{} {} died in a boxed hop; the layer{} it never applied {} removed (cig unburn {} puts its earlier burns back)",
                        ctx.yellow("note:"),
                        r.id,
                        if n == 1 { "" } else { "s" },
                        if n == 1 { "was" } else { "were" },
                        r.id
                    );
                }
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

fn show(ctx: &Ctx, id: &str, all: &[RunRecord], verify: bool) -> i32 {
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
    let checked = if verify {
        journal.as_ref().map(|j| verify_run(&rec, j))
    } else {
        None
    };
    if ctx.json {
        let obj = serde_json::json!({
            "run": rec,
            "journal": journal.as_ref().map(|j| j.entries().to_vec()),
            "verify": checked,
        });
        outln!("{}", serde_json::to_string(&obj).unwrap_or_default());
        return match &checked {
            Some(v) if !v.ok => exit::SCRIPT_ERROR,
            _ => exit::OK,
        };
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
    if let Some(parent) = &rec.parent {
        outln!(
            "  parent    {parent}  (a cig inside a hop of that run, taken in when the hop ended; that run's rollback undoes this one through the burn protocol)"
        );
    }
    let children: Vec<&str> = all
        .iter()
        .filter(|r| r.parent.as_deref() == Some(rec.id.as_str()))
        .map(|r| r.id.as_str())
        .collect();
    if !children.is_empty() {
        outln!(
            "  children  {}  (cigs inside this run's hops)",
            children.join(" ")
        );
    }
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
    if let Some(imp) = &rec.imported {
        outln!(
            "  imported  from {} at {} (paths read through its map)",
            imp["from"].as_str().unwrap_or("?"),
            imp["at"].as_str().unwrap_or("?")
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
    if let Some(v) = checked {
        outln!(
            "  chain     {}",
            if v.chain_ok {
                v.chain_text
            } else {
                ctx.red(&v.chain_text)
            }
        );
        outln!(
            "  head      {}",
            if v.head_ok {
                v.head_text
            } else {
                ctx.red(&v.head_text)
            }
        );
        if v.problems.is_empty() {
            outln!(
                "  objects   every snapshot and tin object the journal needs is there and hashes as recorded"
            );
        } else {
            outln!(
                "  objects   {} problem{}:",
                v.problems.len(),
                if v.problems.len() == 1 { "" } else { "s" }
            );
            for p in &v.problems {
                outln!("    {} {p}", ctx.red("problem"));
            }
        }
        return if v.ok { exit::OK } else { exit::SCRIPT_ERROR };
    }
    exit::OK
}

/// What `cig runs <id> --verify` found.
#[derive(serde::Serialize)]
struct Verified {
    chain: cigscript::burn::journal::Chain,
    chain_ok: bool,
    chain_text: String,
    head_recorded: Option<String>,
    head: String,
    head_ok: bool,
    head_text: String,
    problems: Vec<String>,
    ok: bool,
}

/// Walk the journal's chain, compare its head with the record, and check
/// every snapshot and tin object a rollback would need.
/// Is this journal the record the run wrote? The first line whose `prev`
/// does not fit, or, when every line fits, a last line that is not the one
/// the record saved as the chain's head (edited, or lines added or removed
/// at the end after the record was written; an unburn or a give-back
/// saves the head again after the markers it appends).
/// Runs the ledger lists that are not in `runs/` and were not pruned or
/// forgotten: one calm note (E710), never an alarm. Only a ledger line that
/// fails its MAC is a tripwire, and that is reported by the verify path.
fn note_missing_runs(ctx: &Ctx, all: &[RunRecord]) {
    let Ok(ledger) = cigscript::burn::ledger::load() else {
        return;
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut missing: Vec<String> = Vec::new();
    // Only this tin's lines: another state directory beside the same key
    // keeps its own runs, and they are not missing from here.
    for l in ledger.of_this_tin().rev() {
        if l.kind != "run" && l.kind != "unburn" && l.kind != "origin" {
            continue;
        }
        if !seen.insert(l.run.clone()) {
            continue;
        }
        if all.iter().any(|r| r.id == l.run) || cigscript::burn::ledger::is_let_go(&ledger, &l.run)
        {
            continue;
        }
        missing.push(l.run.clone());
    }
    if missing.is_empty() {
        return;
    }
    missing.sort();
    for id in &missing {
        eprintln!(
            "{} run {id} is in the ledger but not in runs/ (removed by hand?); cig runs --forget {id} to stop this note",
            ctx.yellow("note[E710 burn]:")
        );
    }
}

/// What the ledger says this run's journal ended at, against the journal
/// on disk: `Some(reason)` when the journal was cut short or swapped for
/// an older copy (E709). `None` when it fits, when the ledger has no entry
/// for the run (a run from before the ledger), or when the ledger cannot
/// be read (the key is not here: nothing to compare with).
fn ledger_disagrees(rec: &RunRecord, journal: &Journal) -> Option<String> {
    // A journal under another key cannot be judged against this ledger, and
    // a ledger whose own chain is broken judges nothing (that is E709 in
    // `cig tin verify`, not a verdict on this run).
    if journal.chain().other_key || journal.chain().unverified {
        return None;
    }
    // The record speaks first: a journal whose head is not the one its own
    // record saved is reported by `tampered` (E760). The ledger judges what
    // the record cannot: a journal cut short or swapped whose record was
    // made to agree with it, or carries no head at all.
    if rec
        .journal_head
        .as_deref()
        .is_some_and(|h| h != journal.head())
    {
        return None;
    }
    let ledger = cigscript::burn::ledger::load().ok()?;
    if ledger.broken_at.is_some() {
        return None;
    }
    let line = cigscript::burn::ledger::last_record_of(&ledger, &rec.id)?;
    let lines = journal.chain().lines as u64;
    if lines < line.lines {
        return Some(format!(
            "this run's journal ends at line {lines}, but the ledger says it went to {}: cut short",
            line.lines
        ));
    }
    if line.head != journal.head() && lines == line.lines {
        return Some(
            "this is an old copy of the record: the ledger has a later one with the same length and another head".to_string(),
        );
    }
    None
}

fn tampered(rec: &RunRecord, journal: &Journal) -> Option<(usize, String)> {
    let chain = journal.chain();
    if let Some(b) = &chain.broken_at {
        return Some((b.line, b.reason.clone()));
    }
    // Under another key, or with no key to check with, the head computed
    // here is not the one the record saved: that is E708's business, or a
    // note, not tampering.
    if !chain.checked() || chain.other_key || chain.unverified {
        return None;
    }
    match &rec.journal_head {
        Some(h) if *h != journal.head() => Some((
            chain.lines,
            "is not the last line the record saved as the chain's head: it was edited, or lines were added or removed at the end".to_string(),
        )),
        _ => None,
    }
}

fn verify_run(rec: &RunRecord, journal: &Journal) -> Verified {
    let chain = journal.chain().clone();
    let (chain_ok, chain_text) = if !chain.checked() {
        (
            true,
            format!(
                "none: the journal is format {} (from before 1.5.0), so its lines carry no prev",
                chain.format
            ),
        )
    } else if let Some(b) = &chain.broken_at {
        (false, format!("BROKEN at line {}: {}", b.line, b.reason))
    } else {
        let renamed = match &chain.run {
            Some(r) if *r != rec.id => {
                format!(" (written as run {r}; the directory was renamed since)")
            }
            _ => String::new(),
        };
        (
            true,
            format!(
                "intact: {} line{}, each carrying the hash of the one before it{renamed}",
                chain.lines,
                if chain.lines == 1 { "" } else { "s" }
            ),
        )
    };
    let head = journal.head().to_string();
    let (head_ok, head_text) = match &rec.journal_head {
        _ if !chain.checked() => (
            true,
            "not checked: a journal without a chain has no head to compare".to_string(),
        ),
        None => (
            true,
            "not recorded yet (the run has not finished, or died before it could)".to_string(),
        ),
        Some(h) if *h == head => (true, "matches the record: no line was added or removed since it was saved".to_string()),
        Some(_) => (
            false,
            "does not match the record: lines were added, removed or changed after the record was saved".to_string(),
        ),
    };
    let problems = journal.verify();
    let ok = chain_ok && head_ok && problems.is_empty();
    Verified {
        chain,
        chain_ok,
        chain_text,
        head_recorded: rec.journal_head.clone(),
        head,
        head_ok,
        head_text,
        problems,
        ok,
    }
}

/// `cig unburn <id> --give-back`: put back what the run's rollback moved
/// into the tin. What is at those paths now goes to the tin in turn. With
/// `--dry-run` the list alone, with what is there now and whether it is
/// still what the rollback left; a path changed since is refused without
/// `--force`, as a rollback over a later edit is.
pub fn give_back(ctx: &Ctx, id: &str, dry_run: bool, force: bool) -> i32 {
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
    // The took markers are read from the journal: a journal that is not
    // the record is given back from only on purpose, as it is unburned.
    if let Some((line, reason)) = tampered(&rec, &journal) {
        if !force && !dry_run {
            eprintln!(
                "{} the journal of {} is not the record the run wrote: line {line} {reason}",
                ctx.red("error[E760 burn]:"),
                rec.id,
            );
            eprintln!(
                "  = hint: cig runs {} --verify walks the chain; pass --force to give back from the journal as it is, knowing it was changed",
                rec.id
            );
            eprintln!("  = explain: cig explain E760");
            return exit::SCRIPT_ERROR;
        }
        eprintln!(
            "{} the journal's chain is broken at line {line} ({reason}); {}",
            ctx.yellow("note:"),
            if dry_run {
                "a real give-back would refuse [E760] unless --force"
            } else {
                "--force given"
            }
        );
    }
    let plan = journal.give_back_plan();
    let changed: Vec<&cigscript::burn::journal::GiveBack> =
        plan.iter().filter(|g| g.changed).collect();
    if dry_run {
        if ctx.json {
            outln!(
                "{}",
                serde_json::json!({"run": rec.id, "would_give_back": plan})
            );
            return exit::OK;
        }
        outln!(
            "{}",
            ctx.bold(&format!(
                "would give back {} file{} the rollback of {} moved into the tin",
                plan.len(),
                if plan.len() == 1 { "" } else { "s" },
                rec.id
            ))
        );
        if !changed.is_empty() {
            outln!(
                "  {} {} path{} changed since the rollback; a real give-back would refuse [E704] unless --force",
                ctx.yellow("note:"),
                changed.len(),
                if changed.len() == 1 { "" } else { "s" }
            );
        }
        for g in &plan {
            let column = match (&g.left, g.changed) {
                (None, _) => ctx.dim("  changed since: unknown"),
                (Some(_), false) => ctx.dim("  unchanged since the rollback"),
                (Some(_), true) => ctx.yellow("  changed since the rollback"),
            };
            outln!(
                "  {:>4}  give back  {}  <- {} bytes, sha256 {} (now {}){column}",
                g.seq,
                g.path.display(),
                g.bytes,
                g.sha256.get(..12).unwrap_or(&g.sha256),
                g.now.describe()
            );
        }
        outln!("{}", ctx.dim("nothing was changed"));
        return exit::OK;
    }
    if !changed.is_empty() {
        if !force {
            eprintln!(
                "{} cannot give back for {}: {} path{} changed since the rollback; putting the tin's bytes back would overwrite that newer content",
                ctx.red("error[E704 burn]:"),
                rec.id,
                changed.len(),
                if changed.len() == 1 { "" } else { "s" }
            );
            for g in &changed {
                eprintln!(
                    "  - {}: the rollback left {}, now {}",
                    g.path.display(),
                    g.left
                        .as_ref()
                        .map(|l| l.describe())
                        .unwrap_or_else(|| "something else".to_string()),
                    g.now.describe()
                );
            }
            eprintln!(
                "  = hint: look at the files, then pass --force if the tin's bytes are what you want; cig unburn {} --give-back --dry-run shows the changed-since column",
                rec.id
            );
            eprintln!("  = explain: cig explain E704");
            return exit::SCRIPT_ERROR;
        }
        eprintln!(
            "{} {} path(s) changed since the rollback; --force given, giving back over them",
            ctx.yellow("note:"),
            changed.len()
        );
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
    let mut rec = rec;
    rec.journal_head = Some(journal.head().to_string());
    let _ = cigscript::burn::ledger::append(
        "unburn",
        &rec.id,
        journal.head(),
        "given_back",
        journal.chain().lines as u64,
    );
    if let Err(e) = rec.save() {
        eprintln!(
            "{} could not record the give-back in {}: {e}",
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
        // The blobs the run pinned in git go free with its record.
        if let Ok(text) = std::fs::read_to_string(r.dir().join(cigscript::burn::git::PINS)) {
            for wt in text.lines().filter(|l| !l.trim().is_empty()) {
                cigscript::burn::git::unpin(std::path::Path::new(wt.trim()), &r.id);
            }
        }
        if cigscript::burn::overlay::remove_tree_force(&r.dir()).is_ok() {
            removed += 1;
            // Recorded, so the missing-run note never fires for it.
            let _ = cigscript::burn::ledger::append("pruned", &r.id, "", "pruned", 0);
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

pub fn unburn(ctx: &Ctx, id: &str, dry_run: bool, force: bool, accept_unsigned: bool) -> i32 {
    // A rollback writes run state: the state directory must be ours alone.
    if !dry_run {
        if let Err(e) = cigscript::burn::ensure_private() {
            eprintln!("{} {e}", ctx.red("error[E707 burn]:"));
            eprintln!("  = explain: cig explain E707");
            return exit::SCRIPT_ERROR;
        }
    }
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
    if journal.chain().unverified {
        eprintln!(
            "{} this run's journal is signed and the tin's key is not available to check it; restoring by the hashes it carries",
            ctx.yellow("note:")
        );
    }
    // A signed journal under another key (another machine's, or one since
    // replaced) is not tampering: its objects still verify by hash, and
    // `--accept-unsigned` restores from it after you have read that.
    if journal.chain().other_key {
        if !accept_unsigned && !dry_run {
            eprintln!(
                "{} this run was signed by another key; its files still match their hashes",
                ctx.red("error[E708 burn]:")
            );
            eprintln!(
                "  = hint: cig unburn {} --accept-unsigned restores it anyway (sealed objects still need the key that sealed them: cig tin key import)",
                rec.id
            );
            eprintln!("  = explain: cig explain E708");
            return exit::SCRIPT_ERROR;
        }
        eprintln!(
            "{} this run was signed by another key; restoring from it as its hashes allow",
            ctx.yellow("note:")
        );
    }
    // The ledger recorded where this run's journal ended: a journal cut
    // short, or an older copy swapped in, is caught here.
    if let Some(reason) = ledger_disagrees(&rec, &journal) {
        if !force && !dry_run {
            eprintln!("{} {reason}", ctx.red("error[E709 burn]:"));
            eprintln!("  = hint: the record was changed since it was written; --force restores from it as it is");
            eprintln!("  = explain: cig explain E709");
            return exit::SCRIPT_ERROR;
        }
        eprintln!(
            "{} {reason}; {}",
            ctx.yellow("note:"),
            if dry_run {
                "a real unburn would refuse [E709] unless --force"
            } else {
                "--force given"
            }
        );
    }
    // Every line of a format 2 journal carries the hash of the one before
    // it: a journal that no longer fits itself is not the record, and is
    // restored from only on purpose.
    if let Some((line, reason)) = tampered(&rec, &journal) {
        if !force && !dry_run {
            eprintln!(
                "{} the journal of {} is not the record the run wrote: line {line} {reason}",
                ctx.red("error[E760 burn]:"),
                rec.id,
            );
            eprintln!(
                "  = hint: cig runs {} --verify walks the chain; pass --force to restore from the journal as it is, knowing it was changed",
                rec.id
            );
            eprintln!("  = explain: cig explain E760");
            return exit::SCRIPT_ERROR;
        }
        if dry_run {
            outln!(
                "  {} the journal's chain is broken at line {line} ({reason}); a real unburn would refuse [E760] unless --force",
                ctx.yellow("note:"),
            );
        } else {
            eprintln!(
                "{} the journal's chain is broken at line {line} ({reason}); --force given",
                ctx.yellow("note:"),
            );
        }
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
    // journaled: they are the undo. They run from the run's directory, and a
    // command among them (a hop's own undo) sees this run's id, so a cig it
    // starts may enter the pack this unburn holds.
    std::env::set_var("CIG_RUN_ID", &rec.id);
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
    if let Err(e) = cigscript::burn::ledger::append(
        "unburn",
        &rec.id,
        journal.head(),
        if report.clean() {
            "unburned"
        } else {
            "unburn_failed"
        },
        journal.chain().lines as u64,
    ) {
        eprintln!("{} could not write the ledger: {e}", ctx.yellow("warning:"));
    }
    // The layers of a boxed hop the run never got to apply (it died in the
    // hop) were never the pack's: they go with the rollback.
    let _ = cigscript::burn::overlay::remove_tree_force(&rec.dir().join("box"));
    let _ = cigscript::burn::overlay::remove_tree_force(&cigscript::burn::overlay::inbox_dir(
        &rec.dir(),
    ));
    rec.rollback_took = report.took.len();
    rec.journal_head = Some(journal.head().to_string());
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
    // The tripwires, as after a run's own rollback: every restored path
    // read back against the journal, a sample of the tin re-hashed. A
    // broken promise is a report; the unburn's outcome is what it was.
    let mut trips = Vec::new();
    trips.extend(cigscript::tripwire::after_rollback(&journal, &report));
    trips.extend(cigscript::tripwire::tin_sample(&journal));
    super::run::settle_trips(ctx, trips, &rec, &journal);
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
