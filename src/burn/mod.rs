// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! The kernel: gates side effects, journals them, rolls them back.
//!
//! Every effectful builtin describes what it is about to do as an [`Op`]
//! and asks the [`Kernel`] for a [`Decision`] before touching the world.
//! In `run` mode the kernel snapshots whatever the op will change and
//! appends a journal entry; in dry-run mode (and inside `burn unlit`) it
//! records the intent and tells the builtin to simulate.

pub mod ghost;
pub mod journal;
pub mod runs;

use crate::diagnostics::{burn as burn_error, Diagnostic};
use journal::Journal;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A side effect a builtin wants to perform.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    Write {
        path: PathBuf,
    },
    Append {
        path: PathBuf,
    },
    Delete {
        path: PathBuf,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
    },
    Copy {
        from: PathBuf,
        to: PathBuf,
    },
    Mkdir {
        path: PathBuf,
    },
    Proc {
        cmd: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
    },
    EnvSet {
        key: String,
    },
    EnvUnset {
        key: String,
    },
    /// A file a hop created inside the pack (observed after the hop).
    HopCreated {
        path: PathBuf,
    },
    /// A file a hop modified or deleted inside the pack (observed; the
    /// kernel has no before-image, so it cannot be undone).
    HopChanged {
        path: PathBuf,
        change: String,
    },
    /// The compensation recorded when a `burn { } unburn { }` block ends.
    Compensate {
        line: u32,
    },
}

impl Op {
    pub fn label(&self) -> &'static str {
        match self {
            Op::Write { .. } => "write",
            Op::Append { .. } => "append",
            Op::Delete { .. } => "delete",
            Op::Move { .. } => "move",
            Op::Copy { .. } => "copy",
            Op::Mkdir { .. } => "mkdir",
            Op::Proc { .. } => "proc",
            Op::EnvSet { .. } => "env_set",
            Op::EnvUnset { .. } => "env_unset",
            Op::HopCreated { .. } => "hop-created",
            Op::HopChanged { .. } => "hop-changed",
            Op::Compensate { .. } => "compensate",
        }
    }

    /// Can rollback undo this op on its own? Processes and environment
    /// changes cannot be watched or restored by the kernel, so they are
    /// irreversible and the plan says so.
    pub fn reversible(&self) -> bool {
        !matches!(
            self,
            Op::Proc { .. } | Op::HopChanged { .. } | Op::EnvSet { .. } | Op::EnvUnset { .. }
        )
    }

    pub fn describe(&self) -> String {
        match self {
            Op::Write { path } => format!("write {}", path.display()),
            Op::Append { path } => format!("append to {}", path.display()),
            Op::Delete { path } => format!("delete {}", path.display()),
            Op::Move { from, to } => format!("move {} -> {}", from.display(), to.display()),
            Op::Copy { from, to } => format!("copy {} -> {}", from.display(), to.display()),
            Op::Mkdir { path } => format!("mkdir {}", path.display()),
            Op::Proc { cmd, args, cwd } => {
                let mut s = format!("run {}", shell_quote(cmd));
                for a in args {
                    s.push(' ');
                    s.push_str(&shell_quote(a));
                }
                if let Some(c) = cwd {
                    s.push_str(&format!("  (in {})", c.display()));
                }
                s
            }
            Op::EnvSet { key } => format!("set env {key}"),
            Op::EnvUnset { key } => format!("unset env {key}"),
            Op::HopCreated { path } => format!("hop created {}", path.display()),
            Op::HopChanged { path, change } => format!("hop {change} {}", path.display()),
            Op::Compensate { line } => format!("compensation for the burn at line {line}"),
        }
    }
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Which block class an op was requested under.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BurnClass {
    Burn,
    Unlit,
}

/// An op that was recorded but not executed (dry-run or unlit).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannedOp {
    pub seq: u64,
    pub class: BurnClass,
    pub reversible: bool,
    /// An irreversible op a compensation covers.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compensated: bool,
    /// The chain/step that owns the op, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub summary: String,
    pub op: Op,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Perform the effect; the kernel has journaled it.
    Execute,
    /// Do not touch the world; return a plausible result.
    Simulate,
}

/// Run-wide effect policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Execute burns, journal them.
    Run,
    /// Simulate every burn, produce a plan.
    DryRun,
}

pub struct Kernel {
    pub mode: Mode,
    journal: Journal,
    pub planned: Vec<PlannedOp>,
    seq: u64,
    pub executed: usize,
    pub irreversible: usize,
    /// The ghost filesystem: present in dry-run mode only.
    ghost: Option<ghost::Ghost>,
    /// The declared pack: every native write must stay inside these roots.
    pack: Option<Vec<PathBuf>>,
    /// Inside a `burn { } unburn { }` block: irreversible ops are compensated.
    compensating: u32,
    /// The chain/step path that owns the current ops, for the plan.
    owner: Vec<String>,
    /// Journal length at the start of each open `burn { } unburn { }`
    /// block, so its ops can be stamped `compensated` when it completes.
    compensating_marks: Vec<usize>,
}

