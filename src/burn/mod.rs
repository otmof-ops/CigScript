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
pub mod git;
pub mod journal;
pub mod lock;
pub mod protocol;
pub mod runs;
pub mod seal;
pub mod tin;

/// The one lock every test in this crate that sets `CIGSCRIPT_HOME` takes:
/// the tests share a process and an environment, and `home_dir` reads it.
#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

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
        /// The file is a secret (`fs.write_text(p, t, {sensitive: true})`):
        /// its before-bytes are kept sealed, whatever the name.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        sensitive: bool,
    },
    Append {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        sensitive: bool,
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
        /// Globs of the files the hop may change; their before-bytes go
        /// to the tin first, so a change under them can be undone.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        touches: Vec<String>,
        /// Globs among them whose files are secrets: kept sealed.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        touches_sealed: Vec<String>,
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
        /// What the hop left at the path (its time and size), so a rollback
        /// can tell a file changed again since from the hop's own output.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        left: Option<Left>,
    },
    /// A file a hop modified or deleted inside the pack. Observed after the
    /// hop; undone only when the hop said it might touch the path, which
    /// put the before-bytes in the tin first (`tinned`).
    HopChanged {
        path: PathBuf,
        change: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        tinned: bool,
        /// Where the before-state is kept when the change is reversible:
        /// `tin` for a file's bytes, `link` for a symbolic link's target,
        /// which the journal holds itself, `dir` for a directory the hop
        /// deleted or replaced, made again from its recorded mode.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kept: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        left: Option<Left>,
    },
    /// The compensation recorded when a `burn { } unburn { }` block ends.
    Compensate {
        line: u32,
        /// The burn's name, when it has one (`burn (s, "deploy")`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}

/// What a hop left at a path: its modification time and size as the scan
/// after the hop read them. A rollback compares them with the file now.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Left {
    pub mtime_ns: u64,
    pub len: u64,
}

impl Op {
    /// The one path a hop-watch op is about.
    pub fn path(&self) -> Option<&PathBuf> {
        match self {
            Op::HopCreated { path, .. } | Op::HopChanged { path, .. } => Some(path),
            _ => None,
        }
    }

    /// What a hop left at the path this op is about, when it was a file.
    pub fn left(&self) -> Option<(u64, u64)> {
        match self {
            Op::HopCreated { left, .. } | Op::HopChanged { left, .. } => {
                left.map(|l| (l.mtime_ns, l.len))
            }
            _ => None,
        }
    }

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
        match self {
            Op::Proc { .. } | Op::EnvSet { .. } | Op::EnvUnset { .. } => false,
            Op::HopChanged { tinned, .. } => *tinned,
            _ => true,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Op::Write { path, sensitive } => {
                format!("write {}{}", path.display(), sealed_note(path, *sensitive))
            }
            Op::Append { path, sensitive } => {
                format!(
                    "append to {}{}",
                    path.display(),
                    sealed_note(path, *sensitive)
                )
            }
            Op::Delete { path } => format!("delete {}", path.display()),
            Op::Move { from, to } => format!("move {} -> {}", from.display(), to.display()),
            Op::Copy { from, to } => format!("copy {} -> {}", from.display(), to.display()),
            Op::Mkdir { path } => format!("mkdir {}", path.display()),
            Op::Proc {
                cmd,
                args,
                cwd,
                touches,
                ..
            } => {
                let mut s = format!("run {}", shell_quote(cmd));
                for a in args {
                    s.push(' ');
                    s.push_str(&shell_quote(a));
                }
                if let Some(c) = cwd {
                    s.push_str(&format!("  (in {})", c.display()));
                }
                if !touches.is_empty() {
                    s.push_str(&format!("  (touches {})", touches.join(", ")));
                }
                s
            }
            Op::EnvSet { key } => format!("set env {key}"),
            Op::EnvUnset { key } => format!("unset env {key}"),
            Op::HopCreated { path, .. } => format!("hop created {}", path.display()),
            Op::HopChanged {
                path,
                change,
                tinned,
                kept,
                ..
            } => format!(
                "hop {change} {}{}",
                path.display(),
                match (tinned, kept.as_deref()) {
                    (true, Some("link")) => "  (link kept)",
                    (true, Some("dir")) => "  (directory)",
                    (true, Some("git")) => "  (git)",
                    (true, Some("sealed")) => "  (sealed)",
                    (true, Some("reported")) => "  (reported by the hop)",
                    (true, _) => "  (tin)",
                    (false, _) => "",
                }
            ),
            Op::Compensate { line, label } => match label {
                Some(l) => format!("compensation for {l}"),
                None => format!("compensation for the burn at line {line}"),
            },
        }
    }
}

/// The copy of the before-bytes a file claim points at, with the hash it
/// claims for them, when it gave one.
fn saved_of(c: &protocol::Claim) -> Option<(&Path, Option<&String>)> {
    match c {
        protocol::Claim::Write {
            saved: Some(s),
            before,
            ..
        }
        | protocol::Claim::Delete {
            saved: Some(s),
            before,
            ..
        } => Some((
            s.as_path(),
            match before {
                protocol::Claimed::Hash(h) => Some(h),
                _ => None,
            },
        )),
        _ => None,
    }
}

/// `  (sealed)` when the before-bytes of a native write are kept sealed:
/// the script said so, or the name is a secret's and sealing is on.
fn sealed_note(path: &Path, sensitive: bool) -> &'static str {
    if seal::policy() != seal::Policy::Never && (sensitive || seal::secret_name(path)) {
        "  (sealed)"
    } else {
        ""
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
    /// The pack's lock, held for the run: two runs on one pack would roll
    /// back each other's work.
    lock: Option<lock::PackLock>,
    /// Warnings the end of a hop's watch produced, waiting to be printed.
    late_warnings: Vec<String>,
    /// What a hop's report (the burn protocol) could not be used for.
    late_claims: Vec<String>,
    /// The git blobs this run refers to instead of copying, by repository:
    /// pinned under `refs/cig/runs/<id>` as one tree, rebuilt as it grows.
    git_pins: std::collections::BTreeMap<PathBuf, std::collections::BTreeSet<String>>,
}

