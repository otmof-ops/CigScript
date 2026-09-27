// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! Append-only journal of executed ops with before-state snapshots, and
//! the rollback that replays it in reverse.
//!
//! The journal is write-ahead: the entry and its snapshot are on disk, and
//! synced, before the effect happens. A process killed mid-burn leaves a
//! record `cig unburn` can replay. Symlinks are recorded as links (their
//! target), never as copies of what they point at.

use super::Op;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// What a path looked like before an op touched it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Before {
    Absent,
    File {
        snapshot: PathBuf,
        sha256: String,
        bytes: u64,
    },
    Dir {
        snapshot: PathBuf,
    },
    /// A symbolic link: restored as a link to the same target.
    Symlink {
        target: PathBuf,
    },
    /// The path was moved; rollback moves it back from `to`.
    Moved {
        to: PathBuf,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BeforeState {
    pub path: PathBuf,
    #[serde(flatten)]
    pub before: Before,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    pub at: String,
    pub reversible: bool,
    pub op: Op,
    pub before: Vec<BeforeState>,
    /// Rolled back already, by a retry inside the run; skipped by `unburn`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub undone: bool,
}

pub struct Journal {
    run_dir: Option<PathBuf>,
    file: Option<fs::File>,
    entries: Vec<Entry>,
}

#[derive(Debug, Default, Serialize)]
pub struct RollbackReport {
    pub restored: Vec<String>,
    pub irreversible: Vec<String>,
    pub failed: Vec<String>,
}

impl RollbackReport {
    pub fn clean(&self) -> bool {
        self.failed.is_empty()
    }
}

impl Journal {
    pub fn open(run_dir: Option<PathBuf>) -> Result<Self, io::Error> {
        let file = match &run_dir {
            Some(dir) => {
                fs::create_dir_all(dir.join("snapshots"))?;
                let f = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("journal.jsonl"))?;
                sync_dir(dir)?;
                Some(f)
            }
            None => None,
        };
        Ok(Self {
            run_dir,
            file,
            entries: Vec::new(),
        })
    }

    /// Load a finished run's journal for `cig unburn`.
    pub fn load(run_dir: &Path) -> Result<Self, io::Error> {
        let text = fs::read_to_string(run_dir.join("journal.jsonl"))?;
        let mut entries: Vec<Entry> = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let v: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            // `{"seq": N, "undone": true}` marks an earlier entry as rolled
            // back by a retry inside the run.
            if v.get("undone").and_then(|u| u.as_bool()) == Some(true) && v.get("op").is_none() {
                if let Some(seq) = v.get("seq").and_then(|n| n.as_u64()) {
                    if let Some(e) = entries.iter_mut().find(|e| e.seq == seq) {
                        e.undone = true;
                    }
                }
                continue;
            }
            let e: Entry = serde_json::from_value(v)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            entries.push(e);
        }
        Ok(Self {
            run_dir: Some(run_dir.to_path_buf()),
            file: None,
            entries,
        })
    }

    pub fn run_dir(&self) -> Option<&Path> {
        self.run_dir.as_deref()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Entries still standing (not undone by a retry).
    pub fn len(&self) -> usize {
        self.entries.iter().filter(|e| !e.undone).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every entry, undone ones included: a position for `rollback_since`.
    pub fn mark(&self) -> usize {
        self.entries.len()
    }

    /// Irreversible entries recorded since `mark`.
    pub fn irreversible_since(&self, mark: usize) -> usize {
        self.entries
            .iter()
            .skip(mark)
            .filter(|e| !e.reversible && !e.undone)
            .count()
    }

    /// Roll back the entries recorded since `mark`, newest first, and mark
    /// them undone in the journal so a later `unburn` skips them. The op
    /// numbers stay; the retry's own entries follow.
    pub fn rollback_since(&mut self, mark: usize) -> RollbackReport {
        let report = self.rollback_from(mark);
        let mut markers = String::new();
        for e in self.entries.iter_mut().skip(mark) {
            if !e.undone {
                e.undone = true;
                markers.push_str(&format!("{{\"seq\":{},\"undone\":true}}\n", e.seq));
            }
        }
        if let Some(f) = &mut self.file {
            let _ = f.write_all(markers.as_bytes());
            let _ = f.flush();
            let _ = f.sync_data();
        }
        report
    }

    /// Snapshot everything `op` will change, sync it, then append and sync
    /// the entry. Only after both are durable does the effect happen.
    pub fn record(&mut self, seq: u64, op: Op) -> Result<(), io::Error> {
        let before = self.capture_before(seq, &op)?;
        let entry = Entry {
            seq,
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            reversible: op.reversible(),
            op,
            before,
            undone: false,
        };
        if let Some(f) = &mut self.file {
            let mut line = serde_json::to_string(&entry)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            line.push('\n');
            f.write_all(line.as_bytes())?;
            f.flush()?;
            f.sync_data()?;
        }
        self.entries.push(entry);
        Ok(())
    }

    /// Before-states are listed outermost-first (a created parent, then the
    /// path itself); rollback applies them innermost-first.
    fn capture_before(&self, seq: u64, op: &Op) -> Result<Vec<BeforeState>, io::Error> {
        Ok(match op {
            Op::Write { path } | Op::Append { path } => self.destination_states(seq, path, 0)?,
            Op::Delete { path } => vec![self.snapshot(seq, path, 0)?],
            Op::Mkdir { path } => {
                // `mkdir -p` may create several levels; remember the highest
                // one that is about to appear so rollback removes all of them.
                let created_root = first_missing_ancestor(path);
                vec![self.snapshot(seq, &created_root, 0)?]
            }
            Op::Copy { to, .. } => self.destination_states(seq, to, 0)?,
            Op::Move { from, to } => {
                let mut states = vec![BeforeState {
                    path: from.clone(),
                    before: Before::Moved { to: to.clone() },
                }];
                states.extend(self.destination_states(seq, to, 1)?);
                states
            }
            Op::Proc { .. } | Op::EnvSet { .. } | Op::EnvUnset { .. } => Vec::new(),
        })
    }

    /// Before-states for a path that is about to receive bytes: the highest
    /// directory that will be created, the path itself, and, when the path
    /// is a symlink, the place the bytes actually land.
    fn destination_states(
        &self,
        seq: u64,
        path: &Path,
        slot: usize,
    ) -> Result<Vec<BeforeState>, io::Error> {
        let mut states = Vec::new();
        let created_root = first_missing_ancestor(path);
        if created_root != *path {
            states.push(self.snapshot(seq, &created_root, slot)?);
        }
        states.push(self.snapshot(seq, path, slot + 1)?);
        if is_symlink(path) {
            let landing = resolve_symlink_chain(path);
            if landing != *path {
                states.push(self.snapshot(seq, &landing, slot + 2)?);
            }
        }
        Ok(states)
    }

    fn snapshot(&self, seq: u64, path: &Path, slot: usize) -> Result<BeforeState, io::Error> {
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(BeforeState {
                    path: path.to_path_buf(),
                    before: Before::Absent,
                })
            }
            Err(e) => return Err(e),
        };
        if meta.file_type().is_symlink() {
            // A link is recorded as a link, whatever it points at, and
            // whether or not the target exists.
            return Ok(BeforeState {
                path: path.to_path_buf(),
                before: Before::Symlink {
                    target: fs::read_link(path)?,
                },
            });
        }
        let Some(dir) = &self.run_dir else {
            // In-memory journal (eval/repl): remember only that it existed.
            return Ok(BeforeState {
                path: path.to_path_buf(),
                before: if meta.is_dir() {
                    Before::Dir {
                        snapshot: PathBuf::new(),
                    }
                } else {
                    Before::File {
                        snapshot: PathBuf::new(),
                        sha256: String::new(),
                        bytes: meta.len(),
                    }
                },
            });
        };
        let base = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "root".to_string());
        let snap = dir
            .join("snapshots")
            .join(format!("{seq:05}-{slot}-{base}"));
        if meta.is_dir() {
            copy_tree_durable(path, &snap)?;
            sync_dir(&dir.join("snapshots"))?;
            Ok(BeforeState {
                path: path.to_path_buf(),
                before: Before::Dir { snapshot: snap },
            })
        } else {
            fs::copy(path, &snap)?;
            fs::File::open(&snap)?.sync_all()?;
            sync_dir(&dir.join("snapshots"))?;
            let sha256 = sha256_file(&snap)?;
            Ok(BeforeState {
                path: path.to_path_buf(),
                before: Before::File {
                    snapshot: snap,
                    sha256,
                    bytes: meta.len(),
                },
            })
        }
    }

    /// Everything a rollback would need, checked before anything is touched.
    /// Returns one line per problem; empty means safe to proceed.
    pub fn verify(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (idx, entry) in self.entries.iter().enumerate() {
            if !entry.reversible || entry.undone {
                continue;
            }
            // Rollback runs newest-first, so a later entry's before-state
            // for a path puts it back before this entry needs it.
            let later_restores = |path: &Path| {
                self.entries[idx + 1..]
                    .iter()
                    .any(|e| e.reversible && !e.undone && e.before.iter().any(|b| b.path == *path))
            };
            for state in &entry.before {
                match &state.before {
                    Before::File {
                        snapshot, sha256, ..
                    } => {
                        if snapshot.as_os_str().is_empty() {
                            problems.push(format!(
                                "op {}: no snapshot was taken for {} (in-memory journal)",
                                entry.seq,
                                state.path.display()
                            ));
                        } else if !snapshot.is_file() {
                            problems.push(format!(
                                "op {}: snapshot missing for {} (expected {})",
                                entry.seq,
                                state.path.display(),
                                snapshot.display()
                            ));
                        } else if !sha256.is_empty() {
                            match sha256_file(snapshot) {
                                Ok(actual) if &actual == sha256 => {}
                                Ok(actual) => problems.push(format!(
                                    "op {}: snapshot for {} is corrupt (sha256 {} recorded, {} on disk)",
                                    entry.seq,
                                    state.path.display(),
                                    &sha256[..12],
                                    &actual[..12]
                                )),
                                Err(e) => problems.push(format!(
                                    "op {}: cannot read snapshot for {}: {e}",
                                    entry.seq,
                                    state.path.display()
                                )),
                            }
                        }
                    }
                    Before::Dir { snapshot } => {
                        if snapshot.as_os_str().is_empty() || !snapshot.is_dir() {
                            problems.push(format!(
                                "op {}: directory snapshot missing for {}",
                                entry.seq,
                                state.path.display()
                            ));
                        }
                    }
                    Before::Moved { to } => {
                        if fs::symlink_metadata(to).is_err() && !later_restores(to) {
                            problems.push(format!(
                                "op {}: {} is no longer at {} to move back",
                                entry.seq,
                                state.path.display(),
                                to.display()
                            ));
                        }
                    }
                    Before::Absent | Before::Symlink { .. } => {}
                }
            }
        }
        problems
    }

    /// Restore every before-state, newest entry first. Call `verify` first;
    /// this does what it can and reports what it could not.
    pub fn rollback(&mut self) -> RollbackReport {
        self.rollback_from(0)
    }

    fn rollback_from(&mut self, from: usize) -> RollbackReport {
        let mut report = RollbackReport::default();
        for entry in self.entries.iter().skip(from).rev() {
            if entry.undone {
                continue;
            }
            if !entry.reversible {
                report.irreversible.push(entry.op.describe());
                continue;
            }
            // Before-states are captured outermost-first (created parent, then
            // the path itself) and restored innermost-first, except that a
            // move is always moved back before anything else.
            let mut order: Vec<&BeforeState> = entry.before.iter().collect();
            order.reverse();
            if let Some(pos) = order
                .iter()
                .position(|s| matches!(s.before, Before::Moved { .. }))
            {
                let moved = order.remove(pos);
                order.insert(0, moved);
            }
            for state in order {
                match restore(state) {
                    Ok(()) => report.restored.push(format!(
                        "{} ({})",
                        entry.op.describe(),
                        match state.before {
                            Before::Absent => "removed",
                            Before::File { .. } => "file restored",
                            Before::Dir { .. } => "directory restored",
                            Before::Symlink { .. } => "link restored",
                            Before::Moved { .. } => "moved back",
                        }
                    )),
                    Err(e) => report.failed.push(format!(
                        "{}: {} ({e})",
                        entry.op.describe(),
                        state.path.display()
                    )),
                }
            }
        }
        report
    }
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// Follow a chain of symlinks without requiring the end to exist: where a
/// write through `path` would land. Stops at 40 hops or a loop.
pub(crate) fn resolve_symlink_chain(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        if !is_symlink(&current) {
            return current;
        }
        let Ok(target) = fs::read_link(&current) else {
            return current;
        };
        let next = if target.is_absolute() {
            target
        } else {
            current.parent().map(|d| d.join(&target)).unwrap_or(target)
        };
        if next == current {
            return current;
        }
        current = next;
    }
    current
}

