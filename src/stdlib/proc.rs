// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! Running other programs. `proc.run` never goes through a shell: the
//! command and its arguments are passed as given, so there is nothing to
//! inject into.

use super::b;
use crate::burn::{Decision, Op};
use crate::diagnostics::{runtime, type_error, Diagnostic, Kind};
use crate::interp::Interp;
use crate::syntax::span::Span;
use crate::value::{expect_str, opt_map, Builtin, Effect, Value};
use indexmap::IndexMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

pub static TABLE: &[Builtin] = &[
    b("proc.run", Effect::Proc, 1, Some(3), run),
    b("proc.which", Effect::Read, 1, Some(1), which),
];

/// Default wall-clock limit for a child process: ten minutes.
pub const DEFAULT_TIMEOUT_MS: u64 = 600_000;

/// `proc.run(cmd, args = [], opts = {})`
///
/// opts: `cwd`, `env` (map), `timeout_ms`, `stdin`, `check` (cough on a
/// non-zero exit). Returns `{code, out, err, duration_ms, timed_out,
/// simulated}`.
fn run(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let cmd = expect_str(a, 0, "proc.run", s)?.to_string();
    if cmd.trim().is_empty() {
        return Err(runtime("proc.run: command is empty").at(s));
    }
    let args: Vec<String> = match a.get(1) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::List(l)) => l.borrow().iter().map(|v| v.display()).collect(),
        Some(Value::Str(one)) => vec![one.to_string()],
        Some(other) => {
            return Err(type_error(format!(
                "proc.run: arguments must be a list, got {}",
                other.type_name()
            ))
            .at(s))
        }
    };
    let opts = opt_map(a, 2, "proc.run", s)?;
    let mut cwd: Option<PathBuf> = None;
    let mut env: Vec<(String, String)> = Vec::new();
    let mut timeout_ms = DEFAULT_TIMEOUT_MS;
    let mut stdin: Option<String> = None;
    let mut check = false;
    if let Some(opts) = &opts {
        for (k, v) in opts.borrow().iter() {
            match (k.as_str(), v) {
                ("cwd", Value::Str(p)) => cwd = Some(PathBuf::from(p.to_string())),
                ("env", Value::Map(m)) => {
                    for (ek, ev) in m.borrow().iter() {
                        env.push((ek.clone(), ev.display()));
                    }
                }
                ("timeout_ms", Value::Int(t)) if *t > 0 => timeout_ms = *t as u64,
                ("stdin", Value::Str(text)) => stdin = Some(text.to_string()),
                ("check", v) => check = v.truthy(),
                ("cwd" | "env" | "timeout_ms" | "stdin", other) => {
                    return Err(type_error(format!(
                        "proc.run: option `{k}` has the wrong type ({})",
                        other.type_name()
                    ))
                    .at(s))
                }
                (other, _) => {
                    return Err(runtime(format!("proc.run: unknown option `{other}`"))
                        .at(s)
                        .with_hint("options are cwd, env, timeout_ms, stdin, check"))
                }
            }
        }
    }

    let op = Op::Proc {
        cmd: cmd.clone(),
        args: args.clone(),
        cwd: cwd.clone(),
    };
    if i.effect("proc.run", op, s)? == Decision::Simulate {
        return Ok(result(0, "", "", 0, false, true));
    }

    let started = Instant::now();
    let mut command = Command::new(&cmd);
    command
        .args(&args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = &cwd {
        command.current_dir(dir);
    }
    for (k, v) in &env {
        command.env(k, v);
    }
    let mut child = command.spawn().map_err(|e| {
        let mut d = runtime(format!(
            "proc.run: could not start `{cmd}`: {}",
            super::fs::describe_io(&e)
        ))
        .code("E509")
        .at(s);
        if e.kind() == std::io::ErrorKind::NotFound {
            d = d.with_hint("is it installed and on PATH? proc.which(name) tells you");
        }
        d
    })?;

    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // A closed pipe is not an error: the child may not read stdin.
        let _ = pipe.write_all(text.as_bytes());
    }
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let (code, timed_out) = match child
        .wait_timeout(Duration::from_millis(timeout_ms))
        .map_err(|e| runtime(format!("proc.run: {e}")).at(s))?
    {
        Some(status) => (status.code().unwrap_or(-1), false),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            (-1, true)
        }
    };
    let out = String::from_utf8_lossy(&out_thread.join().unwrap_or_default()).to_string();
    let err = String::from_utf8_lossy(&err_thread.join().unwrap_or_default()).to_string();
    let duration_ms = started.elapsed().as_millis() as i64;

    if timed_out {
        return Err(runtime(format!(
            "proc.run: `{cmd}` exceeded {timeout_ms} ms and was killed"
        ))
        .code("E509")
        .at(s)
        .with_hint("raise it with {timeout_ms: ...}"));
    }
    if check && code != 0 {
        i.cough_payload = Some(result(code, &out, &err, duration_ms, timed_out, false));
        return Err(Diagnostic::new(
            Kind::Cough,
            format!(
                "`{cmd}` exited with code {code}{}",
                err.trim()
                    .lines()
                    .last()
                    .map(|l| format!(": {l}"))
                    .unwrap_or_default()
            ),
        )
        .code("E603")
        .at(s));
    }
    Ok(result(code, &out, &err, duration_ms, timed_out, false))
}

fn result(
    code: i32,
    out: &str,
    err: &str,
    duration_ms: i64,
    timed_out: bool,
    simulated: bool,
) -> Value {
    let mut m = IndexMap::new();
    m.insert("code".to_string(), Value::Int(code as i64));
    m.insert("out".to_string(), Value::str(out));
    m.insert("err".to_string(), Value::str(err));
    m.insert("duration_ms".to_string(), Value::Int(duration_ms));
    m.insert("timed_out".to_string(), Value::Bool(timed_out));
    m.insert("simulated".to_string(), Value::Bool(simulated));
    Value::map(m)
}

fn which(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let name = expect_str(a, 0, "proc.which", s)?;
    let Some(paths) = std::env::var_os("PATH") else {
        return Ok(Value::Null);
    };
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(Value::str(candidate.to_string_lossy()));
        }
    }
    Ok(Value::Null)
}
