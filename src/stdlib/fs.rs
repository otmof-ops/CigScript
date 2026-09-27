// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! File system access. Reads are free; anything that changes the disk goes
//! through the kernel.

use super::b;
use crate::burn::{journal::copy_tree, Decision, Op};
use crate::diagnostics::{runtime, Diagnostic};
use crate::interp::Interp;
use crate::syntax::span::Span;
use crate::value::{expect_str, Builtin, Effect, Value};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub static TABLE: &[Builtin] = &[
    b("fs.read_text", Effect::Read, 1, Some(1), read_text),
    b("fs.read_lines", Effect::Read, 1, Some(1), read_lines),
    b("fs.exists", Effect::Read, 1, Some(1), exists),
    b("fs.is_file", Effect::Read, 1, Some(1), is_file),
    b("fs.is_dir", Effect::Read, 1, Some(1), is_dir),
    b("fs.size", Effect::Read, 1, Some(1), size),
    b("fs.modified_ms", Effect::Read, 1, Some(1), modified_ms),
    b("fs.list", Effect::Read, 1, Some(1), list),
    b("fs.glob", Effect::Read, 1, Some(1), glob),
    b("fs.cwd", Effect::Read, 0, Some(0), cwd),
    b("fs.home", Effect::Read, 0, Some(0), home),
    b("fs.write_text", Effect::Write, 2, Some(2), write_text),
    b("fs.append_text", Effect::Write, 2, Some(2), append_text),
    b("fs.mkdir", Effect::Write, 1, Some(1), mkdir),
    b("fs.rm", Effect::Write, 1, Some(1), rm),
    b("fs.cp", Effect::Write, 2, Some(2), cp),
    b("fs.mv", Effect::Write, 2, Some(2), mv),
];

fn path_arg(a: &[Value], i: usize, name: &str, s: Span) -> Result<PathBuf, Diagnostic> {
    let p = expect_str(a, i, name, s)?;
    if p.is_empty() {
        return Err(runtime(format!("{name}: path is empty")).at(s));
    }
    Ok(PathBuf::from(p))
}

/// Refuse a copy or move whose destination is the source itself (the copy
/// would truncate the file to nothing) or lies inside it (the copy would
/// have to copy its own output).
fn same_or_inside(name: &str, from: &Path, to: &Path, s: Span) -> Result<(), Diagnostic> {
    let a = crate::burn::scope_path(from);
    let b = crate::burn::scope_path(to);
    #[cfg(unix)]
    let same_inode = {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(from), fs::metadata(to)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    };
    #[cfg(not(unix))]
    let same_inode = false;
    if a == b || same_inode {
        return Err(runtime(format!(
            "{name}: {} and {} are the same file",
            from.display(),
            to.display()
        ))
        .code("E508")
        .at(s)
        .with_subject(from.display().to_string())
        .with_hint("give the destination a different name; nothing was changed"));
    }
    if b.starts_with(&a) && from.is_dir() {
        return Err(runtime(format!(
            "{name}: {} is inside {}, the directory being {}",
            to.display(),
            from.display(),
            if name == "fs.mv" { "moved" } else { "copied" }
        ))
        .code("E508")
        .at(s)
        .with_subject(to.display().to_string())
        .with_hint("choose a destination outside the source; nothing was changed"));
    }
    Ok(())
}

fn io_err(name: &str, path: &Path, e: io::Error, s: Span) -> Diagnostic {
    runtime(format!("{name}: {}: {}", path.display(), describe_io(&e)))
        .code("E508")
        .at(s)
        .with_subject(path.display().to_string())
}

pub(crate) fn describe_io(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "no such file or directory".to_string(),
        io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        io::ErrorKind::AlreadyExists => "already exists".to_string(),
        _ => e.to_string(),
    }
}

/// Refuse to slurp a file larger than the allocation ceiling (E514).
pub(crate) fn check_read_size(name: &str, p: &std::path::Path, s: Span) -> Result<(), Diagnostic> {
    if let Ok(m) = fs::metadata(p) {
        crate::value::check_alloc(m.len() as u128, &format!("{name} of {}", p.display()), s)?;
    }
    Ok(())
}

// ----- the ghost filesystem: dry-run reads consult it before the disk ---------

/// A read of a path that a simulated op removed earlier in this dry-run.
pub(crate) fn ghost_removed(name: &str, p: &Path, seq: u64, by: &str, s: Span) -> Diagnostic {
    runtime(format!(
        "{name}: {}: removed earlier in this dry-run by op {seq} ({by})",
        p.display()
    ))
    .code("E520")
    .at(s)
    .with_subject(p.display().to_string())
    .with_hint("read it before that op, or do not remove it; the plan shows the order")
}