/// What a hop changed, as observed by comparing the pack before and after.
#[derive(Debug, Default, Clone, Serialize)]
pub struct HopWrites {
    pub created: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
    /// Changes seen outside the pack (in the child's working directory).
    pub outside: Vec<PathBuf>,
}

/// A snapshot of mtimes and sizes, cheap enough to take around every hop.
pub struct PackWatch {
    inside: std::collections::BTreeMap<PathBuf, (u64, u64)>,
    outside: std::collections::BTreeMap<PathBuf, (u64, u64)>,
    outside_root: Option<PathBuf>,
}

/// Expand `~`, make absolute against the working directory, and normalise
/// lexically (no symlink resolution, no disk access).
pub fn normalize_root(raw: &str) -> PathBuf {
    let expanded = if let Some(rest) = raw.strip_prefix("~/") {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|| PathBuf::from(raw))
    } else if raw == "~" {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(raw))
    } else {
        PathBuf::from(raw)
    };
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(expanded)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Where a path really is: lexically normalised, then the nearest existing
/// ancestor canonicalised (symlinks resolved) with the rest appended. A
/// symlink inside the pack that points outside resolves outside.
pub fn scope_path(path: &Path) -> PathBuf {
    let abs = normalize_root(&path.to_string_lossy());
    let mut existing = abs.clone();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match existing.file_name() {
            Some(n) => {
                rest.push(n.to_os_string());
                if !existing.pop() {
                    break;
                }
            }
            None => break,
        }
    }
    let mut out = std::fs::canonicalize(&existing).unwrap_or(existing);
    for c in rest.iter().rev() {
        out.push(c);
    }
    out
}

/// Is `path` inside one of the roots, with symlinks resolved on both sides?
pub fn inside_roots(path: &Path, roots: &[PathBuf]) -> bool {
    let real = scope_path(path);
    roots.iter().any(|r| {
        let root = scope_path(r);
        real == root || real.starts_with(&root)
    })
}

/// What `scan` records for a directory in place of a file's (mtime, size):
/// directories are watched for being created, not for changing.
const DIR_MARK: (u64, u64) = (u64::MAX, u64::MAX);

fn scan(
    root: &Path,
    max_depth: usize,
    cap: usize,
) -> std::collections::BTreeMap<PathBuf, (u64, u64)> {
    let mut out = std::collections::BTreeMap::new();
    // Links are followed: a directory linked from inside the pack is part
    // of what a child can write through, and a loop is skipped by walkdir.
    for entry in walkdir::WalkDir::new(root)
        .min_depth(1)
        .max_depth(max_depth)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            n != ".git" && n != "node_modules" && n != "target" && n != ".cigscript"
        })
    {
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_dir() {
            out.insert(entry.path().to_path_buf(), DIR_MARK);
            if out.len() >= cap {
                break;
            }
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(m) = entry.metadata() else { continue };
        let mtime = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        out.insert(entry.path().to_path_buf(), (mtime, m.len()));
        if out.len() >= cap {
            break;
        }
    }
    out
}

impl Kernel {
    /// A kernel that journals into `run_dir`, or in memory only when `None`.
    pub fn new(mode: Mode, run_dir: Option<PathBuf>) -> Result<Self, Diagnostic> {
        Ok(Self {
            mode,
            journal: Journal::open(run_dir)
                .map_err(|e| burn_error(format!("could not open the run journal: {e}")))?,
            planned: Vec::new(),
            seq: 0,
            executed: 0,
            irreversible: 0,
            ghost: if mode == Mode::DryRun {
                Some(ghost::Ghost::new())
            } else {
                None
            },
            pack: None,
            compensating: 0,
            owner: Vec::new(),
            compensating_marks: Vec::new(),
        })
    }

    /// Declare the pack: the roots every native write must stay inside.
    pub fn set_pack(&mut self, roots: Vec<PathBuf>) {
        self.pack = Some(roots);
    }

    pub fn pack(&self) -> Option<&[PathBuf]> {
        self.pack.as_deref()
    }

    pub fn enter_compensating(&mut self) {
        self.compensating += 1;
        self.compensating_marks.push(self.journal.entries().len());
    }

    /// Leave the block; returns the journal mark where it began, for
    /// [`Kernel::record_compensation`].
    pub fn leave_compensating(&mut self) -> usize {
        self.compensating = self.compensating.saturating_sub(1);
        self.compensating_marks.pop().unwrap_or(0)
    }

