// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! The tin: the before-bytes of files a hop may change, kept outside the
//! project under `~/.cigscript/tin/objects/<sha256>`, one object per
//! distinct content. A hop that says what it may `touches` has every
//! existing file under those globs stashed here before it runs, and a file
//! it modified or deleted comes back from the tin on rollback, byte for
//! byte, hash-verified. Objects are shared by content, so a formatter that
//! changes three lines of a file stores that file about once.
//!
//! The tin also keeps what a rollback displaces: a file a hop created, or
//! one it modified, is moved here before the rollback removes or rewrites
//! it, so a save made while the hop ran is never destroyed, only set aside
//! (`cig unburn <id> --give-back` puts it back).

use super::journal::sha256_file;
use globset::{Glob, GlobSet, GlobSetBuilder};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

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

/// A copy of a file waiting beside the objects under a temporary name,
/// hashed as it was copied: it becomes an object under that hash when it
/// is committed. The two steps let a run write down which objects it is
/// about to depend on before any of them exists under its final name, so
/// a `cig tin gc` in another terminal never removes one between the two.
#[derive(Debug)]
pub struct Staged {
    tmp: PathBuf,
    sha256: String,
    bytes: u64,
    mode: Option<u32>,
    mtime_ns: Option<u64>,
    link: Option<PathBuf>,
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn temp_name(tag: &str) -> PathBuf {
    objects_dir().join(format!(
        ".{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Copy `from` to `to`, hashing the bytes as they are written: the hash
/// names what was copied, not what the file held a moment earlier. A file
/// that changes while it is read gives a copy that is a moment newer than
/// the scan, and an object that still matches its name.
fn copy_hashing(from: &Path, to: &Path) -> io::Result<(String, u64)> {
    let mut src = fs::File::open(from)?;
    let mut dst = fs::File::create(to)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        dst.write_all(&buf[..n])?;
        total += n as u64;
    }
    dst.flush()?;
    Ok((hex::encode(hasher.finalize()), total))
}

fn mode_of(meta: &fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(meta.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

fn make_read_only(p: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(p, fs::Permissions::from_mode(0o444))
    }
    #[cfg(not(unix))]
    {
        let mut perms = fs::metadata(p)?.permissions();
        perms.set_readonly(true);
        fs::set_permissions(p, perms)
    }
}

/// Copy `path` beside the objects, hashed as it is copied. A symbolic link
/// needs no copy: it is kept as its target.
pub fn prepare(path: &Path) -> io::Result<Staged> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(Staged {
            tmp: PathBuf::new(),
            sha256: String::new(),
            bytes: 0,
            mode: None,
            mtime_ns: None,
            link: Some(fs::read_link(path)?),
        });
    }
    fs::create_dir_all(objects_dir())?;
    let tmp = temp_name("stash");
    let (sha256, bytes) = match copy_hashing(path, &tmp) {
        Ok(v) => v,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    };
    Ok(Staged {
        tmp,
        sha256,
        bytes,
        mode: mode_of(&meta),
        mtime_ns: super::journal::mtime_ns(&meta),
        link: None,
    })
}

impl Staged {
    /// Where the object will be once committed (empty for a link).
    pub fn object(&self) -> PathBuf {
        if self.link.is_some() {
            PathBuf::new()
        } else {
            objects_dir().join(&self.sha256[..2]).join(&self.sha256)
        }
    }

    /// Give the copy its name. An object already there is trusted only
    /// when it hashes to its name; one that does not is replaced, so a
    /// corrupt object is never reused by a later stash.
    pub fn commit(self) -> io::Result<Stashed> {
        if let Some(target) = self.link {
            return Ok(Stashed {
                object: PathBuf::new(),
                sha256: String::new(),
                bytes: 0,
                mode: None,
                mtime_ns: None,
                link: Some(target),
            });
        }
        let object = self.object();
        let bucket = object.parent().expect("an object has a bucket");
        fs::create_dir_all(bucket)?;
        let existing_ok = object.is_file()
            && fs::metadata(&object).map(|m| m.len()).ok() == Some(self.bytes)
            && sha256_file(&object).ok().as_deref() == Some(self.sha256.as_str());
        if existing_ok {
            let _ = fs::remove_file(&self.tmp);
        } else {
            // The flush goes through a handle that works on a read-only
            // file too, then the object is named and its directory synced,
            // so the journal entry that follows never points at a name the
            // disk has not learned yet.
            super::journal::sync_file(&self.tmp)?;
            make_read_only(&self.tmp)?;
            if object.exists() {
                lift_read_only(&object);
                let _ = fs::remove_file(&object);
            }
            fs::rename(&self.tmp, &object)?;
            let _ = super::journal::sync_dir_pub(bucket);
        }
        Ok(Stashed {
            object,
            sha256: self.sha256,
            bytes: self.bytes,
            mode: self.mode,
            mtime_ns: self.mtime_ns,
            link: None,
        })
    }
}

fn lift_read_only(p: &Path) {
    if let Ok(m) = fs::metadata(p) {
        let mut perms = m.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o644);
        }
        #[cfg(not(unix))]
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = fs::set_permissions(p, perms);
    }
}

/// Keep a copy of `path` in the tin, named by its content. The same bytes
/// stashed twice cost nothing. The object is read-only; the file's own
/// mode and modification time travel in the journal and are put back on
/// restore. A symbolic link is kept as a link.
pub fn stash(path: &Path) -> io::Result<Stashed> {
    prepare(path)?.commit()
}