// ----- after-state: what the run left, so unburn can tell what changed since -----

/// What a path looks like at one moment, in enough detail to notice a change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PathState {
    Absent,
    File { sha256: String, bytes: u64 },
    Dir,
    Symlink { target: PathBuf },
}

impl PathState {
    pub fn observe(path: &Path) -> Self {
        match fs::symlink_metadata(path) {
            Err(_) => PathState::Absent,
            Ok(m) if m.file_type().is_symlink() => PathState::Symlink {
                target: fs::read_link(path).unwrap_or_default(),
            },
            Ok(m) if m.is_dir() => PathState::Dir,
            Ok(m) => PathState::File {
                sha256: sha256_file(path).unwrap_or_default(),
                bytes: m.len(),
            },
        }
    }

    pub fn describe(&self) -> String {
        match self {
            PathState::Absent => "absent".to_string(),
            PathState::File { sha256, bytes } => {
                format!(
                    "a file, {bytes} bytes, sha256 {}",
                    &sha256[..sha256.len().min(12)]
                )
            }
            PathState::Dir => "a directory".to_string(),
            PathState::Symlink { target } => format!("a symlink to {}", target.display()),
        }
    }
}

/// `after.json`: every path the run touched, as the run left it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AfterState {
    pub recorded: String,
    pub paths: std::collections::BTreeMap<PathBuf, PathState>,
}