/// The `touches` glob nearest to `glob` among the globs that would match an
/// existing file: the file itself, or a `dir/**` over one of its parents.
fn nearest_glob(glob: &str, existing: &[String]) -> Option<String> {
    let mut candidates: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for rel in existing {
        candidates.insert(rel.clone());
        let mut parts: Vec<&str> = rel.split('/').collect();
        parts.pop();
        while !parts.is_empty() {
            candidates.insert(format!("{}/**", parts.join("/")));
            parts.pop();
        }
    }
    let limit = if glob.len() <= 4 { 1 } else { 2 };
    candidates
        .into_iter()
        .filter(|c| c != glob)
        .map(|c| (edit_distance(glob, &c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.len().cmp(&b.1.len())))
        .map(|(_, c)| c)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
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

/// What hop watching sees of one path: enough to notice a change without
/// reading the file. A link is its target, and the time and size of the
/// file it leads to when it leads to one, so a write through it is seen; a
/// directory is watched for being made, deleted or replaced, and its mode
/// is kept so a deleted one comes back as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Seen {
    File {
        mtime: u64,
        len: u64,
        mode: u32,
    },
    Dir {
        mode: u32,
    },
    Link {
        target: PathBuf,
        through: Option<(u64, u64)>,
    },
}

impl Seen {
    fn is_dir(&self) -> bool {
        matches!(self, Seen::Dir { .. })
    }

    /// The (mtime, len) of a regular file, for the `left` a journal entry
    /// records.
    fn left(&self) -> Option<Left> {
        match self {
            Seen::File { mtime, len, .. } => Some(Left {
                mtime_ns: *mtime,
                len: *len,
            }),
            _ => None,
        }
    }
}

/// One pass over a tree: what was seen, whether the pass saw everything,
/// and the directory links it did not walk into.
struct Scan {
    seen: std::collections::BTreeMap<PathBuf, Seen>,
    /// False when the walk stopped at the cap. What was not seen cannot be
    /// reasoned about, and the kernel says so instead of guessing.
    complete: bool,
    /// Links to directories, with where they lead: the kernel decides
    /// whether a target is part of the pack or a door out of it.
    dir_links: Vec<(PathBuf, PathBuf)>,
}

/// A snapshot of mtimes, sizes, modes and link targets, cheap enough to
/// take around every hop, plus the before-bytes of the files the hop said
/// it may touch.
pub struct PackWatch {
    inside: std::collections::BTreeMap<PathBuf, Seen>,
    /// A scan stopped at the cap: what the hop created cannot be told from
    /// what the scan never reached, so nothing is journaled as created.
    partial: bool,
    /// Places outside the pack the hop may write in, each scanned
    /// shallowly: its working directory when that is outside, and where a
    /// directory link inside the pack leads when that is outside. Changes
    /// there are reported (E752), never journaled.
    outside: Vec<(PathBuf, std::collections::BTreeMap<PathBuf, Seen>)>,
    /// Files stashed in the tin before the hop, by path.
    tinned: std::collections::BTreeMap<PathBuf, tin::Stashed>,
    /// The hop's `touches`, for the hints the report gives.
    touches: Vec<String>,
    /// What could not be stashed, one line each, for the hop to report.
    pub warnings: Vec<String>,
}

/// How many entries hop watching reads of a pack before it stops. The
/// `watch_cap` setting raises it; the safety rule at the cap holds at any
/// size.
pub fn watch_cap() -> usize {
    static CAP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CAP.get_or_init(|| {
        crate::config::Config::load()
            .get("watch_cap")
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(50_000)
    })
}

/// The device name any component of a path spells, if one does: `con`,
/// `prn`, `aux`, `nul`, `conin$`, `conout$`, `com0` to `com9` and `lpt0`
/// to `lpt9` (the superscript digits included), in any case and with any
/// extension (`nul.txt` is still the null device on Windows, and so is
/// `out/nul/x`: no directory can be called that). Such a path is a device
/// on Windows and an ordinary file everywhere else, so a script that
/// writes to one is not portable; the checker says so, and the kernel
/// refuses it on Windows before anything is journaled.
pub fn reserved_device_name(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    for part in text.split(['/', '\\']) {
        let stem = part
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end_matches([' ', '.'])
            .to_ascii_lowercase();
        let reserved = matches!(
            stem.as_str(),
            "con" | "prn" | "aux" | "nul" | "conin$" | "conout$"
        ) || ((stem.starts_with("com") || stem.starts_with("lpt"))
            && port_number(&stem[3..]));
        if reserved {
            return Some(stem);
        }
    }
    None
}

/// One digit, `0` to `9`, or a superscript `¹`, `²` or `³`: the port
/// numbers Windows reserves after `com` and `lpt`.
fn port_number(rest: &str) -> bool {
    let mut chars = rest.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(c), None) if c.is_ascii_digit() || matches!(c, '\u{b9}' | '\u{b2}' | '\u{b3}')
    )
}