/// The bytes of `p` as the dry-run sees them: `Some` from the ghost, `None`
/// to read the disk. A ghost-removed path is E520; a ghost directory E508.
pub(crate) fn ghost_read(
    i: &Interp,
    name: &str,
    p: &Path,
    s: Span,
) -> Result<Option<Vec<u8>>, Diagnostic> {
    use crate::burn::ghost::Read;
    let Some(g) = i.ghost() else {
        return Ok(None);
    };
    match g.read(p) {
        Read::Content(b) => Ok(Some(b)),
        Read::Absent { seq, by } => Err(ghost_removed(name, p, seq, &by, s)),
        Read::IsDir => Err(runtime(format!("{name}: {}: is a directory", p.display()))
            .code("E508")
            .at(s)
            .with_subject(p.display().to_string())),
        Read::Passthrough => Ok(None),
    }
}

pub(crate) fn ghost_text(
    i: &Interp,
    name: &str,
    p: &Path,
    s: Span,
) -> Result<Option<String>, Diagnostic> {
    Ok(ghost_read(i, name, p, s)?.map(|b| String::from_utf8_lossy(&b).to_string()))
}

/// `Some(kind)` when the ghost decides; `None` to ask the disk. The kind is
/// `Some(true)` for a directory, `Some(false)` for a file, `None` for absent.
pub(crate) fn ghost_kind(i: &Interp, p: &Path) -> Option<Option<bool>> {
    use crate::burn::ghost::Stat;
    match i.ghost()?.stat(p) {
        Stat::Absent { .. } => Some(None),
        Stat::File { .. } => Some(Some(false)),
        Stat::Dir => Some(Some(true)),
        Stat::Passthrough => None,
    }
}

fn read_text(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.read_text", s)?;
    if let Some(t) = ghost_text(i, "fs.read_text", &p, s)? {
        return Ok(Value::str(t));
    }
    check_read_size("fs.read_text", &p, s)?;
    fs::read_to_string(&p)
        .map(Value::str)
        .map_err(|e| io_err("fs.read_text", &p, e, s))
}

fn read_lines(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.read_lines", s)?;
    let text = match ghost_text(i, "fs.read_lines", &p, s)? {
        Some(t) => t,
        None => {
            check_read_size("fs.read_lines", &p, s)?;
            fs::read_to_string(&p).map_err(|e| io_err("fs.read_lines", &p, e, s))?
        }
    };
    Ok(Value::list(text.lines().map(Value::str).collect()))
}

fn exists(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.exists", s)?;
    if let Some(kind) = ghost_kind(i, &p) {
        return Ok(Value::Bool(kind.is_some()));
    }
    // A dangling symlink exists as far as `fs.rm` and `fs.mv` are concerned.
    Ok(Value::Bool(fs::symlink_metadata(&p).is_ok()))
}

fn is_file(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.is_file", s)?;
    if let Some(kind) = ghost_kind(i, &p) {
        return Ok(Value::Bool(kind == Some(false)));
    }
    Ok(Value::Bool(p.is_file()))
}

fn is_dir(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.is_dir", s)?;
    if let Some(kind) = ghost_kind(i, &p) {
        return Ok(Value::Bool(kind == Some(true)));
    }
    Ok(Value::Bool(p.is_dir()))
}

fn size(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.size", s)?;
    if let Some(b) = ghost_read(i, "fs.size", &p, s)? {
        return Ok(Value::Int(b.len() as i64));
    }
    fs::metadata(&p)
        .map(|m| Value::Int(m.len() as i64))
        .map_err(|e| io_err("fs.size", &p, e, s))
}

fn modified_ms(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.modified_ms", s)?;
    if ghost_read(i, "fs.modified_ms", &p, s)?.is_some() {
        // A pretend write happened just now.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        return Ok(Value::Int(now));
    }
    let m = fs::metadata(&p).map_err(|e| io_err("fs.modified_ms", &p, e, s))?;
    let t = m
        .modified()
        .map_err(|e| io_err("fs.modified_ms", &p, e, s))?;
    let ms = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(Value::Int(ms))
}

/// Entries of a directory as full paths, sorted by name.
fn list(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.list", s)?;
    if let Some(g) = i.ghost() {
        if let crate::burn::ghost::Stat::Absent { seq, by } = g.stat(&p) {
            return Err(ghost_removed("fs.list", &p, seq, &by, s));
        }
        if let Some(names) = g.list(&p) {
            // Keep the caller's spelling: relative in, relative out.
            let shown: Vec<Value> = names
                .iter()
                .map(|n| {
                    let rel = if p.is_absolute() {
                        n.clone()
                    } else {
                        n.strip_prefix(g.cwd())
                            .map(Path::to_path_buf)
                            .unwrap_or_else(|_| n.clone())
                    };
                    Value::str(rel.to_string_lossy())
                })
                .collect();
            return Ok(Value::list(shown));
        }
    }
    let mut names: Vec<PathBuf> = fs::read_dir(&p)
        .map_err(|e| io_err("fs.list", &p, e, s))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    names.sort();
    Ok(Value::list(
        names
            .into_iter()
            .map(|n| Value::str(n.to_string_lossy()))
            .collect(),
    ))
}