/// A path that is no longer as the run left it.
#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub path: PathBuf,
    /// The first journal entry that touched the path.
    pub seq: u64,
    pub was: PathState,
    pub now: PathState,
}

/// What `changed_since` knows: nothing at all for a run that predates
/// after-state records, or the list of changes (possibly empty).
#[derive(Clone, Debug, Serialize)]
pub struct ChangedSince {
    pub known: bool,
    pub changes: Vec<Change>,
}

impl Journal {
    /// Every path a rollback would touch, with the entry that first names it.
    pub fn touched_paths(&self) -> Vec<(PathBuf, u64)> {
        let mut seen: Vec<(PathBuf, u64)> = Vec::new();
        for e in &self.entries {
            if !e.reversible || e.undone {
                continue;
            }
            for b in &e.before {
                if !seen.iter().any(|(p, _)| p == &b.path) {
                    seen.push((b.path.clone(), e.seq));
                }
                if let Before::Moved { to } = &b.before {
                    if !seen.iter().any(|(p, _)| p == to) {
                        seen.push((to.clone(), e.seq));
                    }
                }
            }
        }
        seen
    }

    /// Record what the run left behind, once its effects are complete.
    pub fn write_after_state(&self) -> Result<(), io::Error> {
        let Some(dir) = &self.run_dir else {
            return Ok(());
        };
        let mut after = AfterState {
            recorded: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            paths: Default::default(),
        };
        for (path, _) in self.touched_paths() {
            after.paths.insert(path.clone(), PathState::observe(&path));
        }
        let text = serde_json::to_string_pretty(&after)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(dir.join("after.json"), text)?;
        Ok(())
    }