/// Move a file into the tin: what a rollback is about to remove or rewrite
/// goes here first, so nothing a rollback displaces is lost. The file
/// leaves its place (a rename where the tin is on the same filesystem, a
/// copy and a remove where it is not) and becomes an object named by the
/// hash of what arrived.
pub fn take(path: &Path) -> io::Result<Stashed> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() {
        return Err(io::Error::other(format!(
            "{} is not a regular file; the tin keeps files",
            path.display()
        )));
    }
    fs::create_dir_all(objects_dir())?;
    let tmp = temp_name("take");
    match fs::rename(path, &tmp) {
        Ok(()) => {}
        // Across filesystems, or a file Windows will not let go of yet: copy.
        // A file another program holds open stays where it is for now; its
        // bytes are kept all the same, and the caller decides what a file
        // still at the path means (a restore writes over it in place).
        Err(_) => {
            copy_hashing(path, &tmp).inspect_err(|_| {
                let _ = fs::remove_file(&tmp);
            })?;
            let _ = fs::remove_file(path);
        }
    }
    lift_read_only(&tmp);
    let sha256 = sha256_file(&tmp)?;
    let bytes = fs::metadata(&tmp)?.len();
    let staged = Staged {
        tmp,
        sha256,
        bytes,
        mode: mode_of(&meta),
        mtime_ns: super::journal::mtime_ns(&meta),
        link: None,
    };
    staged.commit()
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

/// The file a running hop lists the objects it stashed in, inside its run
/// directory, before its journal refers to them. `referenced` reads it, so
/// a `cig tin gc` during the hop keeps what the hop's rollback will need.
pub const PENDING: &str = "tin.pending";

/// Every object a live run still refers to, by path: from its journal, and
/// from the pending list of a hop still running.
pub fn referenced() -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    for rec in super::runs::list().unwrap_or_default() {
        if let Ok(j) = super::journal::Journal::load(&rec.dir()) {
            for e in j.entries() {
                for b in e.before.iter().chain(e.took.iter()) {
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
        if let Ok(text) = fs::read_to_string(rec.dir().join(PENDING)) {
            for line in text.lines() {
                let line = line.trim();
                if !line.is_empty() {
                    out.insert(PathBuf::from(line));
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
        lift_read_only(&p);
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

    /// The tests below point the tin at a scratch home. Every test in this
    /// crate that sets `CIGSCRIPT_HOME` takes the same lock, since they
    /// share one process and one environment.
    fn with_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _g = super::super::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let old = std::env::var_os("CIGSCRIPT_HOME");
        std::env::set_var("CIGSCRIPT_HOME", tmp.path().join("home"));
        let out = f(tmp.path());
        match old {
            Some(v) => std::env::set_var("CIGSCRIPT_HOME", v),
            None => std::env::remove_var("CIGSCRIPT_HOME"),
        }
        out
    }

    #[test]
    fn an_object_is_named_by_the_hash_of_the_copy_and_a_corrupt_one_is_not_reused() {
        with_home(|root| {
            let f = root.join("f.txt");
            fs::write(&f, "content one\n").unwrap();
            let first = stash(&f).unwrap();
            assert_eq!(
                first.sha256,
                sha256_file(&first.object).unwrap(),
                "the object hashes to its name"
            );
            assert_eq!(fs::read_to_string(&first.object).unwrap(), "content one\n");
            // Corrupt the object in place: same size, other bytes.
            lift_read_only(&first.object);
            fs::write(&first.object, "content two\n").unwrap();
            assert_eq!(verify().len(), 1);
            // A later stash of the same content does not trust the name.
            let again = stash(&f).unwrap();
            assert_eq!(again.object, first.object);
            assert_eq!(fs::read_to_string(&again.object).unwrap(), "content one\n");
            assert!(verify().is_empty(), "the corrupt object was replaced");
        });
    }

    #[test]
    fn take_moves_a_file_into_the_tin_and_names_it_by_content() {
        with_home(|root| {
            let f = root.join("notes.txt");
            fs::write(&f, "my notes\n").unwrap();
            let taken = take(&f).unwrap();
            assert!(!f.exists(), "the file left its place");
            assert_eq!(fs::read_to_string(&taken.object).unwrap(), "my notes\n");
            assert_eq!(taken.sha256, sha256_file(&taken.object).unwrap());
            assert_eq!(taken.bytes, 9);
            // The same content taken twice is one object.
            fs::write(&f, "my notes\n").unwrap();
            let twice = take(&f).unwrap();
            assert_eq!(twice.object, taken.object);
            assert_eq!(objects().len(), 1);
        });
    }

    #[test]
    fn prepare_then_commit_leaves_no_object_until_commit() {
        with_home(|root| {
            let f = root.join("f.txt");
            fs::write(&f, "staged\n").unwrap();
            let staged = prepare(&f).unwrap();
            let object = staged.object();
            assert!(!object.exists(), "nothing under the final name yet");
            assert!(objects().is_empty(), "a temporary copy is not an object");
            let st = staged.commit().unwrap();
            assert_eq!(st.object, object);
            assert!(object.is_file());
            assert_eq!(objects().len(), 1);
        });
    }
}
