// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! Hops: running other programs.
//!
//! A child process is the one place the kernel performed nothing, saw
//! nothing and can undo nothing, so the boundary is where the logic is
//! hardest. Every hop here keeps the rules in `docs/HOPS.md`:
//!
//! * no shell unless you ask for one by name (`proc.shell`);
//! * stdout and stderr are read concurrently and never merged;
//! * every hop has a timeout, and a timed-out or signalled child produced
//!   *nothing* as far as the next step is concerned;
//! * the child runs in its own process group and is killed with it, so
//!   grandchildren do not outlive a timeout;
//! * exit codes are a contract (`ok: [0, 1]`), not a boolean;
//! * output is decoded explicitly: invalid UTF-8 is a diagnostic unless the
//!   script asks for `lossy`, `utf-16` or `latin1`;
//! * every failure has its own code (E55x) and names the hop.
//!
//! `proc.run` is the one call shape; `proc.text`, `proc.lines`,
//! `proc.json`, `proc.csv` and `proc.kv` parse stdout into a real value and
//! fail at the hop that produced a bad shape. `proc.pipe` connects stages
//! without a shell.

use super::b;
use crate::burn::{Decision, Op};
use crate::diagnostics::{runtime, type_error, Diagnostic};
use crate::interp::Interp;
use crate::syntax::span::Span;
use crate::value::{expect_str, opt_map, Builtin, Effect, Value};
use indexmap::IndexMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

pub static TABLE: &[Builtin] = &[
    b("proc.run", Effect::Proc, 1, Some(3), run),
    b("proc.text", Effect::Proc, 1, Some(3), text),
    b("proc.lines", Effect::Proc, 1, Some(3), lines),
    b("proc.json", Effect::Proc, 1, Some(3), json),
    b("proc.csv", Effect::Proc, 1, Some(3), csv),
    b("proc.kv", Effect::Proc, 1, Some(3), kv),
    b("proc.pipe", Effect::Proc, 1, Some(2), pipe),
    b("proc.shell", Effect::Proc, 1, Some(2), shell),
    b("proc.which", Effect::Read, 1, Some(1), which),
];

/// Default wall-clock limit for a hop: ten minutes.
pub const DEFAULT_TIMEOUT_MS: u64 = 600_000;
/// After SIGTERM, how long a child gets before SIGKILL.
pub const DEFAULT_GRACE_MS: u64 = 500;
/// How much of a child's output a diagnostic quotes.
const QUOTE_BYTES: usize = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Lossy,
    Utf16,
    Latin1,
}

struct Options {
    cwd: Option<PathBuf>,
    env: Vec<(String, String)>,
    clean_env: bool,
    timeout_ms: u64,
    grace_ms: u64,
    stdin: Option<String>,
    /// The exit-code contract; `None` means any code is an answer.
    ok: Option<Vec<i64>>,
    encoding: Encoding,
    header: bool,
    sep: char,
}