    pub fn after_state(&self) -> Option<AfterState> {
        let dir = self.run_dir.as_ref()?;
        let text = fs::read_to_string(dir.join("after.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Which touched paths are no longer as the run left them. Restoring
    /// over one of these would overwrite work done since.
    pub fn changed_since(&self) -> ChangedSince {
        let Some(after) = self.after_state() else {
            return ChangedSince {
                known: false,
                changes: Vec::new(),
            };
        };
        let mut changes = Vec::new();
        for (path, seq) in self.touched_paths() {
            let Some(was) = after.paths.get(&path) else {
                continue;
            };
            let now = PathState::observe(&path);
            if &now != was {
                changes.push(Change {
                    path,
                    seq,
                    was: was.clone(),
                    now,
                });
            }
        }
        ChangedSince {
            known: true,
            changes,
        }
    }
}

/// The topmost path component that does not exist yet (or `path` itself).
fn first_missing_ancestor(path: &Path) -> PathBuf {
    let mut candidate = path.to_path_buf();
    for ancestor in path.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() || fs::symlink_metadata(ancestor).is_ok() {
            break;
        }
        candidate = ancestor.to_path_buf();
    }
    candidate
}

fn restore(state: &BeforeState) -> Result<(), io::Error> {
    let path = &state.path;
    match &state.before {
        Before::Absent => remove_any(path),
        Before::File { snapshot, .. } => {
            if snapshot.as_os_str().is_empty() {
                return Err(io::Error::other(
                    "no snapshot was taken (in-memory journal)",
                ));
            }
            if !snapshot.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("snapshot {} is missing", snapshot.display()),
                ));
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            // Stage beside the destination, then swap in, so a failure
            // half-way leaves whatever is there untouched.
            let staged = staging_path(path);
            fs::copy(snapshot, &staged)?;
            if fs::symlink_metadata(path)
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                remove_any(path)?;
            }
            fs::rename(&staged, path)
        }
        Before::Dir { snapshot } => {
            if snapshot.as_os_str().is_empty() {
                return Err(io::Error::other(
                    "no snapshot was taken (in-memory journal)",
                ));
            }
            if !snapshot.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("snapshot {} is missing", snapshot.display()),
                ));
            }
            let staged = staging_path(path);
            copy_tree(snapshot, &staged)?;
            remove_any(path)?;
            fs::rename(&staged, path)
        }
        Before::Symlink { target } => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            remove_any(path)?;
            make_symlink(target, path)
        }
        Before::Moved { to } => {
            if fs::symlink_metadata(to).is_err() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{} is no longer there to move back", to.display()),
                ));
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            remove_any(path)?;
            fs::rename(to, path)
        }
    }
}