    /// The op just journaled did not happen (its syscall refused it before
    /// changing anything): mark it undone so rollback and `cig unburn`
    /// skip it, and take it back out of the counts.
    pub fn op_failed(&mut self) {
        if let Some((reversible, compensated)) = self.journal.mark_last_failed() {
            self.executed = self.executed.saturating_sub(1);
            if !reversible && !compensated {
                self.irreversible = self.irreversible.saturating_sub(1);
            }
        }
    }

    pub fn push_owner(&mut self, owner: String) {
        self.owner.push(owner);
    }

    pub fn pop_owner(&mut self) {
        self.owner.pop();
    }

    fn owner_path(&self) -> Option<String> {
        if self.owner.is_empty() {
            None
        } else {
            Some(self.owner.join("/"))
        }
    }

    /// Record the compensation of a `burn { } unburn { }` block that
    /// completed, and only then stamp the block's irreversible ops
    /// `compensated`: a block that fails half-way, or a process that dies
    /// inside one, leaves them honestly irreversible. In a dry-run the
    /// compensation appears in the plan.
    pub fn record_compensation(
        &mut self,
        comp: journal::Compensation,
        mark: usize,
    ) -> Result<(), Diagnostic> {
        self.seq += 1;
        let op = Op::Compensate { line: comp.line };
        if self.mode == Mode::DryRun {
            self.planned.push(PlannedOp {
                seq: self.seq,
                class: BurnClass::Burn,
                reversible: true,
                compensated: false,
                owner: self.owner_path(),
                summary: op.describe(),
                op,
            });
            return Ok(());
        }
        self.journal
            .record_with(self.seq, op, Some(comp), false)
            .map_err(|e| {
                burn_error(format!("could not journal the compensation: {e}")).code("E702")
            })?;
        let stamped = self.journal.mark_compensated_since(mark);
        self.irreversible = self.irreversible.saturating_sub(stamped);
        Ok(())
    }

    /// Before a hop: remember the pack's files (and the child's working
    /// directory, when it is outside the pack) by mtime and size.
    pub fn pack_watch_begin(&self, cwd: Option<&Path>) -> Option<PackWatch> {
        let roots = self.pack.as_ref()?;
        if self.mode == Mode::DryRun {
            return None;
        }
        let mut inside = std::collections::BTreeMap::new();
        for r in roots {
            inside.extend(scan(r, 64, 50_000));
        }
        let work = cwd
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .map(|p| normalize_root(&p.to_string_lossy()));
        let outside_root = work.filter(|w| !inside_roots(w, roots));
        let outside = outside_root
            .as_ref()
            .map(|w| scan(w, 3, 5_000))
            .unwrap_or_default();
        Some(PackWatch {
            inside,
            outside,
            outside_root,
        })
    }

    /// After a hop: what changed. Files the hop created inside the pack are
    /// journaled as reversible (rollback removes them); modified or deleted
    /// ones as irreversible with the detail; changes outside the pack are
    /// returned for the caller to report (E752).
    pub fn pack_watch_end(&mut self, watch: PackWatch) -> HopWrites {
        let Some(roots) = self.pack.clone() else {
            return HopWrites::default();
        };
        let mut after = std::collections::BTreeMap::new();
        for r in &roots {
            after.extend(scan(r, 64, 50_000));
        }
        let mut writes = HopWrites::default();
        // A directory the hop created is journaled before the files in it
        // (the map is ordered), so rollback, newest first, empties it and
        // then removes it. A deleted directory is told by its files.
        for (p, meta) in &after {
            match watch.inside.get(p) {
                None => writes.created.push(p.clone()),
                Some(before) if before != meta => writes.modified.push(p.clone()),
                Some(_) => {}
            }
        }
        for (p, meta) in &watch.inside {
            if *meta != DIR_MARK && !after.contains_key(p) {
                writes.deleted.push(p.clone());
            }
        }
        if let Some(w) = &watch.outside_root {
            let now = scan(w, 3, 5_000);
            for (p, meta) in &now {
                if *meta == DIR_MARK || inside_roots(p, &roots) {
                    continue;
                }
                match watch.outside.get(p) {
                    None => writes.outside.push(p.clone()),
                    Some(before) if before != meta => writes.outside.push(p.clone()),
                    Some(_) => {}
                }
            }
            for (p, meta) in &watch.outside {
                if *meta != DIR_MARK && !now.contains_key(p) && !inside_roots(p, &roots) {
                    writes.outside.push(p.clone());
                }
            }
        }
        let compensated = self.compensating > 0;
        for p in &writes.created {
            self.seq += 1;
            self.executed += 1;
            let _ = self.journal.record_with(
                self.seq,
                Op::HopCreated { path: p.clone() },
                None,
                compensated,
            );
        }
        for (p, change) in writes
            .modified
            .iter()
            .map(|p| (p, "modified"))
            .chain(writes.deleted.iter().map(|p| (p, "deleted")))
        {
            self.seq += 1;
            self.executed += 1;
            if !compensated {
                self.irreversible += 1;
            }
            let _ = self.journal.record_with(
                self.seq,
                Op::HopChanged {
                    path: p.clone(),
                    change: change.to_string(),
                },
                None,
                compensated,
            );
        }
        writes
    }