impl Options {
    fn parse(
        a: &[Value],
        index: usize,
        name: &str,
        s: Span,
        default_ok: Option<Vec<i64>>,
    ) -> Result<Self, Diagnostic> {
        let mut o = Options {
            cwd: None,
            env: Vec::new(),
            clean_env: false,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            grace_ms: DEFAULT_GRACE_MS,
            stdin: None,
            ok: default_ok,
            encoding: Encoding::Utf8,
            header: true,
            sep: ',',
        };
        let Some(opts) = opt_map(a, index, name, s)? else {
            return Ok(o);
        };
        for (k, v) in opts.borrow().iter() {
            match (k.as_str(), v) {
                ("cwd", Value::Str(p)) => o.cwd = Some(PathBuf::from(p.to_string())),
                ("env", Value::Map(m)) => {
                    for (ek, ev) in m.borrow().iter() {
                        o.env.push((ek.clone(), ev.display()));
                    }
                }
                ("clean_env", v) => o.clean_env = v.truthy(),
                ("timeout_ms", Value::Int(t)) if *t > 0 => o.timeout_ms = *t as u64,
                ("grace_ms", Value::Int(t)) if *t >= 0 => o.grace_ms = *t as u64,
                ("stdin", Value::Str(text)) => o.stdin = Some(text.to_string()),
                ("check", v) => {
                    if v.truthy() {
                        o.ok = Some(vec![0]);
                    } else if o.ok == Some(vec![0]) {
                        o.ok = None;
                    }
                }
                ("ok", Value::Int(n)) => o.ok = Some(vec![*n]),
                ("ok", Value::List(l)) => {
                    let mut codes = Vec::new();
                    for item in l.borrow().iter() {
                        match item {
                            Value::Int(n) => codes.push(*n),
                            other => {
                                return Err(type_error(format!(
                                    "{name}: `ok` lists exit codes (ints), got {}",
                                    other.type_name()
                                ))
                                .at(s))
                            }
                        }
                    }
                    o.ok = Some(codes);
                }
                ("ok", Value::Null) => o.ok = None,
                ("encoding", Value::Str(e)) => {
                    o.encoding = match e.to_ascii_lowercase().replace('_', "-").as_str() {
                        "utf-8" | "utf8" => Encoding::Utf8,
                        "lossy" => Encoding::Lossy,
                        "utf-16" | "utf16" => Encoding::Utf16,
                        "latin1" | "latin-1" | "iso-8859-1" => Encoding::Latin1,
                        other => {
                            return Err(runtime(format!("{name}: unknown encoding `{other}`"))
                                .at(s)
                                .with_hint("use utf-8 (strict, the default), lossy, utf-16 or latin1"))
                        }
                    }
                }
                ("header", v) => o.header = v.truthy(),
                ("sep", Value::Str(c)) => o.sep = c.chars().next().unwrap_or(','),
                ("cwd" | "env" | "timeout_ms" | "grace_ms" | "stdin" | "ok" | "encoding" | "sep", other) => {
                    return Err(type_error(format!(
                        "{name}: option `{k}` has the wrong type ({})",
                        other.type_name()
                    ))
                    .at(s))
                }
                (other, _) => {
                    return Err(runtime(format!("{name}: unknown option `{other}`"))
                        .at(s)
                        .with_hint("use one of cwd, env, clean_env, timeout_ms, grace_ms, stdin, check, ok, encoding, header, sep"))
                }
            }
        }
        Ok(o)
    }
}

fn args_of(a: &[Value], index: usize, name: &str, s: Span) -> Result<Vec<String>, Diagnostic> {
    Ok(match a.get(index) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::List(l)) => l.borrow().iter().map(|v| v.display()).collect(),
        Some(Value::Str(one)) => vec![one.to_string()],
        Some(other) => {
            return Err(type_error(format!(
                "{name}: arguments must be a list, got {}",
                other.type_name()
            ))
            .at(s))
        }
    })
}

/// What a hop produced, undecoded.
struct Raw {
    code: i32,
    out: Vec<u8>,
    err: Vec<u8>,
    duration_ms: i64,
}

fn quote(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let s = s.trim_end();
    let mut q: String = s.chars().take(QUOTE_BYTES).collect();
    if s.chars().count() > QUOTE_BYTES {
        q.push('…');
    }
    q.replace('\n', "⏎")
}

fn last_line(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    s.trim()
        .lines()
        .last()
        .unwrap_or("")
        .chars()
        .take(QUOTE_BYTES)
        .collect()
}

fn hop_label(cmd: &str, args: &[String]) -> String {
    let mut parts = vec![cmd.to_string()];
    parts.extend(args.iter().take(3).cloned());
    if args.len() > 3 {
        parts.push("…".to_string());
    }
    parts.join(" ")
}