/// A sibling path to stage a restore in before swapping it into place.
fn staging_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "root".to_string());
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut n = 0u32;
    loop {
        let candidate = parent.join(format!(".{name}.cig-restore-{}-{n}", std::process::id()));
        if fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
        n += 1;
    }
}

/// Remove a file, a directory tree, or a link (never following the link).
fn remove_any(path: &Path) -> Result<(), io::Error> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => fs::remove_file(path),
        Ok(m) if m.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn make_symlink(target: &Path, at: &Path) -> Result<(), io::Error> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, at)
    }
    #[cfg(windows)]
    {
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, at)
        } else {
            std::os::windows::fs::symlink_file(target, at)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, at);
        Err(io::Error::other(
            "symlinks are not supported on this platform",
        ))
    }
}

/// Copy a tree, preserving links as links. Directory permissions are applied
/// after the walk so a read-only directory still receives its children.
pub fn copy_tree(from: &Path, to: &Path) -> Result<(), io::Error> {
    copy_tree_inner(from, to, false)
}

/// `copy_tree` for a snapshot: every copied file is synced to disk and the
/// directories fsynced, so the journal entry that follows never points at
/// bytes that were still in the page cache when the power went.
fn copy_tree_durable(from: &Path, to: &Path) -> Result<(), io::Error> {
    copy_tree_inner(from, to, true)
}

fn copy_tree_inner(from: &Path, to: &Path, durable: bool) -> Result<(), io::Error> {
    fs::create_dir_all(to)?;
    let mut dir_modes: Vec<(PathBuf, fs::Permissions)> = Vec::new();
    for entry in walkdir::WalkDir::new(from)
        .min_depth(1)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry.map_err(io::Error::other)?;
        let rel = entry
            .path()
            .strip_prefix(from)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let dest = to.join(rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        if entry.file_type().is_symlink() {
            let target = fs::read_link(entry.path())?;
            make_symlink(&target, &dest)?;
        } else if entry.file_type().is_dir() {
            fs::create_dir_all(&dest)?;
            let meta = entry.metadata().map_err(io::Error::other)?;
            dir_modes.push((dest, meta.permissions()));
        } else {
            fs::copy(entry.path(), &dest)?;
            if durable {
                fs::File::open(&dest)?.sync_all()?;
            }
        }
    }
    for (dir, perms) in dir_modes.into_iter().rev() {
        fs::set_permissions(&dir, perms)?;
    }
    if durable {
        sync_dir(to)?;
    }
    Ok(())
}