    /// The ghost filesystem, when this is a dry-run.
    pub fn ghost(&self) -> Option<&ghost::Ghost> {
        self.ghost.as_ref()
    }

    /// Give a simulated write or append its bytes (dry-run only).
    pub fn ghost_put(&mut self, path: &Path, bytes: &[u8], append: bool) {
        let seq = self.seq;
        if let Some(g) = self.ghost.as_mut() {
            g.put(path, bytes, append, seq);
        }
    }

    pub fn ephemeral(mode: Mode) -> Self {
        Self::new(mode, None).expect("in-memory journal cannot fail")
    }

    pub fn run_dir(&self) -> Option<&Path> {
        self.journal.run_dir()
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// Ask permission to perform `op`. `unlit` is true inside `burn unlit`.
    pub fn decide(&mut self, op: Op, unlit: bool) -> Result<Decision, Diagnostic> {
        if let Some(roots) = &self.pack {
            let outside: Vec<&PathBuf> = match &op {
                Op::Write { path }
                | Op::Append { path }
                | Op::Delete { path }
                | Op::Mkdir { path } => vec![path],
                Op::Copy { to, .. } => vec![to],
                Op::Move { from, to } => vec![from, to],
                _ => Vec::new(),
            }
            .into_iter()
            .filter(|p| !inside_roots(p, roots))
            .collect();
            if let Some(p) = outside.first() {
                // A path that reads as inside but resolves outside is a
                // symlink escape; say where it really lands.
                let real = scope_path(p);
                let lexical = normalize_root(&p.to_string_lossy());
                let resolved = if real != lexical {
                    format!(", it resolves to {}", real.display())
                } else {
                    String::new()
                };
                return Err(burn_error(format!(
                    "{} is outside the pack ({}){resolved}",
                    p.display(),
                    roots
                        .iter()
                        .map(|r| r.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .code("E751")
                .with_subject(p.display().to_string())
                .with_hint("widen pack { } or fix the path; nothing was changed"));
            }
        }
        self.seq += 1;
        let simulate = unlit || self.mode == Mode::DryRun;
        let compensated = self.compensating > 0 && !op.reversible();
        if simulate {
            if !unlit {
                if let Some(g) = self.ghost.as_mut() {
                    g.apply(self.seq, &op);
                }
            }
            self.planned.push(PlannedOp {
                seq: self.seq,
                class: if unlit {
                    BurnClass::Unlit
                } else {
                    BurnClass::Burn
                },
                reversible: op.reversible(),
                compensated,
                owner: self.owner_path(),
                summary: op.describe(),
                op,
            });
            return Ok(Decision::Simulate);
        }
        // The journal entry is stamped `compensated` only when the block
        // completes and its compensation is recorded (see
        // `record_compensation`); until then the op is what it is.
        if !op.reversible() {
            self.irreversible += 1;
        }
        self.executed += 1;
        self.journal.record_with(self.seq, op, None, false).map_err(|e| {
            burn_error(format!("could not journal the burn: {e}"))
                .code("E702")
                .with_hint("free space or fix permissions under ~/.cigscript (CIGSCRIPT_HOME), then run again; nothing was changed")
        })?;
        Ok(Decision::Execute)
    }

    /// Undo every journaled op, newest first.
    pub fn rollback(&mut self) -> journal::RollbackReport {
        self.journal.rollback()
    }

    /// Undo every journaled op, running compensations in place.
    pub fn rollback_with(
        &mut self,
        runner: journal::CompensationRunner<'_>,
    ) -> journal::RollbackReport {
        self.journal.rollback_with(runner)
    }

    /// A position in the journal, for a retry to roll back to.
    pub fn mark(&self) -> usize {
        self.journal.mark()
    }

    pub fn irreversible_since(&self, mark: usize) -> usize {
        self.journal.irreversible_since(mark)
    }

    /// Undo what was journaled since `mark` (a failed attempt), so a retry
    /// runs against the rolled-back state.
    pub fn rollback_since(&mut self, mark: usize) -> journal::RollbackReport {
        self.journal.rollback_since(mark)
    }

    pub fn intents(&self) -> impl Iterator<Item = &PlannedOp> {
        self.planned.iter().filter(|p| p.class == BurnClass::Unlit)
    }

    pub fn dry_run_plan(&self) -> impl Iterator<Item = &PlannedOp> {
        self.planned.iter().filter(|p| p.class == BurnClass::Burn)
    }
}