/// Build a `Command` the same way for every hop: no shell, its own process
/// group, `NO_COLOR` set, the run id passed down, and either the inherited
/// environment plus the script's additions or a clean documented one.
fn command(cmd: &str, args: &[String], o: &Options) -> Command {
    let mut c = Command::new(cmd);
    c.args(args)
        .stdin(if o.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = &o.cwd {
        c.current_dir(dir);
    }
    if o.clean_env {
        c.env_clear();
        for key in [
            "PATH",
            "HOME",
            "USER",
            "LOGNAME",
            "SHELL",
            "TMPDIR",
            "TERM",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "TZ",
            "SYSTEMROOT",
            "USERPROFILE",
            "APPDATA",
            "COMSPEC",
            "PATHEXT",
        ] {
            if let Some(v) = std::env::var_os(key) {
                c.env(key, v);
            }
        }
    }
    c.env("NO_COLOR", "1");
    if let Ok(id) = std::env::var("CIG_RUN_ID") {
        c.env("CIG_RUN_ID", id);
    }
    for (k, v) in &o.env {
        c.env(k, v);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    c
}

fn spawn_error(name: &str, cmd: &str, e: std::io::Error, s: Span) -> Diagnostic {
    let (code, hint) = match e.kind() {
        std::io::ErrorKind::NotFound => (
            "E551",
            "check with proc.which(name); install it, or give the full path",
        ),
        std::io::ErrorKind::PermissionDenied => {
            ("E552", "chmod +x it, or run as a user that may execute it")
        }
        _ => ("E550", "the operating system's reason is in the message"),
    };
    runtime(format!(
        "{name}: could not start `{cmd}`: {}",
        super::fs::describe_io(&e)
    ))
    .code(code)
    .at(s)
    .with_subject(cmd.to_string())
    .with_hint(hint)
}

/// Kill a child and everything in its process group: SIGTERM, a grace
/// period, then SIGKILL. Returns once the group is gone.
fn kill_group(child: &mut Child, grace_ms: u64) {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        // The child is its own group leader (process_group(0)).
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_millis(grace_ms);
        loop {
            if let Ok(Some(_)) = child.try_wait() {
                break;
            }
            if Instant::now() >= deadline {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
                let _ = child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Grandchildren that ignored SIGTERM get the SIGKILL too.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = grace_ms;
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Run one child to completion under the rules. `Err` for anything that
/// means the next step must not see the output.
fn run_child(
    name: &str,
    cmd: &str,
    args: &[String],
    o: &Options,
    s: Span,
) -> Result<Raw, Diagnostic> {
    let started = Instant::now();
    let mut child = command(cmd, args, o)
        .spawn()
        .map_err(|e| spawn_error(name, cmd, e, s))?;
    if let (Some(text), Some(mut pipe)) = (&o.stdin, child.stdin.take()) {
        // A closed pipe is not an error: the child may not read stdin.
        let _ = pipe.write_all(text.as_bytes());
    }
    let (out_thread, err_thread) = readers(&mut child);
    let label = hop_label(cmd, args);
    let waited = child
        .wait_timeout(Duration::from_millis(o.timeout_ms))
        .map_err(|e| runtime(format!("{name}: {e}")).code("E550").at(s))?;
    match waited {
        None => {
            kill_group(&mut child, o.grace_ms);
            let out = out_thread.join().unwrap_or_default();
            let err = err_thread.join().unwrap_or_default();
            Err(runtime(format!(
                "{name}: `{label}` ran longer than {} ms and was killed; it had written {} bytes to stdout{} and {} bytes to stderr{}",
                o.timeout_ms,
                out.len(),
                if out.is_empty() { String::new() } else { format!(" (`{}`)", quote(&out)) },
                err.len(),
                if err.is_empty() { String::new() } else { format!(" (`{}`)", quote(&err)) },
            ))
            .code("E554")
            .at(s)
            .with_subject(cmd.to_string())
            .with_hint("raise {timeout_ms: N} on this hop, or split the work; nothing it wrote was passed on"))
        }
        Some(status) => {
            let out = out_thread.join().unwrap_or_default();
            let err = err_thread.join().unwrap_or_default();
            let duration_ms = started.elapsed().as_millis() as i64;
            match status.code() {
                Some(code) => Ok(Raw {
                    code,
                    out,
                    err,
                    duration_ms,
                }),
                None => {
                    #[cfg(unix)]
                    let signal = {
                        use std::os::unix::process::ExitStatusExt;
                        status.signal().unwrap_or(0)
                    };
                    #[cfg(not(unix))]
                    let signal = 0;
                    Err(runtime(format!(
                        "{name}: `{label}` was killed by signal {signal}{}; it had written {} bytes to stdout and {} bytes to stderr{}",
                        signal_name(signal),
                        out.len(),
                        err.len(),
                        if err.is_empty() { String::new() } else { format!(" (`{}`)", quote(&err)) },
                    ))
                    .code("E555")
                    .at(s)
                    .with_subject(cmd.to_string())
                    .with_hint("the program died, it did not finish; its partial output was not passed on"))
                }
            }
        }
    }
}

fn signal_name(signal: i32) -> &'static str {
    match signal {
        1 => " (SIGHUP)",
        2 => " (SIGINT)",
        6 => " (SIGABRT)",
        9 => " (SIGKILL)",
        11 => " (SIGSEGV)",
        13 => " (SIGPIPE)",
        15 => " (SIGTERM)",
        _ => "",
    }
}

type Reader = std::thread::JoinHandle<Vec<u8>>;

/// Read both streams concurrently: a child filling stderr while stdout is
/// awaited is the classic deadlock.
fn readers(child: &mut Child) -> (Reader, Reader) {
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
    (out_thread, err_thread)
}

fn decode(
    name: &str,
    stream: &str,
    bytes: &[u8],
    enc: Encoding,
    s: Span,
) -> Result<String, Diagnostic> {
    match enc {
        Encoding::Utf8 => String::from_utf8(bytes.to_vec()).map_err(|e| {
            let at = e.utf8_error().valid_up_to();
            runtime(format!(
                "{name}: {stream} is not valid UTF-8 at byte {at}; the child wrote {} bytes",
                bytes.len()
            ))
            .code("E557")
            .at(s)
            .with_hint("add {encoding: \"lossy\"} to replace bad bytes, \"latin1\" for single-byte text, or \"utf-16\" for a Windows tool")
        }),
        Encoding::Lossy => Ok(String::from_utf8_lossy(bytes).to_string()),
        Encoding::Latin1 => Ok(bytes.iter().map(|&b| b as char).collect()),
        Encoding::Utf16 => {
            let (body, big_endian) = match bytes {
                [0xFF, 0xFE, rest @ ..] => (rest, false),
                [0xFE, 0xFF, rest @ ..] => (rest, true),
                _ => (bytes, false),
            };
            if body.len() % 2 != 0 {
                return Err(runtime(format!(
                    "{name}: {stream} is not valid UTF-16 (odd length {})",
                    body.len()
                ))
                .code("E557")
                .at(s));
            }
            let units: Vec<u16> = body
                .chunks(2)
                .map(|c| {
                    if big_endian {
                        u16::from_be_bytes([c[0], c[1]])
                    } else {
                        u16::from_le_bytes([c[0], c[1]])
                    }
                })
                .collect();
            String::from_utf16(&units).map_err(|_| {
                runtime(format!("{name}: {stream} is not valid UTF-16"))
                    .code("E557")
                    .at(s)
            })
        }
    }
}

/// Enforce the exit-code contract: outside it, the hop failed, with the
/// stderr's last line in the message and the whole result as the payload.
fn enforce_contract(
    i: &mut Interp,
    name: &str,
    label: &str,
    raw: &Raw,
    o: &Options,
    result_value: Value,
    s: Span,
) -> Result<(), Diagnostic> {
    let Some(ok) = &o.ok else {
        return Ok(());
    };
    if ok.contains(&(raw.code as i64)) {
        return Ok(());
    }
    let contract = ok
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let tail = last_line(&raw.err);
    i.cough_payload = Some(result_value);
    Err(runtime(format!(
        "{name}: `{label}` exited {} (contract: {contract}){}",
        raw.code,
        if tail.is_empty() {
            String::new()
        } else {
            format!("; stderr: `{tail}`")
        }
    ))
    .code("E553")
    .at(s)
    .with_hint("if that code is an answer, not a failure, say so: {ok: [0, 1]}; read err in the ashtray value otherwise"))
}

fn result_map(code: i32, out: &str, err: &str, duration_ms: i64, simulated: bool) -> Value {
    let mut m = IndexMap::new();
    m.insert("code".to_string(), Value::Int(code as i64));
    m.insert("out".to_string(), Value::str(out));
    m.insert("err".to_string(), Value::str(err));
    m.insert("duration_ms".to_string(), Value::Int(duration_ms));
    m.insert("timed_out".to_string(), Value::Bool(false));
    m.insert("simulated".to_string(), Value::Bool(simulated));
    Value::map(m)
}

/// The shared path of every verb: gate the effect, run, decode, enforce.
/// `None` when the hop was simulated.
fn hop(
    i: &mut Interp,
    name: &str,
    cmd: &str,
    args: &[String],
    o: &Options,
    s: Span,
) -> Result<Option<(Raw, String, String)>, Diagnostic> {
    let op = Op::Proc {
        cmd: cmd.to_string(),
        args: args.to_vec(),
        cwd: o.cwd.clone(),
    };
    if i.effect(name, op, s)? == Decision::Simulate {
        return Ok(None);
    }
    let raw = run_child(name, cmd, args, o, s)?;
    let out = decode(name, "stdout", &raw.out, o.encoding, s)?;
    let err = decode(name, "stderr", &raw.err, Encoding::Lossy, s)?;
    let value = result_map(raw.code, &out, &err, raw.duration_ms, false);
    enforce_contract(i, name, &hop_label(cmd, args), &raw, o, value, s)?;
    Ok(Some((raw, out, err)))
}

/// `proc.run(cmd, args = [], opts = {})` → `{code, out, err, duration_ms,
/// timed_out, simulated}`. Any exit code is an answer unless `check` or
/// `ok` says otherwise.
fn run(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let cmd = expect_str(a, 0, "proc.run", s)?.to_string();
    if cmd.trim().is_empty() {
        return Err(runtime("proc.run: command is empty").at(s));
    }
    let args = args_of(a, 1, "proc.run", s)?;
    let o = Options::parse(a, 2, "proc.run", s, None)?;
    match hop(i, "proc.run", &cmd, &args, &o, s)? {
        None => Ok(result_map(0, "", "", 0, true)),
        Some((raw, out, err)) => Ok(result_map(raw.code, &out, &err, raw.duration_ms, false)),
    }
}

/// The sugar verbs share one shape: contract `[0]` unless told otherwise,
/// stdout parsed, stderr shown only on failure.
fn sugar(
    i: &mut Interp,
    name: &str,
    a: &[Value],
    s: Span,
) -> Result<Option<(Raw, String)>, Diagnostic> {
    let cmd = expect_str(a, 0, name, s)?.to_string();
    if cmd.trim().is_empty() {
        return Err(runtime(format!("{name}: command is empty")).at(s));
    }
    let args = args_of(a, 1, name, s)?;
    let o = Options::parse(a, 2, name, s, Some(vec![0]))?;
    Ok(hop(i, name, &cmd, &args, &o, s)?.map(|(raw, out, _)| (raw, out)))
}

fn text(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    Ok(match sugar(i, "proc.text", a, s)? {
        None => Value::str(""),
        Some((_, out)) => Value::str(out),
    })
}

fn lines(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    Ok(match sugar(i, "proc.lines", a, s)? {
        None => Value::list(vec![]),
        Some((_, out)) => Value::list(out.lines().map(Value::str).collect()),
    })
}

fn bad_shape(name: &str, expected: &str, out: &str, detail: &str, s: Span) -> Diagnostic {
    runtime(format!(
        "{name}: expected {expected} on stdout, got `{}`{}",
        quote(out.as_bytes()),
        if detail.is_empty() {
            String::new()
        } else {
            format!(" ({detail})")
        }
    ))
    .code("E556")
    .at(s)
    .with_hint("structured data comes from stdout only; send progress to stderr, or use proc.run and parse what you can")
}

fn json(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    match sugar(i, "proc.json", a, s)? {
        None => Ok(Value::Null),
        Some((_, out)) => serde_json::from_str::<serde_json::Value>(&out)
            .map(super::json::to_value)
            .map_err(|e| bad_shape("proc.json", "JSON", &out, &e.to_string(), s)),
    }
}

fn csv(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let o = Options::parse(a, 2, "proc.csv", s, Some(vec![0]))?;
    match sugar(i, "proc.csv", a, s)? {
        None => Ok(Value::list(vec![])),
        Some((_, out)) => {
            if out.trim().is_empty() {
                return Ok(Value::list(vec![]));
            }
            Ok(super::csv::rows_to_value(
                super::csv::parse_rows(&out, o.sep),
                o.header,
            ))
        }
    }
}

/// `KEY=value` lines (an `.env` file, `env`, `git config -l`) as a map.
fn kv(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    match sugar(i, "proc.kv", a, s)? {
        None => Ok(Value::map(IndexMap::new())),
        Some((_, out)) => {
            let mut m = IndexMap::new();
            for (n, line) in out.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some((k, v)) = line.split_once('=') else {
                    return Err(bad_shape(
                        "proc.kv",
                        "KEY=value lines",
                        &out,
                        &format!("line {} has no `=`", n + 1),
                        s,
                    ));
                };
                let v = v.trim();
                let v = v
                    .strip_prefix('"')
                    .and_then(|x| x.strip_suffix('"'))
                    .unwrap_or(v);
                m.insert(k.trim().to_string(), Value::str(v));
            }
            Ok(Value::map(m))
        }
    }
}

/// `proc.shell(command, opts)`: the shell, by name, for the honest cases.
/// Everything you pass is interpreted by `sh -c`; that is where injection
/// lives, so this verb exists so that `proc.run` does not have to.
fn shell(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let script = expect_str(a, 0, "proc.shell", s)?.to_string();
    let o = Options::parse(a, 1, "proc.shell", s, None)?;
    #[cfg(windows)]
    let (cmd, args) = ("cmd".to_string(), vec!["/C".to_string(), script.clone()]);
    #[cfg(not(windows))]
    let (cmd, args) = ("sh".to_string(), vec!["-c".to_string(), script.clone()]);
    match hop(i, "proc.shell", &cmd, &args, &o, s)? {
        None => Ok(result_map(0, "", "", 0, true)),
        Some((raw, out, err)) => Ok(result_map(raw.code, &out, &err, raw.duration_ms, false)),
    }
}

/// `proc.pipe([[cmd, args...], ...], opts)`: stages connected stdout to
/// stdin, no shell, one process group, one timeout. Every stage must exit
/// 0 (the last one honours `ok`); a failure names the stage.
fn pipe(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let stages: Vec<(String, Vec<String>)> = match a.first() {
        Some(Value::List(l)) => {
            let mut out = Vec::new();
            for (n, stage) in l.borrow().iter().enumerate() {
                let words: Vec<String> = match stage {
                    Value::List(w) => w.borrow().iter().map(|v| v.display()).collect(),
                    Value::Str(one) => vec![one.to_string()],
                    other => {
                        return Err(type_error(format!(
                            "proc.pipe: stage {} must be a list of words, got {}",
                            n + 1,
                            other.type_name()
                        ))
                        .at(s))
                    }
                };
                let Some((cmd, rest)) = words.split_first() else {
                    return Err(runtime(format!("proc.pipe: stage {} is empty", n + 1)).at(s));
                };
                out.push((cmd.clone(), rest.to_vec()));
            }
            out
        }
        _ => {
            return Err(
                type_error("proc.pipe: expected a list of stages, each a list of words")
                    .at(s)
                    .with_hint(
                        "proc.pipe([[\"cat\", \"big.log\"], [\"grep\", \"ERROR\"], [\"sort\"]])",
                    ),
            )
        }
    };
    if stages.is_empty() {
        return Err(runtime("proc.pipe: no stages").at(s));
    }
    let o = Options::parse(a, 1, "proc.pipe", s, None)?;
    // One effect for the pipeline; the plan shows it as one hop.
    let summary_args: Vec<String> = stages
        .iter()
        .map(|(c, args)| hop_label(c, args))
        .collect::<Vec<_>>()
        .join(" | ")
        .split(' ')
        .map(str::to_string)
        .collect();
    let op = Op::Proc {
        cmd: "pipe:".to_string(),
        args: summary_args,
        cwd: o.cwd.clone(),
    };
    if i.effect("proc.pipe", op, s)? == Decision::Simulate {
        return Ok(result_map(0, "", "", 0, true));
    }
    let started = Instant::now();
    let mut children: Vec<Child> = Vec::new();
    let mut err_readers: Vec<Reader> = Vec::new();
    let mut previous_out: Option<std::process::ChildStdout> = None;
    for (n, (cmd, args)) in stages.iter().enumerate() {
        let mut c = command(cmd, args, &o);
        match previous_out.take() {
            Some(prev) => {
                c.stdin(Stdio::from(prev));
            }
            None if n == 0 && o.stdin.is_some() => {
                c.stdin(Stdio::piped());
            }
            None => {
                c.stdin(Stdio::null());
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Every stage joins the first stage's group so one kill ends all.
            if let Some(first) = children.first() {
                c.process_group(first.id() as i32);
            }
        }
        let mut child = c.spawn().map_err(|e| {
            for mut earlier in children.drain(..) {
                kill_group(&mut earlier, 0);
            }
            spawn_error(&format!("proc.pipe (stage {})", n + 1), cmd, e, s)
        })?;
        if n == 0 {
            if let (Some(text), Some(mut pipe_in)) = (&o.stdin, child.stdin.take()) {
                let _ = pipe_in.write_all(text.as_bytes());
            }
        }
        let mut err_pipe = child.stderr.take();
        err_readers.push(std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(p) = err_pipe.as_mut() {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        }));
        if n + 1 < stages.len() {
            previous_out = child.stdout.take();
        }
        children.push(child);
    }
    let last = children.len() - 1;
    let mut out_pipe = children[last].stdout.take();
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let deadline = Instant::now() + Duration::from_millis(o.timeout_ms);
    let mut codes: Vec<Option<i32>> = vec![None; children.len()];
    let mut timed_out = false;
    'wait: loop {
        for (n, child) in children.iter_mut().enumerate() {
            if codes[n].is_none() {
                match child.try_wait() {
                    Ok(Some(status)) => codes[n] = Some(status.code().unwrap_or(-1)),
                    Ok(None) => {}
                    Err(_) => codes[n] = Some(-1),
                }
            }
        }
        if codes.iter().all(Option::is_some) {
            break 'wait;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            break 'wait;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if timed_out {
        for child in children.iter_mut() {
            kill_group(child, o.grace_ms);
        }
        let _ = out_reader.join();
        for r in err_readers {
            let _ = r.join();
        }
        return Err(runtime(format!(
            "proc.pipe: the pipeline ran longer than {} ms and was killed",
            o.timeout_ms
        ))
        .code("E554")
        .at(s)
        .with_hint(
            "raise {timeout_ms: N}, or split the pipeline; nothing it wrote was passed on",
        ));
    }
    let out_bytes = out_reader.join().unwrap_or_default();
    let errs: Vec<Vec<u8>> = err_readers
        .into_iter()
        .map(|r| r.join().unwrap_or_default())
        .collect();
    let duration_ms = started.elapsed().as_millis() as i64;
    // Every stage but the last must exit 0; the last honours the contract.
    for (n, code) in codes.iter().enumerate() {
        let code = code.unwrap_or(-1);
        let is_last = n == last;
        let allowed: Vec<i64> = if is_last {
            o.ok.clone().unwrap_or_else(|| vec![0])
        } else {
            vec![0]
        };
        if !allowed.contains(&(code as i64)) {
            let (cmd, args) = &stages[n];
            let tail = last_line(&errs[n]);
            let out_text = String::from_utf8_lossy(&out_bytes).to_string();
            let err_text = String::from_utf8_lossy(&errs[n]).to_string();
            i.cough_payload = Some(result_map(code, &out_text, &err_text, duration_ms, false));
            return Err(runtime(format!(
                "proc.pipe: stage {} (`{}`) exited {code} (contract: {}){}",
                n + 1,
                hop_label(cmd, args),
                allowed
                    .iter()
                    .map(|c| c.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                if tail.is_empty() {
                    String::new()
                } else {
                    format!("; stderr: `{tail}`")
                }
            ))
            .code("E553")
            .at(s)
            .with_hint("if that code is an answer for the last stage, say so with {ok: [0, 1]}"));
        }
    }
    let out = decode("proc.pipe", "stdout", &out_bytes, o.encoding, s)?;
    let err: String = errs
        .iter()
        .map(|e| String::from_utf8_lossy(e).to_string())
        .collect::<Vec<_>>()
        .join("");
    let mut m = match result_map(codes[last].unwrap_or(-1), &out, &err, duration_ms, false) {
        Value::Map(m) => m.borrow().clone(),
        _ => unreachable!(),
    };
    m.insert(
        "stages".to_string(),
        Value::list(
            stages
                .iter()
                .zip(codes.iter())
                .map(|((cmd, args), code)| {
                    let mut sm = IndexMap::new();
                    sm.insert("cmd".to_string(), Value::str(hop_label(cmd, args)));
                    sm.insert("code".to_string(), Value::Int(code.unwrap_or(-1) as i64));
                    Value::map(sm)
                })
                .collect(),
        ),
    );
    Ok(Value::map(m))
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