/// Do `a` and `b` lead to one object on disk, symlinks followed?
#[cfg(unix)]
pub(crate) fn same_object(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

/// On Windows the canonical path spells the file as the disk does, so two
/// spellings of one file canonicalize alike; a path that does not exist
/// has no canonical form and is nobody's twin.
#[cfg(not(unix))]
pub(crate) fn same_object(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The directory a path names, lexically: no symlink is resolved, so a
/// link is placed where it sits, not where it points.
pub(crate) fn lexical_parent(p: &Path) -> Option<PathBuf> {
    normalize_root(&p.to_string_lossy())
        .parent()
        .map(Path::to_path_buf)
}

/// `a` and `b` are one name spelled in two cases, in one directory: what
/// a case-insensitive filesystem (APFS, NTFS) reads as the same entry.
pub(crate) fn case_variants(a: &Path, b: &Path) -> bool {
    let (Some(x), Some(y)) = (a.file_name(), b.file_name()) else {
        return false;
    };
    if x == y {
        return false;
    }
    let (x, y) = (x.to_string_lossy(), y.to_string_lossy());
    if x.to_lowercase() != y.to_lowercase() {
        return false;
    }
    match (lexical_parent(a), lexical_parent(b)) {
        (Some(p), Some(q)) => p == q,
        _ => false,
    }
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

/// Is this entry the pack's? A file or directory by where it resolves; a
/// link by where it sits, since a link to the outside is still an entry
/// inside, removable without touching what it points to.
fn entry_inside(p: &Path, meta: &Seen, roots: &[PathBuf]) -> bool {
    if matches!(meta, Seen::Link { .. }) {
        match p.parent() {
            Some(parent) => inside_roots(parent, roots),
            None => inside_roots(p, roots),
        }
    } else {
        inside_roots(p, roots)
    }
}

fn mode_bits(m: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        0
    }
}

fn scan(root: &Path, max_depth: usize, cap: usize) -> Scan {
    let mut seen = std::collections::BTreeMap::new();
    let mut dir_links = Vec::new();
    let mut complete = true;
    // Links are not walked: a link is watched as a link (its target can
    // change, and a write through it is seen by the time and size of what
    // it leads to), and what it leads to is seen under its own path when
    // that is inside the pack. A link that leads out of the pack is a door,
    // not a part of it, so nothing behind it is ever journaled.
    let ignore = tin::ignore_set(root);
    // The state directory is never part of a pack, wherever CIGSCRIPT_HOME
    // put it: the tin watching itself would stash its own objects.
    let home = runs::home_dir();
    let mut walk = walkdir::WalkDir::new(root)
        .min_depth(1)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            if tin::SKIPPED_ALWAYS.contains(&n.as_ref()) || e.path() == home {
                return false;
            }
            match (&ignore, e.path().strip_prefix(root)) {
                (Some(set), Ok(rel)) => !set.is_match(rel),
                _ => true,
            }
        });
    for entry in walk.by_ref() {
        let Ok(entry) = entry else { continue };
        if seen.len() >= cap {
            complete = false;
            break;
        }
        let path = entry.path().to_path_buf();
        if entry.file_type().is_symlink() {
            let Ok(target) = std::fs::read_link(&path) else {
                continue;
            };
            // A dangling link resolves to nothing and is a link all the same.
            let through = match std::fs::metadata(&path) {
                Ok(m) if m.is_file() => Some((journal::mtime_ns(&m).unwrap_or(0), m.len())),
                Ok(m) if m.is_dir() => {
                    dir_links.push((path.clone(), journal::resolve_symlink_chain(&path)));
                    None
                }
                _ => None,
            };
            seen.insert(path, Seen::Link { target, through });
            continue;
        }
        if entry.file_type().is_dir() {
            let mode = journal::fresh_metadata(&entry)
                .map(|m| mode_bits(&m))
                .unwrap_or(0);
            seen.insert(path, Seen::Dir { mode });
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(m) = journal::fresh_metadata(&entry) else {
            continue;
        };
        seen.insert(
            path,
            Seen::File {
                mtime: journal::mtime_ns(&m).unwrap_or(0),
                len: m.len(),
                mode: mode_bits(&m),
            },
        );
    }
    Scan {
        seen,
        complete,
        dir_links,
    }
}

/// The `touches` glob that would have covered `rel`, for the hint when a
/// file outside every glob changed: its directory's tree, or the root's
/// own files.
fn covering_glob(rel: &Path) -> String {
    match rel.parent() {
        Some(p) if !p.as_os_str().is_empty() => {
            format!("{}/**", p.to_string_lossy().replace('\\', "/"))
        }
        _ => "*".to_string(),
    }
}

/// The warnings for files a hop changed outside every `touches` glob. A
/// lone file is named, with the glob that would have kept it. Several are
/// said once per top-level folder they fall under, as the smallest
/// directory that covers them and one glob for it, with a few of them as
/// examples: a deleted folder of a thousand files is one line, not a
/// thousand, and `cig runs <id>` has every one.
fn uncovered_warnings(uncovered: &[(PathBuf, String)], run: &str) -> Vec<String> {
    let mut groups: std::collections::BTreeMap<String, Vec<&(PathBuf, String)>> =
        std::collections::BTreeMap::new();
    for item in uncovered {
        let top = match item.1.split_once('/') {
            Some((top, _)) => top.to_string(),
            None => String::new(),
        };
        groups.entry(top).or_default().push(item);
    }
    let mut out = Vec::new();
    for files in groups.values() {
        if let [(p, rel)] = files.as_slice() {
            out.push(format!(
                "{} is not under touches, so the hop's change to it is irreversible; touches: [\"{}\"] would keep it",
                p.display(),
                covering_glob(Path::new(rel))
            ));
            continue;
        }
        let dir = common_dir(files.iter().map(|(_, rel)| rel.as_str()));
        let (place, glob) = if dir.is_empty() {
            ("at the pack root".to_string(), "*".to_string())
        } else {
            (format!("under {dir}/"), format!("{dir}/**"))
        };
        let examples: Vec<&str> = files.iter().take(3).map(|(_, rel)| rel.as_str()).collect();
        out.push(format!(
            "{} files {place} are not under touches, so the hop's changes to them are irreversible; touches: [\"{glob}\"] would keep them ({}{}; cig runs {run} lists them all)",
            thousands(files.len()),
            examples.join(", "),
            if files.len() > examples.len() {
                ", ..."
            } else {
                ""
            }
        ));
    }
    out
}

/// The deepest directory every one of these relative paths is under, as
/// `a/b`; empty for files at the root.
fn common_dir<'a>(rels: impl Iterator<Item = &'a str>) -> String {
    let mut common: Option<Vec<&str>> = None;
    for rel in rels {
        let mut parts: Vec<&str> = rel.split('/').collect();
        parts.pop();
        common = Some(match common {
            None => parts,
            Some(c) => c
                .iter()
                .zip(parts.iter())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| *a)
                .collect(),
        });
    }
    common.unwrap_or_default().join("/")
}

/// `1000` as `1,000`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// How many of the files the `touches` globs match would be kept sealed,
/// for the plan: by name, by the root's `.cigseal`, by `touches_sealed`.
fn sealed_count(
    roots: &[PathBuf],
    touches: &[String],
    touches_sealed: &[String],
    cap: usize,
) -> usize {
    let Ok(set) = tin::globs(touches) else {
        return 0;
    };
    let hop_sealed = if touches_sealed.is_empty() {
        None
    } else {
        tin::globs(touches_sealed).ok()
    };
    let mut n = 0;
    for r in roots {
        let root_set = seal::seal_set(r);
        let sets: Vec<&globset::GlobSet> = [root_set.as_ref(), hop_sealed.as_ref()]
            .into_iter()
            .flatten()
            .collect();
        for (path, meta) in &scan(r, 64, cap).seen {
            if meta.is_dir() || matches!(meta, Seen::Link { .. }) {
                continue;
            }
            let Ok(rel) = path.strip_prefix(r) else {
                continue;
            };
            if set.is_match(rel) && seal::should_seal(path, Some(r), &sets) {
                n += 1;
            }
        }
    }
    n
}

