// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! The inbox: the one place under the state directory a guarded hop can
//! write. A cig started inside the hop has `inbox/<run>/home` as its state
//! root and `inbox/<run>/keys` as its key folder, so it runs as it would
//! anywhere; when the hop ends, its runs are taken in as children of the
//! run whose hop it was, and the inbox goes. A child's changes reached the
//! pack through the hop's layer, so they are in the parent's journal with
//! their before-images already: a child keeps its record and its journal
//! for the reading, not its objects, and is unburned through its parent.

use std::fs;
use std::path::{Path, PathBuf};

use super::overlay::dir_name;
use super::runs::RunRecord;

/// What the take-in found: the children now recorded, and what was
/// dropped, with why.
#[derive(Debug, Default)]
pub struct TakenIn {
    pub children: Vec<String>,
    pub notes: Vec<String>,
}

/// `YYYYMMDDTHHMMSS-xxxxxx`: the shape of a run id, and nothing else.
fn plausible_id(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 22
        && b[8] == b'T'
        && b[15] == b'-'
        && b[..8].iter().chain(&b[9..15]).all(u8::is_ascii_digit)
        && b[16..].iter().all(u8::is_ascii_hexdigit)
}

/// Take a run's inbox in: each run under it becomes a child of this run,
/// its journal under this machine's key; whatever is not a run is dropped
/// with a note. The inbox is removed either way.
pub fn take_in(outer_run_dir: &Path) -> TakenIn {
    let mut out = TakenIn::default();
    let inbox = super::overlay::inbox_dir(outer_run_dir);
    if !inbox.is_dir() {
        return out;
    }
    let outer = dir_name(outer_run_dir);
    if let Ok(rd) = fs::read_dir(inbox.join("home").join("runs")) {
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for dir in entries {
            let id = dir_name(&dir);
            match take_one(&dir, &id, &outer) {
                Ok(()) => out.children.push(id),
                Err(why) => out.notes.push(format!(
                    "the inbox of run {outer} held `{id}`, which is not a run of a cig inside the hop ({why}); dropped"
                )),
            }
        }
    }
    let _ = super::overlay::remove_tree_force(&inbox);
    out
}

fn take_one(dir: &Path, id: &str, outer: &str) -> Result<(), String> {
    if !plausible_id(id) {
        return Err("not a run id".to_string());
    }
    let mut rec = RunRecord::load(dir).map_err(|e| format!("run.json: {e}"))?;
    if rec.id != id {
        return Err("run.json names another run".to_string());
    }
    let dest = super::runs::runs_dir().join(id);
    if dest.exists() {
        return Err("a run of that id already exists".to_string());
    }
    let journal = fs::read_to_string(dir.join("journal.jsonl")).unwrap_or_default();
    for (n, line) in journal.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        serde_json::from_str::<serde_json::Value>(line)
            .map_err(|_| format!("journal line {} is not JSON", n + 1))?;
    }
    fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o700));
    }
    if !journal.is_empty() {
        fs::write(dest.join("journal.jsonl"), journal.as_bytes()).map_err(|e| e.to_string())?;
    }
    if let Ok(after) = fs::read(dir.join("after.json")) {
        let _ = fs::write(dest.join("after.json"), after);
    }
    // Under this machine's key now, like any run recorded here.
    let head = if dest.join("journal.jsonl").is_file() {
        super::journal::resign_in_place(&dest).map_err(|e| e.to_string())?
    } else {
        None
    };
    // A child the cig inside took in itself keeps its own parent.
    if rec.parent.is_none() {
        rec.parent = Some(outer.to_string());
    }
    if head.is_some() {
        rec.journal_head = head;
    }
    rec.loaded_from = Some(dest);
    rec.save().map_err(|e| e.to_string())?;
    Ok(())
}
