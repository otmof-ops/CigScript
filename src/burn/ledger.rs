// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-script)
// SPDX-License-Identifier: Apache-2.0

//! The ledger: one line per run, appended when a run ends, when it is
//! unburned, when it is pruned or forgotten, and when a parcel brings one
//! in. Each line carries a MAC under the `ledger-mac` subkey over the line
//! before it and its own text, so a line edited, removed or reordered no
//! longer verifies, and a journal cut short or swapped for an older copy
//! is caught against the head the ledger recorded for it. It lives in the
//! keys directory, apart from the tin, owner-only, append-only. Each line
//! names the tin it witnesses (a hash of the state directory's path), so
//! two state directories beside one key keep separate accounts.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

type HmacSha256 = Hmac<Sha256>;

pub const FILE: &str = "ledger.jsonl";
/// What the first line's MAC chains from.
const SEED: &str = "cig-ledger";

/// Where the ledger lives: beside the key, outside the tin.
pub fn path() -> PathBuf {
    super::seal::keys_dir().join(FILE)
}

/// One entry. `kind` is `run` (a run ended), `unburn` (a rollback ended),
/// `pruned`, `forgotten` (the user said to stop noting a missing run) or
/// `origin` (a run a parcel brought in).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Line {
    pub seq: u64,
    pub kind: String,
    pub run: String,
    pub head: String,
    pub status: String,
    pub lines: u64,
    /// The tin this line witnesses: `tin_id()` of the state directory
    /// whose run it records.
    #[serde(default)]
    pub tin: String,
    pub t: String,
    #[serde(default)]
    pub mac: String,
}

/// What names a tin in the ledger: the first sixteen hex digits of the
/// SHA-256 of its state directory's canonical path. Derived from where the
/// tin is, not from anything kept inside it, so nothing written into the
/// tin can detach it from its own account.
pub fn tin_id() -> String {
    use sha2::Digest;
    let home = super::runs::home_dir();
    let home = fs::canonicalize(&home)
        .or_else(|_| std::path::absolute(&home))
        .unwrap_or(home);
    let digest = Sha256::digest(home.to_string_lossy().as_bytes());
    hex::encode(&digest[..8])
}

/// The ledger as read: every line, and where its chain first failed.
#[derive(Clone, Debug, Default)]
pub struct Loaded {
    pub lines: Vec<Line>,
    /// 1-based line number of the first line whose MAC does not verify.
    pub broken_at: Option<usize>,
    /// The MAC of the last line (the seed when there is none).
    pub head: String,
}

impl Loaded {
    /// The lines that witness this tin. A line with no tin named is from
    /// a ledger written before lines were stamped, and counts as this
    /// tin's.
    pub fn of_this_tin(&self) -> impl DoubleEndedIterator<Item = &Line> {
        let me = tin_id();
        self.lines
            .iter()
            .filter(move |l| l.tin.is_empty() || l.tin == me)
    }
}

fn key() -> Result<[u8; 32], String> {
    super::seal::key().map(|(m, _)| super::seal::derive(&m, super::seal::LABEL_LEDGER_MAC))
}

fn mac_hex(key: &[u8; 32], prev: &str, body: &[u8]) -> String {
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    m.update(prev.as_bytes());
    m.update(body);
    hex::encode(m.finalize().into_bytes())
}

/// The bytes a line's MAC covers: the line as it was before its own `mac`
/// field, which is always written last; the closing brace is put back, so
/// this is exactly the body that was MACed.
fn body_of(line: &str) -> String {
    match line.rfind(",\"mac\":") {
        Some(i) => format!("{}}}", &line[..i]),
        None => line.to_string(),
    }
}

/// Read and verify the ledger. A missing file is an empty ledger. The key
/// unavailable is an error: nothing can be verified without it.
pub fn load() -> Result<Loaded, String> {
    let text = match fs::read_to_string(path()) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Loaded {
                head: SEED.to_string(),
                ..Default::default()
            })
        }
        Err(e) => return Err(format!("cannot read the ledger: {e}")),
    };
    let key = key()?;
    let mut out = Loaded {
        head: SEED.to_string(),
        ..Default::default()
    };
    let mut prev = SEED.to_string();
    for (i, raw) in text.lines().filter(|l| !l.trim().is_empty()).enumerate() {
        let line: Line = match serde_json::from_str(raw) {
            Ok(l) => l,
            Err(_) => {
                if out.broken_at.is_none() {
                    out.broken_at = Some(i + 1);
                }
                continue;
            }
        };
        let want = mac_hex(&key, &prev, body_of(raw).as_bytes());
        if line.mac != want && out.broken_at.is_none() {
            out.broken_at = Some(i + 1);
        }
        prev = line.mac.clone();
        out.lines.push(line);
    }
    out.head = prev;
    Ok(out)
}

/// Append one line, MACed against the current head, and make it durable.
pub fn append(kind: &str, run: &str, head: &str, status: &str, lines: u64) -> Result<(), String> {
    let key = key()?;
    let loaded = load()?;
    let seq = loaded.lines.last().map(|l| l.seq + 1).unwrap_or(1);
    let t = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    // The body, with `mac` written last so `body_of` finds it again.
    let mut v = serde_json::json!({
        "seq": seq, "kind": kind, "run": run, "head": head, "status": status, "lines": lines,
        "tin": tin_id(), "t": t,
    });
    let body = serde_json::to_string(&v).map_err(|e| e.to_string())?;
    let mac = mac_hex(&key, &loaded.head, body.as_bytes());
    v["mac"] = serde_json::Value::String(mac);
    let text = serde_json::to_string(&v).map_err(|e| e.to_string())?;
    debug_assert_eq!(body_of(&text), body, "the MAC must cover exactly the body");
    let p = path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let mut opts = fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&p)
        .map_err(|e| format!("cannot open the ledger: {e}"))?;
    f.write_all(text.as_bytes())
        .and_then(|()| f.write_all(b"\n"))
        .and_then(|()| f.sync_all())
        .map_err(|e| e.to_string())
}

/// The last `run` or `unburn` line for a run: what the ledger says its
/// journal ended at.
pub fn last_record_of<'a>(loaded: &'a Loaded, run: &str) -> Option<&'a Line> {
    loaded
        .lines
        .iter()
        .rev()
        .find(|l| l.run == run && (l.kind == "run" || l.kind == "unburn" || l.kind == "origin"))
}

/// Has the ledger been told this run is gone on purpose (pruned or
/// forgotten)?
pub fn is_let_go(loaded: &Loaded, run: &str) -> bool {
    loaded
        .lines
        .iter()
        .rev()
        .find(|l| l.run == run)
        .map(|l| l.kind == "pruned" || l.kind == "forgotten")
        .unwrap_or(false)
}
