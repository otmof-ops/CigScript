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

fn io_err(name: &str, path: &Path, e: io::Error, s: Span) -> Diagnostic {
    runtime(format!("{name}: {}: {}", path.display(), describe_io(&e)))
        .code("E508")
        .at(s)
}

pub(crate) fn describe_io(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "no such file or directory".to_string(),
        io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        io::ErrorKind::AlreadyExists => "already exists".to_string(),
        _ => e.to_string(),
    }
}

fn read_text(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.read_text", s)?;
    fs::read_to_string(&p)
        .map(Value::str)
        .map_err(|e| io_err("fs.read_text", &p, e, s))
}

fn read_lines(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.read_lines", s)?;
    let text = fs::read_to_string(&p).map_err(|e| io_err("fs.read_lines", &p, e, s))?;
    Ok(Value::list(text.lines().map(Value::str).collect()))
}

fn exists(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Bool(path_arg(a, 0, "fs.exists", s)?.exists()))
}

fn is_file(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Bool(path_arg(a, 0, "fs.is_file", s)?.is_file()))
}

fn is_dir(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Bool(path_arg(a, 0, "fs.is_dir", s)?.is_dir()))
}

fn size(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.size", s)?;
    fs::metadata(&p)
        .map(|m| Value::Int(m.len() as i64))
        .map_err(|e| io_err("fs.size", &p, e, s))
}

fn modified_ms(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.modified_ms", s)?;
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
fn list(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.list", s)?;
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
fn glob(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let pattern = expect_str(a, 0, "fs.glob", s)?;
    let paths =
        glob_paths(pattern).map_err(|e| runtime(format!("fs.glob: {e}")).code("E508").at(s))?;
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

fn write_text(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.write_text", s)?;
    let text = expect_str(a, 1, "fs.write_text", s)?;
    if i.effect("fs.write_text", Op::Write { path: p.clone() }, s)? == Decision::Execute {
        if let Some(parent) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| io_err("fs.write_text", parent, e, s))?;
        }
        fs::write(&p, text).map_err(|e| io_err("fs.write_text", &p, e, s))?;
    }
    Ok(Value::Int(text.len() as i64))
}

fn append_text(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.append_text", s)?;
    let text = expect_str(a, 1, "fs.append_text", s)?;
    if i.effect("fs.append_text", Op::Append { path: p.clone() }, s)? == Decision::Execute {
        use std::io::Write;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .map_err(|e| io_err("fs.append_text", &p, e, s))?;
        f.write_all(text.as_bytes())
            .map_err(|e| io_err("fs.append_text", &p, e, s))?;
    }
    Ok(Value::Int(text.len() as i64))
}

fn mkdir(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.mkdir", s)?;
    if p.is_dir() {
        return Ok(Value::Bool(false));
    }
    if i.effect("fs.mkdir", Op::Mkdir { path: p.clone() }, s)? == Decision::Execute {
        fs::create_dir_all(&p).map_err(|e| io_err("fs.mkdir", &p, e, s))?;
    }
    Ok(Value::Bool(true))
}

fn rm(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let p = path_arg(a, 0, "fs.rm", s)?;
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
    let op = Op::Move {
        from: from.clone(),
        to: to.clone(),
    };
    if i.effect("fs.mv", op, s)? == Decision::Execute {
        if let Some(parent) = to.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| io_err("fs.mv", parent, e, s))?;
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
                return Err(io_err("fs.mv", &from, e, s));
            }
        }
    }
    Ok(Value::Null)
}