/// Recursive glob (`**` supported) relative to the current directory or an
/// absolute root, sorted.
fn glob(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let pattern = expect_str(a, 0, "fs.glob", s)?;
    let mut paths =
        glob_paths(pattern).map_err(|e| runtime(format!("fs.glob: {e}")).code("E508").at(s))?;
    if let Some(g) = i.ghost() {
        if !g.is_empty() {
            let matcher = globset::GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .map_err(|e| runtime(format!("fs.glob: {e}")).code("E508").at(s))?
                .compile_matcher();
            let absolute = Path::new(pattern).is_absolute();
            paths.retain(|p| !g.is_removed(p));
            for live in g.live_paths() {
                let candidate = if absolute {
                    live.clone()
                } else {
                    match live.strip_prefix(g.cwd()) {
                        Ok(rel) => rel.to_path_buf(),
                        Err(_) => continue,
                    }
                };
                if matcher.is_match(&candidate) && !paths.contains(&candidate) {
                    paths.push(candidate);
                }
            }
            paths.sort();
        }
    }
    Ok(Value::list(
        paths
            .into_iter()
            .map(|p| Value::str(p.to_string_lossy()))
            .collect(),
    ))
}

pub(crate) fn glob_paths(pattern: &str) -> Result<Vec<PathBuf>, String> {
    let matcher = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|e| e.to_string())?
        .compile_matcher();
    // Walk from the longest literal prefix so `src/**/*.rs` does not scan the
    // whole tree.
    let root = literal_prefix(pattern);
    let walk_root = if root.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        root
    };
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(&walk_root)
        .min_depth(0)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        let candidate = path.strip_prefix(".").unwrap_or(path);
        if matcher.is_match(candidate) {
            out.push(candidate.to_path_buf());
        }
    }
    out.sort();
    Ok(out)
}

fn literal_prefix(pattern: &str) -> PathBuf {
    let mut prefix = PathBuf::new();
    for part in Path::new(pattern).components() {
        let text = part.as_os_str().to_string_lossy();
        if text.contains(['*', '?', '[', '{']) {
            break;
        }
        prefix.push(part);
    }
    if prefix == Path::new(pattern) {
        // A pattern with no wildcards: walk its parent.
        return prefix.parent().map(Path::to_path_buf).unwrap_or_default();
    }
    prefix
}

fn cwd(_: &mut Interp, _: &[Value], s: Span) -> Result<Value, Diagnostic> {
    std::env::current_dir()
        .map(|p| Value::str(p.to_string_lossy()))
        .map_err(|e| runtime(format!("fs.cwd: {e}")).at(s))
}

fn home(_: &mut Interp, _: &[Value], _: Span) -> Result<Value, Diagnostic> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| h.to_string_lossy().to_string());
    Ok(home.map(Value::str).unwrap_or(Value::Null))
}

// ----- effects --------------------------------------------------------------------

/// In a dry-run, an op whose source does not exist (on the ghost or the
/// disk) would fail for real, so the plan says so instead of lying.
fn dry_run_source_check(i: &Interp, name: &str, p: &Path, s: Span) -> Result<(), Diagnostic> {
    let Some(g) = i.ghost() else {
        return Ok(());
    };
    if i.in_unlit() {
        return Ok(());
    }
    match g.stat(p) {
        crate::burn::ghost::Stat::Absent { seq, by } => Err(ghost_removed(name, p, seq, &by, s)),
        crate::burn::ghost::Stat::Passthrough if fs::symlink_metadata(p).is_err() => Err(io_err(
            name,
            p,
            io::Error::new(io::ErrorKind::NotFound, "no such file or directory"),
            s,
        )),
        _ => Ok(()),
    }
}

fn write_text(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.write_text", s)?;
    let text = expect_str(a, 1, "fs.write_text", s)?;
    if i.effect("fs.write_text", Op::Write { path: p.clone() }, s)? == Decision::Execute {
        if let Some(parent) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| io_err("fs.write_text", parent, e, s))?;
        }
        fs::write(&p, text).map_err(|e| io_err("fs.write_text", &p, e, s))?;
    } else {
        i.ghost_put(&p, text.as_bytes(), false);
    }
    Ok(Value::Int(text.len() as i64))
}

