// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! The tin: the before-bytes of files a hop may change, kept outside the
//! project under `~/.cigscript/tin/objects/<sha256>`, one object per
//! distinct content. A hop that says what it may `touches` has every
//! existing file under those globs stashed here before it runs, and a file
//! it modified or deleted comes back from the tin on rollback, byte for
//! byte, hash-verified. Objects are shared by content, so a formatter that
//! changes three lines of a file stores that file about once.

use super::journal::sha256_file;
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn dir() -> PathBuf {
    super::runs::home_dir().join("tin")
}

fn objects_dir() -> PathBuf {
    dir().join("objects")
}

/// One path kept in the tin: a file's bytes as an object, with its mode
/// and modification time beside them in the journal; or a symbolic link,
/// kept as its target alone, since a link has no bytes of its own.
#[derive(Clone, Debug)]
pub struct Stashed {
    pub object: PathBuf,
    pub sha256: String,
    pub bytes: u64,
    pub mode: Option<u32>,
    pub mtime_ns: Option<u64>,
    /// The target, when the path is a symbolic link.
    pub link: Option<PathBuf>,
}

/// Keep a copy of `path` in the tin, named by its content. The same bytes
/// stashed twice cost nothing. The object is read-only; the file's own
/// mode and modification time travel in the journal and are put back on
/// restore. A symbolic link is kept as a link.
pub fn stash(path: &Path) -> io::Result<Stashed> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(Stashed {
            object: PathBuf::new(),
            sha256: String::new(),
            bytes: 0,
            mode: None,
            mtime_ns: None,
            link: Some(fs::read_link(path)?),
        });
    }
    let sha256 = sha256_file(path)?;
    let bucket = objects_dir().join(&sha256[..2]);
    fs::create_dir_all(&bucket)?;
    let object = bucket.join(&sha256);
    if !object.is_file() {
        let tmp = bucket.join(format!(".{}.{}", &sha256[..16], std::process::id()));
        fs::copy(path, &tmp)?;
        // The copy carries the file's mode, which may be read-only: the
        // flush goes through a handle that works either way.
        super::journal::sync_file(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o444))?;
        }
        fs::rename(&tmp, &object)?;
    }
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(meta.permissions().mode() & 0o7777)
    };
    #[cfg(not(unix))]
    let mode = None;
    Ok(Stashed {
        object,
        sha256,
        bytes: meta.len(),
        mode,
        mtime_ns: super::journal::mtime_ns(&meta),
        link: None,
    })
}

/// Is this snapshot path an object in the tin (shared by content), as
/// opposed to a run's own snapshot?
pub fn holds(path: &Path) -> bool {
    path.starts_with(objects_dir())
}

/// A set of `touches` globs, matched against a path relative to the pack
/// root it is under. `*` is the root's own files, `**` everything, `src/**`
/// a tree, `package.json` one file.
pub fn globs(patterns: &[String]) -> Result<GlobSet, String> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        let g = Glob::new(p.trim_start_matches("./"))
            .map_err(|e| format!("touches: `{p}` is not a glob: {e}"))?;
        b.add(g);
    }
    b.build().map_err(|e| format!("touches: {e}"))
}

/// Every object a live run still refers to, by path.
pub fn referenced() -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    for rec in super::runs::list().unwrap_or_default() {
        if let Ok(j) = super::journal::Journal::load(&rec.dir()) {
            for e in j.entries() {
                for b in &e.before {
                    match &b.before {
                        super::journal::Before::File { snapshot, .. }
                        | super::journal::Before::Dir { snapshot }
                            if holds(snapshot) =>
                        {
                            out.insert(snapshot.clone());
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

/// Every object in the tin, with its size.
pub fn objects() -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let Ok(buckets) = fs::read_dir(objects_dir()) else {
        return out;
    };
    for bucket in buckets.flatten() {
        let Ok(entries) = fs::read_dir(bucket.path()) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.len() == 64 && name.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Ok(m) = e.metadata() {
                    out.push((p, m.len()));
                }
            }
        }
    }
    out.sort();
    out
}

/// What the tin holds: objects and bytes.
pub fn du() -> (usize, u64) {
    let all = objects();
    let bytes = all.iter().map(|(_, b)| b).sum();
    (all.len(), bytes)
}

/// Re-hash every object against its name. Returns one line per problem.
pub fn verify() -> Vec<String> {
    let mut problems = Vec::new();
    for (p, _) in objects() {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        match sha256_file(&p) {
            Ok(actual) if actual == name => {}
            Ok(actual) => problems.push(format!(
                "{} does not hash to its name (sha256 {} on disk); a restore from it would be refused",
                p.display(),
                &actual[..12]
            )),
            Err(e) => problems.push(format!("{}: cannot read: {e}", p.display())),
        }
    }
    problems
}

/// Remove every object no live run refers to. Returns (removed, bytes).
pub fn gc() -> (usize, u64) {
    let keep = referenced();
    let mut removed = 0;
    let mut bytes = 0;
    for (p, size) in objects() {
        if keep.contains(&p) {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o644));
        }
        if fs::remove_file(&p).is_ok() {
            removed += 1;
            bytes += size;
        }
    }
    (removed, bytes)
}

/// The names hop watching never looks into: build and dependency trees
/// that are rebuilt, not restored, plus whatever a pack root's
/// `.cigignore` lists (one glob per line, `#` comments, relative to the root).
pub const SKIPPED_ALWAYS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".cigscript",
    ".venv",
    "venv",
    "__pycache__",
    ".gradle",
    ".terraform",
];

/// The `.cigignore` of a pack root, as a glob set; `None` when there is none.
pub fn ignore_set(root: &Path) -> Option<GlobSet> {
    let text = fs::read_to_string(root.join(".cigignore")).ok()?;
    let mut b = GlobSetBuilder::new();
    let mut any = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let pat = line.trim_start_matches("./").trim_end_matches('/');
        for p in [pat.to_string(), format!("{pat}/**")] {
            if let Ok(g) = Glob::new(&p) {
                b.add(g);
                any = true;
            }
        }
    }
    if !any {
        return None;
    }
    b.build().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touches_globs_match_relative_paths() {
        let g = globs(&["src/**".to_string(), "package.json".to_string()]).unwrap();
        assert!(g.is_match("src/a/b.ts"));
        assert!(g.is_match("package.json"));
        assert!(!g.is_match("README.md"));
        let all = globs(&["**".to_string()]).unwrap();
        assert!(all.is_match("deep/er/file"));
        assert!(globs(&["[".to_string()]).is_err());
    }
}
