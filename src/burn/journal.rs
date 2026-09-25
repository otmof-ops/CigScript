// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! Append-only journal of executed ops with before-state snapshots, and
//! the rollback that replays it in reverse.

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
                Some(
                    fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(dir.join("journal.jsonl"))?,
                )
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
        let mut entries = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let e: Entry = serde_json::from_str(line)
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

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Snapshot everything `op` will change, then append the entry.
    pub fn record(&mut self, seq: u64, op: Op) -> Result<(), io::Error> {
        let before = self.capture_before(seq, &op)?;
        let entry = Entry {
            seq,
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            reversible: op.reversible(),
            op,
            before,
        };
        if let Some(f) = &mut self.file {
            let mut line = serde_json::to_string(&entry)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            line.push('\n');
            f.write_all(line.as_bytes())?;
            f.flush()?;
        }
        self.entries.push(entry);
        Ok(())
    }

    /// Before-states are listed in the order rollback must apply them.
    fn capture_before(&self, seq: u64, op: &Op) -> Result<Vec<BeforeState>, io::Error> {
        Ok(match op {
            Op::Write { path } | Op::Append { path } => {
                let mut states = Vec::new();
                let created_root = first_missing_ancestor(path);
                if created_root != *path {
                    states.push(self.snapshot(seq, &created_root, 0)?);
                }
                states.push(self.snapshot(seq, path, 1)?);
                states
            }
            Op::Delete { path } => vec![self.snapshot(seq, path, 0)?],
            Op::Mkdir { path } => {
                // `mkdir -p` may create several levels; remember the highest
                // one that is about to appear so rollback removes all of them.
                let created_root = first_missing_ancestor(path);
                vec![self.snapshot(seq, &created_root, 0)?]
            }
            Op::Copy { to, .. } => {
                let mut states = Vec::new();
                let created_root = first_missing_ancestor(to);
                if created_root != *to {
                    states.push(self.snapshot(seq, &created_root, 0)?);
                }
                states.push(self.snapshot(seq, to, 1)?);
                states
            }
            Op::Move { from, to } => {
                let mut states = vec![BeforeState {
                    path: from.clone(),
                    before: Before::Moved { to: to.clone() },
                }];
                let created_root = first_missing_ancestor(to);
                if created_root != *to {
                    states.push(self.snapshot(seq, &created_root, 1)?);
                }
                states.push(self.snapshot(seq, to, 2)?);
                states
            }
            Op::Proc { .. } | Op::EnvSet { .. } | Op::EnvUnset { .. } => Vec::new(),
        })
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
            copy_tree(path, &snap)?;
            Ok(BeforeState {
                path: path.to_path_buf(),
                before: Before::Dir { snapshot: snap },
            })
        } else {
            fs::copy(path, &snap)?;
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

    /// Restore every before-state, newest entry first.
    pub fn rollback(&mut self) -> RollbackReport {
        let mut report = RollbackReport::default();
        for entry in self.entries.iter().rev() {
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
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            if path.is_dir() {
                fs::remove_dir_all(path)?;
            }
            fs::copy(snapshot, path).map(|_| ())
        }
        Before::Dir { snapshot } => {
            if snapshot.as_os_str().is_empty() {
                return Err(io::Error::other(
                    "no snapshot was taken (in-memory journal)",
                ));
            }
            remove_any(path)?;
            copy_tree(snapshot, path)
        }
        Before::Moved { to } => {
            if !to.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{} is no longer there to move back", to.display()),
                ));
            }
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(to, path)
        }
    }
}

/// The topmost path component that does not exist yet (or `path` itself).
fn first_missing_ancestor(path: &Path) -> PathBuf {
    let mut candidate = path.to_path_buf();
    for ancestor in path.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() || ancestor.exists() {
            break;
        }
        candidate = ancestor.to_path_buf();
    }
    candidate
}

fn remove_any(path: &Path) -> Result<(), io::Error> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn copy_tree(from: &Path, to: &Path) -> Result<(), io::Error> {
    fs::create_dir_all(to)?;
    for entry in walkdir::WalkDir::new(from).min_depth(1).sort_by_file_name() {
        let entry = entry.map_err(io::Error::other)?;
        let rel = entry
            .path()
            .strip_prefix(from)
            .map_err(|e| io::Error::other(e.to_string()))?;
        let dest = to.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dest)?;
        } else if entry.file_type().is_symlink() {
            let target = fs::read_link(entry.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &dest)?;
            #[cfg(not(unix))]
            {
                let _ = target;
            }
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
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
}