/// fsync a directory so newly created entries in it are durable. A no-op
/// where directories cannot be opened for syncing.
fn sync_dir(dir: &Path) -> Result<(), io::Error> {
    #[cfg(unix)]
    {
        match fs::File::open(dir) {
            Ok(f) => f.sync_all(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

pub fn sha256_file(path: &Path) -> Result<String, io::Error> {
    use sha2::{Digest, Sha256};
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn write_delete_move_roll_back() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let run_dir = root.join("run");
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        let c = root.join("c.txt");
        fs::write(&a, "original").unwrap();
        fs::write(&b, "to be deleted").unwrap();

        let mut j = Journal::open(Some(run_dir.clone())).unwrap();
        j.record(1, Op::Write { path: a.clone() }).unwrap();
        fs::write(&a, "changed").unwrap();
        j.record(2, Op::Delete { path: b.clone() }).unwrap();
        fs::remove_file(&b).unwrap();
        j.record(
            3,
            Op::Move {
                from: a.clone(),
                to: c.clone(),
            },
        )
        .unwrap();
        fs::rename(&a, &c).unwrap();
        j.record(
            4,
            Op::Write {
                path: root.join("new.txt"),
            },
        )
        .unwrap();
        fs::write(root.join("new.txt"), "brand new").unwrap();
        j.record(
            5,
            Op::Proc {
                cmd: "true".into(),
                args: vec![],
                cwd: None,
            },
        )
        .unwrap();

        assert!(!a.exists() && c.exists() && !b.exists());

        // Reload from disk to prove the journal round-trips.
        let mut loaded = Journal::load(&run_dir).unwrap();
        assert_eq!(loaded.len(), 5);
        assert!(loaded.verify().is_empty());
        let report = loaded.rollback();
        assert!(report.clean(), "{report:?}");
        assert_eq!(report.irreversible.len(), 1);
        assert_eq!(fs::read_to_string(&a).unwrap(), "original");
        assert_eq!(fs::read_to_string(&b).unwrap(), "to be deleted");
        assert!(!c.exists());
        assert!(!root.join("new.txt").exists());
    }

    #[test]
    fn directory_delete_rolls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let d = root.join("dir");
        fs::create_dir_all(d.join("nested")).unwrap();
        fs::write(d.join("nested/f.txt"), "deep").unwrap();
        let mut j = Journal::open(Some(root.join("run"))).unwrap();
        j.record(1, Op::Delete { path: d.clone() }).unwrap();
        fs::remove_dir_all(&d).unwrap();
        let report = j.rollback();
        assert!(report.clean(), "{report:?}");
        assert_eq!(fs::read_to_string(d.join("nested/f.txt")).unwrap(), "deep");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_restore_is_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("target.txt"), "t").unwrap();
        let live = root.join("live.lnk");
        let dangling = root.join("dangling.lnk");
        std::os::unix::fs::symlink("target.txt", &live).unwrap();
        std::os::unix::fs::symlink("nowhere/at/all", &dangling).unwrap();
        let mut j = Journal::open(Some(root.join("run"))).unwrap();
        j.record(1, Op::Delete { path: live.clone() }).unwrap();
        fs::remove_file(&live).unwrap();
        j.record(
            2,
            Op::Delete {
                path: dangling.clone(),
            },
        )
        .unwrap();
        fs::remove_file(&dangling).unwrap();
        // Overwriting a link with a file, then rolling back, gives the link back.
        j.record(
            3,
            Op::Write {
                path: root.join("target.txt"),
            },
        )
        .unwrap();
        fs::write(root.join("target.txt"), "changed").unwrap();
        assert!(j.verify().is_empty());
        let report = j.rollback();
        assert!(report.clean(), "{report:?}");
        assert!(fs::symlink_metadata(&live)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&live).unwrap(), PathBuf::from("target.txt"));
        assert!(fs::symlink_metadata(&dangling)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_link(&dangling).unwrap(),
            PathBuf::from("nowhere/at/all")
        );
        assert_eq!(fs::read_to_string(root.join("target.txt")).unwrap(), "t");
    }

    #[test]
    fn changed_since_sees_edits_after_the_run() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let run_dir = root.join("run");
        fs::write(root.join("f.txt"), "one").unwrap();
        let mut j = Journal::open(Some(run_dir.clone())).unwrap();
        j.record(
            1,
            Op::Write {
                path: root.join("f.txt"),
            },
        )
        .unwrap();
        fs::write(root.join("f.txt"), "two").unwrap();
        j.record(
            2,
            Op::Write {
                path: root.join("g.txt"),
            },
        )
        .unwrap();
        fs::write(root.join("g.txt"), "new").unwrap();
        j.write_after_state().unwrap();
        let loaded = Journal::load(&run_dir).unwrap();
        let c = loaded.changed_since();
        assert!(c.known && c.changes.is_empty(), "{c:?}");
        fs::write(root.join("f.txt"), "three").unwrap();
        fs::remove_file(root.join("g.txt")).unwrap();
        let c = loaded.changed_since();
        assert_eq!(c.changes.len(), 2, "{c:?}");
        assert!(matches!(c.changes[1].now, PathState::Absent));
        // A run without after.json knows nothing, and says so.
        fs::remove_file(run_dir.join("after.json")).unwrap();
        assert!(!loaded.changed_since().known);
    }

    #[test]
    fn verify_refuses_before_a_partial_restore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let run_dir = root.join("run");
        fs::write(root.join("m1.txt"), "old1").unwrap();
        fs::write(root.join("m2.txt"), "old2").unwrap();
        let mut j = Journal::open(Some(run_dir.clone())).unwrap();
        j.record(
            1,
            Op::Write {
                path: root.join("m1.txt"),
            },
        )
        .unwrap();
        fs::write(root.join("m1.txt"), "one").unwrap();
        j.record(
            2,
            Op::Write {
                path: root.join("m2.txt"),
            },
        )
        .unwrap();
        fs::write(root.join("m2.txt"), "two").unwrap();
        // Lose the second snapshot.
        for e in fs::read_dir(run_dir.join("snapshots")).unwrap().flatten() {
            if e.file_name().to_string_lossy().starts_with("00002") {
                fs::remove_file(e.path()).unwrap();
            }
        }
        let loaded = Journal::load(&run_dir).unwrap();
        let problems = loaded.verify();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("snapshot missing for"), "{problems:?}");
        // Nothing was touched by verifying.
        assert_eq!(fs::read_to_string(root.join("m1.txt")).unwrap(), "one");
    }

    // ----- the property test: random ops over random trees ------------------

    /// A tiny deterministic PRNG so the test needs no dependency and every
    /// failure is reproducible from its seed.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
        fn chance(&mut self, one_in: usize) -> bool {
            self.below(one_in) == 0
        }
    }

    /// Byte-for-byte fingerprint of a tree: kind, mode, content or link target.
    fn fingerprint(root: &Path) -> BTreeMap<PathBuf, String> {
        let mut out = BTreeMap::new();
        for entry in walkdir::WalkDir::new(root)
            .min_depth(1)
            .follow_links(false)
            .sort_by_file_name()
        {
            let entry = entry.unwrap();
            let rel = entry.path().strip_prefix(root).unwrap().to_path_buf();
            let meta = fs::symlink_metadata(entry.path()).unwrap();
            let desc = if meta.file_type().is_symlink() {
                format!("link -> {}", fs::read_link(entry.path()).unwrap().display())
            } else if meta.is_dir() {
                "dir".to_string()
            } else {
                #[cfg(unix)]
                let mode = {
                    use std::os::unix::fs::PermissionsExt;
                    meta.permissions().mode() & 0o777
                };
                #[cfg(not(unix))]
                let mode = 0;
                format!("file mode={mode:o} {}", sha256_file(entry.path()).unwrap())
            };
            out.insert(rel, desc);
        }
        out
    }

    fn random_tree(rng: &mut Rng, root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let dirs = ["", "a", "a/b", "c"];
        for d in dirs {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        for i in 0..(2 + rng.below(6)) {
            let d = dirs[rng.below(dirs.len())];
            let p = root.join(d).join(format!("f{i}.txt"));
            fs::write(&p, format!("content {i} {}", rng.next())).unwrap();
            #[cfg(unix)]
            if rng.chance(3) {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
            }
            files.push(p);
        }
        #[cfg(unix)]
        for i in 0..rng.below(3) {
            let l = root.join(format!("l{i}.lnk"));
            let target = match rng.below(3) {
                0 if !files.is_empty() => files[rng.below(files.len())]
                    .file_name()
                    .unwrap()
                    .to_os_string(),
                1 => format!("ghost{i}.txt").into(), // dangling, but a write lands
                _ => "missing/target".into(),        // dangling, nowhere to land
            };
            std::os::unix::fs::symlink(target, &l).unwrap();
            files.push(l);
        }
        files
    }

    /// Would `fs::write` succeed here? A regular file, an absent path whose
    /// parent exists, or a link whose (possibly absent) target has a parent.
    fn can_receive_bytes(path: &Path) -> bool {
        match fs::metadata(path) {
            Ok(m) => m.is_file(),
            Err(_) => {
                let landing = resolve_symlink_chain(path);
                landing.parent().map(|d| d.is_dir()).unwrap_or(false)
            }
        }
    }

    fn apply(op: &Op) -> io::Result<()> {
        match op {
            Op::Write { path } => {
                if let Some(p) = path.parent() {
                    fs::create_dir_all(p)?;
                }
                fs::write(path, "written")
            }
            Op::Append { path } => {
                let mut f = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?;
                f.write_all(b"+appended")
            }
            Op::Delete { path } => remove_any(path),
            Op::Move { from, to } => {
                if let Some(p) = to.parent() {
                    fs::create_dir_all(p)?;
                }
                remove_any(to)?;
                fs::rename(from, to)
            }
            Op::Copy { from, to } => {
                if let Some(p) = to.parent() {
                    fs::create_dir_all(p)?;
                }
                if fs::metadata(from)?.is_dir() {
                    copy_tree(from, to)
                } else {
                    fs::copy(from, to).map(|_| ())
                }
            }
            Op::Mkdir { path } => fs::create_dir_all(path),
            _ => Ok(()),
        }
    }

    #[test]
    fn rollback_property_over_random_trees() {
        for seed in 1..=60u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("tree");
            fs::create_dir_all(&root).unwrap();
            let mut paths = random_tree(&mut rng, &root);
            let before = fingerprint(&root);
            let run_dir = tmp.path().join("run");
            let mut j = Journal::open(Some(run_dir.clone())).unwrap();
            let steps = 1 + rng.below(8);
            for seq in 1..=steps as u64 {
                let fresh = root.join(format!("n{}.txt", rng.next() % 1000));
                let pick =
                    |rng: &mut Rng, paths: &Vec<PathBuf>| paths[rng.below(paths.len())].clone();
                let op = match rng.below(7) {
                    0 => Op::Write {
                        path: pick(&mut rng, &paths),
                    },
                    1 => Op::Write {
                        path: fresh.clone(),
                    },
                    2 => Op::Append {
                        path: pick(&mut rng, &paths),
                    },
                    3 => Op::Delete {
                        path: pick(&mut rng, &paths),
                    },
                    4 => Op::Move {
                        from: pick(&mut rng, &paths),
                        to: if rng.chance(2) {
                            fresh.clone()
                        } else {
                            pick(&mut rng, &paths)
                        },
                    },
                    5 => Op::Copy {
                        from: pick(&mut rng, &paths),
                        to: fresh.clone(),
                    },
                    _ => Op::Mkdir {
                        path: root.join(format!("d{}/deep", rng.next() % 100)),
                    },
                };
                // Ops the interpreter itself would refuse before journaling
                // (a missing source, a write that cannot land) are skipped.
                let source_ok = match &op {
                    Op::Delete { path } => fs::symlink_metadata(path).is_ok(),
                    Op::Write { path } => can_receive_bytes(path),
                    Op::Append { path } => fs::metadata(path).map(|m| m.is_file()).unwrap_or(false),
                    Op::Move { from, to } => fs::symlink_metadata(from).is_ok() && from != to,
                    Op::Copy { from, .. } => {
                        fs::metadata(from).map(|m| m.is_file()).unwrap_or(false)
                    }
                    _ => true,
                };
                if !source_ok {
                    continue;
                }
                j.record(seq, op.clone())
                    .unwrap_or_else(|e| panic!("seed {seed} seq {seq} record {op:?}: {e}"));
                apply(&op).unwrap_or_else(|e| panic!("seed {seed} seq {seq} apply {op:?}: {e}"));
                if let Op::Write { path } | Op::Copy { to: path, .. } | Op::Move { to: path, .. } =
                    &op
                {
                    if !paths.contains(path) {
                        paths.push(path.clone());
                    }
                }
            }
            let mut loaded = Journal::load(&run_dir).unwrap();
            assert!(
                loaded.verify().is_empty(),
                "seed {seed}: {:?}",
                loaded.verify()
            );
            let report = loaded.rollback();
            assert!(report.clean(), "seed {seed}: {report:?}");
            let after = fingerprint(&root);
            assert_eq!(before, after, "seed {seed}: tree differs after rollback");
        }
    }
}