fn append_text(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.append_text", s)?;
    let text = expect_str(a, 1, "fs.append_text", s)?;
    if i.effect("fs.append_text", Op::Append { path: p.clone() }, s)? == Decision::Execute {
        use std::io::Write;
        // Like fs.write_text: the parent is created, so a log under a
        // directory that does not exist yet works, and the dry run (which
        // never minded) and the real run agree.
        if let Some(parent) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| io_err("fs.append_text", parent, e, s))?;
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .map_err(|e| io_err("fs.append_text", &p, e, s))?;
        f.write_all(text.as_bytes())
            .map_err(|e| io_err("fs.append_text", &p, e, s))?;
    } else {
        i.ghost_put(&p, text.as_bytes(), true);
    }
    Ok(Value::Int(text.len() as i64))
}

fn mkdir(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.mkdir", s)?;
    let already = match ghost_kind(i, &p) {
        Some(kind) => kind == Some(true),
        None => p.is_dir(),
    };
    if already {
        return Ok(Value::Bool(false));
    }
    if i.effect("fs.mkdir", Op::Mkdir { path: p.clone() }, s)? == Decision::Execute {
        fs::create_dir_all(&p).map_err(|e| io_err("fs.mkdir", &p, e, s))?;
    }
    Ok(Value::Bool(true))
}

fn rm(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.rm", s)?;
    dry_run_source_check(i, "fs.rm", &p, s)?;
    if i.effect("fs.rm", Op::Delete { path: p.clone() }, s)? == Decision::Execute {
        let meta = fs::symlink_metadata(&p).map_err(|e| io_err("fs.rm", &p, e, s))?;
        let r = if meta.is_dir() {
            fs::remove_dir_all(&p)
        } else {
            fs::remove_file(&p)
        };
        r.map_err(|e| io_err("fs.rm", &p, e, s))?;
    }
    Ok(Value::Null)
}

fn cp(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let from = path_arg(a, 0, "fs.cp", s)?;
    let to = path_arg(a, 1, "fs.cp", s)?;
    dry_run_source_check(i, "fs.cp", &from, s)?;
    same_or_inside("fs.cp", &from, &to, s)?;
    let op = Op::Copy {
        from: from.clone(),
        to: to.clone(),
    };
    if i.effect("fs.cp", op, s)? == Decision::Execute {
        let meta = fs::metadata(&from).map_err(|e| io_err("fs.cp", &from, e, s))?;
        if meta.is_dir() {
            copy_tree(&from, &to).map_err(|e| io_err("fs.cp", &to, e, s))?;
        } else {
            if let Some(parent) = to.parent().filter(|d| !d.as_os_str().is_empty()) {
                fs::create_dir_all(parent).map_err(|e| io_err("fs.cp", parent, e, s))?;
            }
            fs::copy(&from, &to).map_err(|e| io_err("fs.cp", &to, e, s))?;
        }
    }
    Ok(Value::Null)
}

fn mv(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let from = path_arg(a, 0, "fs.mv", s)?;
    let to = path_arg(a, 1, "fs.mv", s)?;
    if i.in_burn() && !i.in_unlit() && i.kernel.mode == crate::burn::Mode::Run {
        fs::symlink_metadata(&from).map_err(|e| io_err("fs.mv", &from, e, s))?;
    }
    dry_run_source_check(i, "fs.mv", &from, s)?;
    same_or_inside("fs.mv", &from, &to, s)?;
    let op = Op::Move {
        from: from.clone(),
        to: to.clone(),
    };
    if i.effect("fs.mv", op, s)? == Decision::Execute {
        if let Some(parent) = to.parent().filter(|d| !d.as_os_str().is_empty()) {
            if let Err(e) = fs::create_dir_all(parent) {
                i.kernel.op_failed();
                return Err(io_err("fs.mv", parent, e, s));
            }
        }
        if let Err(e) = fs::rename(&from, &to) {
            // Cross-device: copy then remove.
            if e.raw_os_error() == Some(18) {
                if from.is_dir() {
                    copy_tree(&from, &to).map_err(|e| io_err("fs.mv", &to, e, s))?;
                    fs::remove_dir_all(&from).map_err(|e| io_err("fs.mv", &from, e, s))?;
                } else {
                    fs::copy(&from, &to).map_err(|e| io_err("fs.mv", &to, e, s))?;
                    fs::remove_file(&from).map_err(|e| io_err("fs.mv", &from, e, s))?;
                }
            } else {
                // A rename is all or nothing: it did not happen, so the
                // journal must not say it did, or rollback would "move
                // back" a file that never left.
                i.kernel.op_failed();
                return Err(io_err("fs.mv", &from, e, s));
            }
        }
    }
    Ok(Value::Null)
}
