// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! Run records: one directory per `cig run`, under `$CIGSCRIPT_HOME/runs`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where CigScript keeps its state. `CIGSCRIPT_HOME` overrides the default
/// of `~/.cigscript`.
pub fn home_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("CIGSCRIPT_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cigscript")
}

pub fn runs_dir() -> PathBuf {
    home_dir().join("runs")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub version: String,
    pub script: String,
    pub script_sha256: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub mode: String,
    pub started: String,
    #[serde(default)]
    pub finished: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    /// The failing diagnostic in its `--json` form, so `cig doctor <run>`
    /// can diagnose it again later, elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<serde_json::Value>,
    #[serde(default)]
    pub burns: usize,
    #[serde(default)]
    pub irreversible: usize,
    #[serde(default)]
    pub rolled_back: bool,
    /// What the automatic rollback could not restore, one line each; the
    /// status is `rollback_incomplete` when this is not empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rollback_failed: Vec<String>,
    /// Process id of the run, to tell an interrupted run from a live one.
    #[serde(default)]
    pub pid: Option<u32>,
    /// The directory this record was read from, which is where its journal
    /// is even when the directory no longer matches the id.
    #[serde(skip)]
    pub loaded_from: Option<PathBuf>,
}

impl RunRecord {
    /// A fresh record with a unique, sortable id: `YYYYMMDDTHHMMSS-<hash>`.
    pub fn begin(script: &Path, source: &str, args: &[String], mode: &str) -> Self {
        use sha2::{Digest, Sha256};
        let now = chrono::Utc::now();
        let sha = hex::encode(Sha256::digest(source.as_bytes()));
        let mut salt = Sha256::new();
        salt.update(now.timestamp_nanos_opt().unwrap_or(0).to_le_bytes());
        salt.update(std::process::id().to_le_bytes());
        salt.update(script.to_string_lossy().as_bytes());
        let salt = hex::encode(salt.finalize());
        Self {
            id: format!("{}-{}", now.format("%Y%m%dT%H%M%S"), &salt[..6]),
            version: crate::VERSION.to_string(),
            script: script.to_string_lossy().to_string(),
            script_sha256: sha,
            args: args.to_vec(),
            cwd: std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            mode: mode.to_string(),
            started: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            finished: None,
            status: "running".to_string(),
            error: None,
            burns: 0,
            irreversible: 0,
            diagnostic: None,
            rolled_back: false,
            rollback_failed: Vec::new(),
            pid: Some(std::process::id()),
            loaded_from: None,
        }
    }

    /// A record for a run directory whose `run.json` cannot be read. The
    /// journal beside it is what matters; nothing about the run is claimed
    /// beyond its id, and `cig unburn` can still reach it.
    pub fn damaged(id: &str, e: &io::Error) -> Self {
        // The id carries the start time, so the run still sorts in order.
        let started =
            chrono::NaiveDateTime::parse_from_str(id.get(..15).unwrap_or(id), "%Y%m%dT%H%M%S")
                .map(|t| {
                    t.and_utc()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                })
                .unwrap_or_else(|_| id.to_string());
        Self {
            id: id.to_string(),
            version: String::new(),
            script: String::new(),
            script_sha256: String::new(),
            args: Vec::new(),
            cwd: String::new(),
            mode: "run".to_string(),
            started,
            finished: None,
            status: "damaged".to_string(),
            error: Some(format!("run.json is damaged: {e}")),
            burns: 0,
            irreversible: 0,
            diagnostic: None,
            rolled_back: false,
            rollback_failed: Vec::new(),
            pid: None,
            loaded_from: None,
        }
    }

    /// Count burns from the journal, which is authoritative even when the
    /// run never got to write its totals, and recognise a run whose process
    /// died mid-burn.
    pub fn refresh_from_journal(&mut self) {
        if let Ok(j) = super::journal::Journal::load(&self.dir()) {
            let burns = j.len();
            let irreversible = j.entries().iter().filter(|e| !e.reversible).count();
            if burns > self.burns {
                self.burns = burns;
                self.irreversible = irreversible;
            }
        }
        if self.status == "running" && !self.pid.map(pid_alive).unwrap_or(false) {
            self.status = "interrupted".to_string();
        }
    }

    pub fn is_interrupted(&self) -> bool {
        self.status == "interrupted"
    }

    pub fn dir(&self) -> PathBuf {
        self.loaded_from
            .clone()
            .unwrap_or_else(|| runs_dir().join(&self.id))
    }

    pub fn save(&self) -> io::Result<()> {
        let dir = self.dir();
        fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        // Write-then-rename so a crash never leaves a torn record.
        let tmp = dir.join("run.json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(tmp, dir.join("run.json"))
    }

    pub fn finish(&mut self, status: &str, error: Option<String>) {
        self.finished =
            Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        self.status = status.to_string();
        self.error = error;
    }

    pub fn load(run_dir: &Path) -> io::Result<Self> {
        let text = fs::read_to_string(run_dir.join("run.json"))?;
        let mut rec: Self = serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        rec.loaded_from = Some(run_dir.to_path_buf());
        Ok(rec)
    }
}

/// All run records, newest first.
/// Is a process with this id still alive? Read-only: no signal is sent.
pub fn pid_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        // The pid may have been reused by another program since the run
        // died; it counts as ours only if it is a cig.
        match fs::read_to_string(format!("/proc/{pid}/comm")) {
            Ok(comm) => comm.trim().starts_with("cig"),
            Err(_) => false,
        }
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

pub fn list() -> io::Result<Vec<RunRecord>> {
    let dir = runs_dir();
    let mut out = Vec::new();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        if entry.path().join("run.json").is_file() {
            match RunRecord::load(&entry.path()) {
                Ok(mut rec) => {
                    rec.refresh_from_journal();
                    out.push(rec);
                }
                Err(e) => {
                    // A damaged record must not hide a run whose journal
                    // is intact: list it, say so, and let unburn reach it.
                    if let Some(id) = entry.file_name().to_str() {
                        let mut rec = RunRecord::damaged(id, &e);
                        rec.loaded_from = Some(entry.path());
                        rec.refresh_from_journal();
                        out.push(rec);
                    }
                }
            }
        }
    }
    // Newest first; `started` carries milliseconds, ids only seconds.
    out.sort_by(|a, b| b.started.cmp(&a.started).then_with(|| b.id.cmp(&a.id)));
    Ok(out)
}

/// Resolve a full id or a unique prefix to a run directory.
pub fn find(id_or_prefix: &str) -> io::Result<Option<RunRecord>> {
    let all = list()?;
    if let Some(exact) = all.iter().find(|r| r.id == id_or_prefix) {
        return Ok(Some(exact.clone()));
    }
    let matches: Vec<&RunRecord> = all
        .iter()
        .filter(|r| r.id.starts_with(id_or_prefix))
        .collect();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0].clone())),
        n => Err(io::Error::other(format!(
            "`{id_or_prefix}` matches {n} runs; give more of the id"
        ))),
    }
}