/// How many of the pack's files each `touches` glob matches, for the dry
/// run's plan line and the warning about a glob that matches nothing.
fn touches_counts(roots: &[PathBuf], touches: &[String], cap: usize) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = touches.iter().map(|t| (t.clone(), 0)).collect();
    let singles: Vec<Option<globset::GlobSet>> = touches
        .iter()
        .map(|t| tin::globs(std::slice::from_ref(t)).ok())
        .collect();
    for r in roots {
        for (path, meta) in &scan(r, 64, cap).seen {
            if meta.is_dir() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(r) else {
                continue;
            };
            for (i, set) in singles.iter().enumerate() {
                if set.as_ref().is_some_and(|s| s.is_match(rel)) {
                    counts[i].1 += 1;
                }
            }
        }
    }
    counts
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
            lock: None,
            late_warnings: Vec::new(),
            late_claims: Vec::new(),
            git_pins: std::collections::BTreeMap::new(),
        })
    }

    /// Declare the pack: the roots every native write must stay inside. A
    /// real run takes the pack's lock first: another run already using it
    /// (or one overlapping it) is refused with E759, because hop watching
    /// would read that run's writes as this hop's and roll them back.
    pub fn set_pack(&mut self, roots: Vec<PathBuf>) -> Result<(), Diagnostic> {
        if self.mode == Mode::Run && self.lock.is_none() {
            if let Some(dir) = self.journal.run_dir() {
                let id = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                match lock::acquire(&roots, &id) {
                    Ok(held) => self.lock = Some(held),
                    Err(lock::Busy::Held { root, holder }) => {
                        return Err(burn_error(format!(
                            "the pack {} is in use by {holder}",
                            root.display()
                        ))
                        .code("E759")
                        .with_subject(root.display().to_string())
                        .with_hint("wait for that run to finish (cig runs shows it), or give the two runs packs that do not overlap; nothing was changed"));
                    }
                    Err(lock::Busy::Io(e)) => {
                        return Err(burn_error(format!(
                            "could not take the pack's lock under {}: {e}",
                            lock::dir().display()
                        ))
                        .code("E702")
                        .with_hint("free space or fix permissions under ~/.cigscript (CIGSCRIPT_HOME); nothing was changed"));
                    }
                }
            }
        }
        self.pack = Some(roots);
        Ok(())
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
        let op = Op::Compensate {
            line: comp.line,
            label: comp.label.clone(),
        };
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
    pub fn pack_watch_begin(
        &mut self,
        cwd: Option<&Path>,
        touches: &[String],
        touches_sealed: &[String],
    ) -> Option<PackWatch> {
        let roots = self.pack.clone()?;
        let roots = &roots;
        if self.mode == Mode::DryRun {
            return None;
        }
        let cap = watch_cap();
        let mut inside = std::collections::BTreeMap::new();
        let mut partial = false;
        let mut outside: Vec<(PathBuf, std::collections::BTreeMap<PathBuf, Seen>)> = Vec::new();
        let mut warnings = Vec::new();
        for r in roots {
            let s = scan(r, 64, cap);
            partial |= !s.complete;
            inside.extend(s.seen);
            // A directory link that leads out of the pack: the hop may
            // write through it, so where it leads is watched for the report.
            for (_, target) in s.dir_links {
                if !inside_roots(&target, roots) && !outside.iter().any(|(w, _)| *w == target) {
                    let shallow = scan(&target, 3, 5_000);
                    outside.push((target, shallow.seen));
                }
            }
        }
        if partial {
            warnings.push(format!(
                "the pack has more than {cap} entries, so hop watching saw only part of it; what this hop creates is irreversible this time (a .cigignore at the pack root, or the watch_cap setting, brings the pack under)"
            ));
        }
        // The before-bytes of what the hop may touch go to the tin first.
        let mut tinned = std::collections::BTreeMap::new();
        if !touches.is_empty() {
            match tin::globs(touches) {
                Ok(set) => {
                    // A `touches` that covers no file at all is a typo until
                    // proven otherwise, and would quietly leave the files it
                    // meant irreversible. One glob among several matching
                    // nothing is ordinary (a lockfile the project may have),
                    // so the list is judged as a whole, and a glob written
                    // with a leading `?` (`?yarn.lock`: a file that may not
                    // exist yet) is never counted against it. The dry run
                    // counts each glob, and the report after the hop names
                    // the glob that would have kept a file changed outside
                    // them all.
                    let counts: Vec<(String, usize)> = touches_counts(roots, touches, cap)
                        .into_iter()
                        .filter(|(g, _)| !g.starts_with('?'))
                        .collect();
                    if !partial && !counts.is_empty() && counts.iter().all(|(_, n)| *n == 0) {
                        let existing: Vec<String> = inside
                            .iter()
                            .filter(|(_, m)| !m.is_dir())
                            .filter_map(|(p, _)| {
                                roots
                                    .iter()
                                    .find_map(|r| p.strip_prefix(r).ok())
                                    .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                            })
                            .collect();
                        let near: Vec<String> = counts
                            .iter()
                            .filter_map(|(g, _)| nearest_glob(g, &existing))
                            .collect();
                        let hint = if near.is_empty() {
                            String::new()
                        } else {
                            format!(
                                " (did you mean {}?)",
                                near.iter()
                                    .map(|g| format!("`{g}`"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        warnings.push(format!(
                            "touches {} match{} no file in the pack{hint}; a change to a file it was meant to cover is irreversible",
                            counts
                                .iter()
                                .map(|(g, _)| format!("`{g}`"))
                                .collect::<Vec<_>>()
                                .join(", "),
                            if counts.len() == 1 { "es" } else { "" }
                        ));
                    }
                    // A file git already holds, tracked and clean against the
                    // index, is referenced from the repository instead of
                    // copied: the run pins the blobs it refers to first, and
                    // falls back to the tin for a repository it cannot pin in.
                    let repos: Vec<Option<git::Repo>> =
                        roots.iter().map(|r| git::repo_for(r)).collect();
                    // Secrets are sealed: by name, by the root's `.cigseal`,
                    // or by the hop's `touches_sealed` globs. A sealed file
                    // is never referenced from git or shared by content.
                    let seal_sets: Vec<Option<globset::GlobSet>> =
                        roots.iter().map(|r| seal::seal_set(r)).collect();
                    let hop_sealed = if touches_sealed.is_empty() {
                        None
                    } else {
                        tin::globs(touches_sealed).ok()
                    };
                    let mut from_git: Vec<(usize, PathBuf, String, String, Seen)> = Vec::new();
                    let mut to_tin: Vec<PathBuf> = Vec::new();
                    let mut to_seal: Vec<PathBuf> = Vec::new();
                    for (path, meta) in &inside {
                        if meta.is_dir() {
                            continue;
                        }
                        let Some(ri) = roots.iter().position(|r| {
                            path.strip_prefix(r)
                                .map(|rel| set.is_match(rel))
                                .unwrap_or(false)
                        }) else {
                            continue;
                        };
                        let sets: Vec<&globset::GlobSet> =
                            [seal_sets[ri].as_ref(), hop_sealed.as_ref()]
                                .into_iter()
                                .flatten()
                                .collect();
                        if !matches!(meta, Seen::Link { .. })
                            && seal::should_seal(path, Some(&roots[ri]), &sets)
                        {
                            to_seal.push(path.clone());
                            continue;
                        }
                        if let (Some(repo), Seen::File { len, .. }) = (&repos[ri], meta) {
                            if let Some((blob, sha256)) = repo.clean_blob(path, *len) {
                                from_git.push((ri, path.clone(), blob, sha256, meta.clone()));
                                continue;
                            }
                        }
                        to_tin.push(path.clone());
                    }
                    if !from_git.is_empty() {
                        let run_id = self
                            .journal
                            .run_dir()
                            .and_then(|d| d.file_name())
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let mut pinned_ok: Vec<bool> = vec![false; roots.len()];
                        for (ri, repo) in repos.iter().enumerate() {
                            let Some(repo) = repo else { continue };
                            let blobs: Vec<&String> = from_git
                                .iter()
                                .filter(|(i, ..)| *i == ri)
                                .map(|(_, _, b, ..)| b)
                                .collect();
                            if blobs.is_empty() {
                                continue;
                            }
                            let set = self.git_pins.entry(repo.work_tree.clone()).or_default();
                            for b in blobs {
                                set.insert(b.clone());
                            }
                            match repo.pin(&run_id, set) {
                                Ok(()) => pinned_ok[ri] = true,
                                Err(e) => warnings.push(format!(
                                    "could not pin the blobs this run refers to in {} ({e}); its files are kept in the tin instead",
                                    repo.work_tree.display()
                                )),
                            }
                        }
                        if let Some(dir) = self.journal.run_dir() {
                            let list: String = self
                                .git_pins
                                .keys()
                                .map(|wt| format!("{}\n", wt.display()))
                                .collect();
                            let _ = std::fs::write(dir.join(git::PINS), list);
                        }
                        for (ri, path, blob, sha256, meta) in from_git {
                            if !pinned_ok[ri] {
                                to_tin.push(path);
                                continue;
                            }
                            let repo = repos[ri].as_ref().expect("a pinned repo");
                            let (len, mtime, mode) = match meta {
                                Seen::File { mtime, len, mode } => (len, mtime, mode),
                                _ => (0, 0, 0),
                            };
                            tinned.insert(
                                path,
                                tin::Stashed {
                                    object: PathBuf::new(),
                                    sha256,
                                    bytes: len,
                                    mode: if cfg!(unix) { Some(mode) } else { None },
                                    mtime_ns: Some(mtime),
                                    link: None,
                                    git: Some(journal::GitBlob {
                                        repo: repo.work_tree.clone(),
                                        blob,
                                    }),
                                },
                            );
                        }
                        to_tin.sort();
                    }
                    let mut staged: Vec<(PathBuf, tin::Staged)> = Vec::new();
                    for path in &to_tin {
                        match tin::prepare(path) {
                            Ok(st) => staged.push((path.clone(), st)),
                            Err(e) => warnings.push(format!(
                                "{} could not be kept in the tin ({e}); a change to it stays irreversible",
                                path.display()
                            )),
                        }
                    }
                    for path in &to_seal {
                        match tin::prepare_sealed(path) {
                            Ok(st) => staged.push((path.clone(), st)),
                            Err(e) => warnings.push(format!(
                                "{} could not be sealed into the tin ({e}); a change to it stays irreversible",
                                path.display()
                            )),
                        }
                    }
                    // The run writes down which objects it is about to
                    // depend on before any exists under its name, so a
                    // `cig tin gc` during the hop keeps them (the journal
                    // refers to them only once the hop is over).
                    if let Some(dir) = self.journal.run_dir() {
                        let list: String = staged
                            .iter()
                            .flat_map(|(_, st)| st.pending_paths())
                            .map(|o| format!("{}\n", o.display()))
                            .collect();
                        if !list.is_empty() {
                            let pending = dir.join(tin::PENDING);
                            if let Err(e) = std::fs::write(&pending, &list)
                                .and_then(|()| journal::sync_file(&pending))
                            {
                                warnings.push(format!(
                                    "could not note the tin objects this hop depends on ({e}); a `cig tin gc` during the hop could remove them"
                                ));
                            }
                        }
                    }
                    for (path, st) in staged {
                        match st.commit() {
                            Ok(st) => {
                                tinned.insert(path, st);
                            }
                            Err(e) => warnings.push(format!(
                                "{} could not be kept in the tin ({e}); a change to it stays irreversible",
                                path.display()
                            )),
                        }
                    }
                }
                Err(e) => warnings.push(e),
            }
        }
        let work = cwd
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .map(|p| normalize_root(&p.to_string_lossy()));
        if let Some(w) = work.filter(|w| !inside_roots(w, roots)) {
            if !outside.iter().any(|(o, _)| *o == w) {
                let shallow = scan(&w, 3, 5_000);
                outside.push((w, shallow.seen));
            }
        }
        Some(PackWatch {
            inside,
            partial,
            outside,
            tinned,
            touches: touches.to_vec(),
            warnings,
        })
    }

    /// After a hop: what changed. Files the hop created inside the pack are
    /// journaled as reversible (rollback removes them); modified or deleted
    /// ones as irreversible with the detail, or reversible when the tin kept
    /// them; a directory it deleted or replaced is made again on rollback;
    /// changes outside the pack are returned for the caller to report (E752).
    pub fn pack_watch_end(
        &mut self,
        mut watch: PackWatch,
        claims: Vec<protocol::Claim>,
        hop_cwd: Option<&Path>,
        mark: usize,
    ) -> HopWrites {
        let Some(roots) = self.pack.clone() else {
            return HopWrites::default();
        };
        // The hop's report: each claim is resolved against the hop's working
        // directory and judged. Outside the pack it is refused; a file claim
        // is kept by path for the loop below; compensations wait their turn.
        let here = hop_cwd
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let resolve = |p: &Path| -> PathBuf {
            if p.is_absolute() {
                normalize_root(&p.to_string_lossy())
            } else {
                normalize_root(&here.join(p).to_string_lossy())
            }
        };
        let mut claimed: std::collections::BTreeMap<PathBuf, protocol::Claim> =
            std::collections::BTreeMap::new();
        let mut claimed_created: Vec<PathBuf> = Vec::new();
        let mut compensations: Vec<protocol::Claim> = Vec::new();
        let mut claim_warnings: Vec<String> = Vec::new();
        let mut outside_claims: Vec<PathBuf> = Vec::new();
        for c in claims {
            match &c {
                protocol::Claim::Compensate { .. } => compensations.push(c),
                _ => {
                    let p = resolve(c.path().expect("a file claim has a path"));
                    if !inside_roots(&p, &roots) {
                        outside_claims.push(p);
                        continue;
                    }
                    if let protocol::Claim::Create { .. } = &c {
                        claimed_created.push(p);
                    } else {
                        claimed.insert(p, c);
                    }
                }
            }
        }
        let cap = watch_cap();
        let mut after = std::collections::BTreeMap::new();
        let mut partial = watch.partial;
        for r in &roots {
            let s = scan(r, 64, cap);
            partial |= !s.complete;
            after.extend(s.seen);
        }
        let mut writes = HopWrites::default();
        // A symbolic link the hop retargeted, replaced or deleted comes
        // back from the journal alone: its target is all there is to keep.
        let mut links: std::collections::BTreeMap<PathBuf, PathBuf> =
            std::collections::BTreeMap::new();
        // Directories the hop deleted, or replaced with something else,
        // with the mode each had: made again on rollback, before the
        // entries for what was inside them.
        let mut dirs_gone: Vec<(PathBuf, u32)> = Vec::new();
        let mut dirs_replaced: Vec<(PathBuf, u32)> = Vec::new();
        let mut unseen_created = 0usize;
        for (p, meta) in &after {
            match watch.inside.get(p) {
                None => {
                    // A scan that stopped at the cap cannot tell a file the
                    // hop made from one it never reached: nothing is removed
                    // on a guess, unless the hop itself reported making it.
                    if partial && !claimed_created.contains(p) {
                        unseen_created += 1;
                        continue;
                    }
                    // A path that reads as inside the pack but leads outside
                    // it (through a link) is not the pack's to remove. A link
                    // the hop made is judged by where it sits, not where it
                    // points: the entry is the pack's, whatever it leads to.
                    if !entry_inside(p, meta, &roots) {
                        writes.outside.push(p.clone());
                        continue;
                    }
                    writes.created.push(p.clone());
                }
                Some(before) => match (before, meta) {
                    // A directory's mode changing is not a change to watch.
                    (Seen::Dir { .. }, Seen::Dir { .. }) => {}
                    (Seen::Dir { mode }, _) => dirs_replaced.push((p.clone(), *mode)),
                    (
                        Seen::Link {
                            target: was,
                            through: led_to,
                        },
                        Seen::Link {
                            target: now,
                            through: leads_to,
                        },
                    ) => {
                        if was != now {
                            links.insert(p.clone(), was.clone());
                            writes.modified.push(p.clone());
                        } else if led_to != leads_to {
                            // Written through the link. Inside the pack the
                            // file is seen under its own path; outside, the
                            // write left the pack and is reported.
                            let landing = journal::resolve_symlink_chain(p);
                            if !inside_roots(&landing, &roots) {
                                writes.outside.push(landing);
                            }
                        }
                    }
                    (Seen::Link { target, .. }, _) => {
                        links.insert(p.clone(), target.clone());
                        writes.modified.push(p.clone());
                    }
                    (Seen::File { .. }, Seen::File { .. }) if before != meta => {
                        writes.modified.push(p.clone());
                    }
                    // Same mtime, size and mode: a tinned file is told by its hash.
                    (Seen::File { .. }, Seen::File { .. }) => {
                        if let Some(st) = watch.tinned.get(p) {
                            if st.link.is_none()
                                && journal::sha256_file(p).ok().as_deref()
                                    != Some(st.sha256.as_str())
                            {
                                writes.modified.push(p.clone());
                            }
                        }
                    }
                    (Seen::File { .. }, _) => writes.modified.push(p.clone()),
                },
            }
        }
        for (p, meta) in &watch.inside {
            if after.contains_key(p) {
                continue;
            }
            // With a scan that stopped at the cap, "not seen" is not "gone":
            // the disk is asked.
            if partial && std::fs::symlink_metadata(p).is_ok() {
                continue;
            }
            match meta {
                Seen::Dir { mode } => dirs_gone.push((p.clone(), *mode)),
                Seen::Link { target, .. } => {
                    links.insert(p.clone(), target.clone());
                    writes.deleted.push(p.clone());
                }
                Seen::File { .. } => writes.deleted.push(p.clone()),
            }
        }
        for (w, before) in &watch.outside {
            let now = scan(w, 3, 5_000).seen;
            for (p, meta) in &now {
                if meta.is_dir() || entry_inside(p, meta, &roots) {
                    continue;
                }
                match before.get(p) {
                    None => writes.outside.push(p.clone()),
                    Some(b) if b != meta => writes.outside.push(p.clone()),
                    Some(_) => {}
                }
            }
            for (p, meta) in before {
                if !meta.is_dir() && !now.contains_key(p) && !entry_inside(p, meta, &roots) {
                    writes.outside.push(p.clone());
                }
            }
        }
        // A `create` the scan did not see at all (past the cap it may not
        // have reached the path) counts when the file is there now and the
        // scan before the hop did not have it; one the scan saw before the
        // hop is a false claim.
        for p in &claimed_created {
            if writes.created.contains(p) {
                continue;
            }
            if watch.inside.contains_key(p) {
                claim_warnings.push(format!(
                    "the hop reported creating {}, which existed before it ran; the claim is dropped",
                    p.display()
                ));
            } else if std::fs::symlink_metadata(p).is_ok()
                && !after.contains_key(p)
                && entry_inside(p, &Seen::Dir { mode: 0 }, &roots)
            {
                writes.created.push(p.clone());
            }
        }
        for p in outside_claims {
            claim_warnings.push(format!(
                "the hop reported a change to {}, which is outside the pack; nothing is journaled for it",
                p.display()
            ));
            writes.outside.push(p);
        }
        writes.outside.sort();
        writes.outside.dedup();
        if unseen_created > 0 {
            watch.warnings.push(format!(
                "hop watching saw only part of the pack, so the {unseen_created} path{} it found after the hop and had not seen before are left as they are, not journaled as created",
                if unseen_created == 1 { "" } else { "s" }
            ));
        }
        let compensated = self.compensating > 0;
        for p in &writes.created {
            self.seq += 1;
            self.executed += 1;
            let _ = self.journal.record_with(
                self.seq,
                Op::HopCreated {
                    path: p.clone(),
                    left: after.get(p).and_then(Seen::left),
                },
                None,
                compensated,
            );
        }
        let touched_set = if watch.touches.is_empty() {
            None
        } else {
            tin::globs(&watch.touches).ok()
        };
        // Files changed outside every `touches` glob, with their path
        // relative to the root, for the grouped warning after the loop.
        let mut uncovered: Vec<(PathBuf, String)> = Vec::new();
        for (p, change) in writes
            .modified
            .iter()
            .map(|p| (p, "modified"))
            .chain(writes.deleted.iter().map(|p| (p, "deleted")))
        {
            self.seq += 1;
            self.executed += 1;
            // The before-state the journal can restore from: a link's
            // target, or a file's bytes in the tin with its mode and time.
            // The hop's own word about this path, when it gave one.
            let claim = claimed.remove(p);
            if let (
                Some(
                    protocol::Claim::Write {
                        before: protocol::Claimed::Hash(h),
                        ..
                    }
                    | protocol::Claim::Delete {
                        before: protocol::Claimed::Hash(h),
                        ..
                    },
                ),
                Some(st),
            ) = (&claim, watch.tinned.get(p))
            {
                if st.link.is_none() && *h != st.sha256 {
                    claim_warnings.push(format!(
                        "the hop's claim about the bytes of {} before it ran does not match what the tin kept; the tin's copy is used",
                        p.display()
                    ));
                }
            }
            let kept = match (links.get(p), watch.tinned.get(p)) {
                (Some(target), _) => Some((
                    journal::Before::Symlink {
                        target: target.clone(),
                    },
                    "link",
                )),
                (None, Some(st)) if st.link.is_none() => Some((
                    journal::Before::File {
                        snapshot: st.object.clone(),
                        sha256: st.sha256.clone(),
                        bytes: st.bytes,
                        mode: st.mode,
                        mtime_ns: st.mtime_ns,
                        git: st.git.clone(),
                    },
                    if st.git.is_some() {
                        "git"
                    } else if tin::is_sealed(&st.object) {
                        "sealed"
                    } else {
                        "tin"
                    },
                )),
                // Not under touches: reversible all the same when the hop
                // kept the bytes before and they hash as it claims.
                (None, None) => match claim.as_ref().and_then(saved_of) {
                    Some((saved, before_hash)) => {
                        // The copy is named as the tool wrote it: relative
                        // to the hop's working directory, or absolute.
                        let saved = resolve(saved);
                        let stashed = if seal::should_seal_here(p) {
                            tin::prepare_sealed(&saved).and_then(tin::Staged::commit)
                        } else {
                            tin::stash(&saved)
                        };
                        match stashed {
                            Ok(st) if before_hash.is_none_or(|h| *h == st.sha256) => Some((
                                journal::Before::File {
                                    snapshot: st.object.clone(),
                                    sha256: st.sha256.clone(),
                                    bytes: st.bytes,
                                    mode: None,
                                    mtime_ns: None,
                                    git: None,
                                },
                                "reported",
                            )),
                            Ok(_) => {
                                claim_warnings.push(format!(
                                    "the hop reported its change to {} with a copy of the bytes before, but the copy does not hash to the before it claims; the change stays irreversible",
                                    p.display()
                                ));
                                None
                            }
                            Err(e) => {
                                claim_warnings.push(format!(
                                    "the hop reported its change to {} with a copy of the bytes before, which could not be kept ({e}); the change stays irreversible",
                                    p.display()
                                ));
                                None
                            }
                        }
                    }
                    None => {
                        if claim.is_some() {
                            claim_warnings.push(format!(
                                "the hop reported its change to {} but kept no copy of the bytes before it; the change stays irreversible",
                                p.display()
                            ));
                        }
                        None
                    }
                },
                _ => None,
            };
            let left = after.get(p).and_then(Seen::left);
            if let Some((before, how)) = kept {
                let _ = self.journal.record_with_before(
                    self.seq,
                    Op::HopChanged {
                        path: p.clone(),
                        change: change.to_string(),
                        tinned: true,
                        kept: Some(how.to_string()),
                        left,
                    },
                    vec![journal::BeforeState {
                        path: p.clone(),
                        before,
                    }],
                    None,
                    compensated,
                );
                continue;
            }
            if !compensated {
                self.irreversible += 1;
            }
            // The glob that would have kept it, when the hop named some and
            // none covered this file: said once per folder, after the loop.
            if let Some(set) = &touched_set {
                if let Some(rel) = roots.iter().find_map(|r| p.strip_prefix(r).ok()) {
                    if !set.is_match(rel) {
                        uncovered.push((p.clone(), rel.to_string_lossy().replace('\\', "/")));
                    }
                }
            }
            let _ = self.journal.record_with(
                self.seq,
                Op::HopChanged {
                    path: p.clone(),
                    change: change.to_string(),
                    tinned: false,
                    kept: None,
                    left,
                },
                None,
                compensated,
            );
        }
        if !uncovered.is_empty() {
            let run = self
                .journal
                .run_dir()
                .and_then(|d| d.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "<run>".to_string());
            watch.warnings.extend(uncovered_warnings(&uncovered, &run));
        }
        // Directories last, deepest first, so a rollback (newest first) makes
        // a parent before its children and both before the files inside.
        dirs_gone.sort_by_key(|(p, _)| std::cmp::Reverse(p.components().count()));
        dirs_replaced.sort_by_key(|(p, _)| std::cmp::Reverse(p.components().count()));
        for (p, mode, change) in dirs_gone
            .iter()
            .map(|(p, m)| (p, *m, "deleted"))
            .chain(dirs_replaced.iter().map(|(p, m)| (p, *m, "replaced")))
        {
            self.seq += 1;
            self.executed += 1;
            let _ = self.journal.record_with_before(
                self.seq,
                Op::HopChanged {
                    path: p.clone(),
                    change: change.to_string(),
                    tinned: true,
                    kept: Some("dir".to_string()),
                    left: None,
                },
                vec![journal::BeforeState {
                    path: p.clone(),
                    before: journal::Before::Directory {
                        mode: if cfg!(unix) { Some(mode) } else { None },
                        mtime_ns: None,
                    },
                }],
                None,
                compensated,
            );
        }
        // The hop's own undo, when it gave one: journaled as a compensation
        // like a block's, and the hop's irreversible ops it covers are
        // stamped compensated the same way.
        if !compensations.is_empty() {
            for c in compensations {
                let protocol::Claim::Compensate {
                    label, undo, cwd, ..
                } = c
                else {
                    continue;
                };
                let cwd = cwd.map(|d| resolve(&d)).or_else(|| Some(here.clone()));
                let comp = journal::Compensation {
                    source: String::new(),
                    state_name: None,
                    state: serde_json::Value::Null,
                    line: 0,
                    label: Some(label.clone()),
                    command: Some(undo),
                    cwd,
                };
                self.seq += 1;
                let _ = self.journal.record_with(
                    self.seq,
                    Op::Compensate {
                        line: 0,
                        label: Some(label),
                    },
                    Some(comp),
                    false,
                );
            }
            let stamped = self.journal.mark_compensated_since(mark);
            self.irreversible = self.irreversible.saturating_sub(stamped);
        }
        // The journal refers to the tin's objects now: the pending list
        // has done its work.
        if let Some(dir) = self.journal.run_dir() {
            let _ = std::fs::remove_file(dir.join(tin::PENDING));
        }
        self.late_warnings = std::mem::take(&mut watch.warnings);
        self.late_claims = claim_warnings;
        writes
    }

    /// Warnings the end of a hop's watch produced, for the hop to print.
    pub fn take_late_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.late_warnings)
    }

    /// What a hop's report could not be used for, for the hop to print.
    pub fn take_claim_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.late_claims)
    }

    /// Where the hop about to run may report what it changes (the burn
    /// protocol): `hops/<seq>.jsonl` in the run directory, for the hop
    /// just journaled. None in a dry run.
    pub fn hop_journal(&self) -> Option<PathBuf> {
        if self.mode == Mode::DryRun {
            return None;
        }
        let dir = self.journal.run_dir()?;
        let hops = dir.join("hops");
        std::fs::create_dir_all(&hops).ok()?;
        Some(hops.join(format!("{}.jsonl", self.seq)))
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
        // A Windows device name is not a file: refused before it is journaled,
        // with its own code, on the platform where it is one, pack or no pack.
        if cfg!(windows) {
            let named: Vec<&PathBuf> = match &op {
                Op::Write { path, .. }
                | Op::Append { path, .. }
                | Op::Delete { path }
                | Op::Mkdir { path } => vec![path],
                Op::Copy { to, .. } => vec![to],
                Op::Move { from, to } => vec![from, to],
                _ => Vec::new(),
            };
            for p in named {
                if let Some(device) = reserved_device_name(p) {
                    return Err(burn_error(format!(
                        "{} is the {device} device on Windows, not a file",
                        p.display()
                    ))
                    .code("E758")
                    .with_subject(p.display().to_string())
                    .with_hint("name the file something else; con, prn, aux, nul, conin$, conout$, com0-9 and lpt0-9 are devices here whatever the extension"));
                }
            }
        }
        if let Some(roots) = &self.pack {
            let outside: Vec<&PathBuf> = match &op {
                Op::Write { path, .. }
                | Op::Append { path, .. }
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
            // The plan says how many files each `touches` glob covers, so a
            // glob that covers none is seen before anything runs.
            let mut summary = op.describe();
            if let (
                Op::Proc {
                    touches,
                    touches_sealed,
                    ..
                },
                Some(roots),
            ) = (&op, &self.pack)
            {
                if !touches.is_empty() {
                    let counts = touches_counts(roots, touches, watch_cap());
                    let mut shown: Vec<String> = counts
                        .iter()
                        .map(|(g, n)| format!("{g}: {n} file{}", if *n == 1 { "" } else { "s" }))
                        .collect();
                    let sealed = sealed_count(roots, touches, touches_sealed, watch_cap());
                    if sealed > 0 {
                        shown.push(format!("{sealed} sealed"));
                    }
                    if let Some(at) = summary.rfind("  (touches ") {
                        summary.truncate(at);
                    }
                    summary.push_str(&format!("  (touches {})", shown.join(", ")));
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
                summary,
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
        if self.executed == 2 && std::env::var("CIG_INTERNAL_PANIC").as_deref() == Ok("after-burn")
        {
            // Test hook for the crash reporter: the process dies with a burn
            // journaled and a second one about to happen, the state a real
            // mid-run panic leaves on disk.
            panic!("deliberate test panic after a burn (CIG_INTERNAL_PANIC=after-burn)");
        }
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
