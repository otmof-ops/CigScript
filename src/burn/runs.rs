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
    #[serde(default)]
    pub burns: usize,
    #[serde(default)]
    pub irreversible: usize,
    #[serde(default)]
    pub rolled_back: bool,
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
            rolled_back: false,
        }
    }

    pub fn dir(&self) -> PathBuf {
        runs_dir().join(&self.id)
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
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// All run records, newest first.
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
            if let Ok(rec) = RunRecord::load(&entry.path()) {
                out.push(rec);
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
