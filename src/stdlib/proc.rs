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
    /// Globs of the files the hop may change, kept in the tin first.
    touches: Vec<String>,
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
            touches: Vec::new(),
        };
        let Some(opts) = opt_map(a, index, name, s)? else {
            return Ok(o);
        };
        // `ok` is the contract itself and wins over `check` whatever the
        // order of the keys; `check` alone sets it to [0] or clears it.
        let mut check: Option<bool> = None;
        let mut ok_given: Option<Option<Vec<i64>>> = None;
        for (k, v) in opts.borrow().iter() {
            match (k.as_str(), v) {
                ("cwd", Value::Str(p)) => o.cwd = Some(PathBuf::from(p.to_string())),
                ("touches", Value::Str(g)) => o.touches = vec![g.to_string()],
                ("touches", Value::List(l)) => {
                    for item in l.borrow().iter() {
                        match item {
                            Value::Str(g) => o.touches.push(g.to_string()),
                            other => {
                                return Err(type_error(format!(
                                    "{name}: `touches` holds a {}; want globs as strings",
                                    other.type_name()
                                ))
                                .at(s))
                            }
                        }
                    }
                }
                ("env", Value::Map(m)) => {
                    for (ek, ev) in m.borrow().iter() {
                        o.env.push((ek.clone(), ev.display()));
                    }
                }
                ("clean_env", v) => o.clean_env = v.truthy(),
                ("timeout_ms", Value::Int(t)) if *t > 0 => o.timeout_ms = *t as u64,
                ("grace_ms", Value::Int(t)) if *t >= 0 => o.grace_ms = *t as u64,
                ("stdin", Value::Str(text)) => o.stdin = Some(text.to_string()),
                ("check", v) => check = Some(v.truthy()),
                ("ok", Value::Int(n)) => ok_given = Some(Some(vec![*n])),
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
                    if codes.is_empty() {
                        return Err(runtime(format!(
                            "{name}: the `ok` contract is empty; no exit code could satisfy it"
                        ))
                        .at(s)
                        .with_hint("list the codes that count as success, {ok: [0, 1]}, or ok: null for no contract"));
                    }
                    ok_given = Some(Some(codes));
                }
                ("ok", Value::Null) => ok_given = Some(None),
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
                ("sep", Value::Str(c)) => {
                    let mut chars = c.chars();
                    match (chars.next(), chars.next()) {
                        (Some(one), None) => o.sep = one,
                        _ => {
                            return Err(runtime(format!(
                                "{name}: `sep` must be one character, got `{c}`"
                            ))
                            .at(s))
                        }
                    }
                }
                ("cwd" | "env" | "timeout_ms" | "grace_ms" | "stdin" | "ok" | "encoding" | "sep" | "touches", other) => {
                    return Err(type_error(format!(
                        "{name}: option `{k}` has the wrong type ({})",
                        other.type_name()
                    ))
                    .at(s))
                }
                (other, _) => {
                    return Err(runtime(format!("{name}: unknown option `{other}`"))
                        .at(s)
                        .with_hint("use one of cwd, env, clean_env, timeout_ms, grace_ms, stdin, check, ok, encoding, header, sep, touches"))
                }
            }
        }
        match (ok_given, check) {
            (Some(ok), _) => o.ok = ok,
            (None, Some(true)) => o.ok = Some(vec![0]),
            (None, Some(false)) => o.ok = None,
            (None, None) => {}
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

/// The Windows `PATHEXT` list, lower-cased with the dots, or the default.
#[cfg(windows)]
fn pathext() -> Vec<String> {
    let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let mut out: Vec<String> = raw
        .split(';')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| e.starts_with('.'))
        .collect();
    if out.is_empty() {
        out = vec![".com".into(), ".exe".into(), ".bat".into(), ".cmd".into()];
    }
    out
}

/// Is this directory Windows' System32 (or SysWOW64), where `bash.exe` is
/// the WSL launcher and `wsl.exe` lives? A hop asking for `bash` or `sh`
/// means a shell, never WSL, so those directories are skipped for them.
#[cfg(windows)]
fn is_system_dir(dir: &std::path::Path) -> bool {
    let root = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("C:\\Windows"));
    let lower = dir.to_string_lossy().to_ascii_lowercase();
    for sub in ["system32", "syswow64"] {
        let sys = root.join(sub).to_string_lossy().to_ascii_lowercase();
        if lower.trim_end_matches('\\') == sys.trim_end_matches('\\') {
            return true;
        }
    }
    false
}

/// A candidate on PATH that a hop could start: a regular file, executable
/// where the platform says so.
fn startable(p: &std::path::Path) -> bool {
    let m = match std::fs::metadata(p) {
        Ok(m) => m,
        // A Windows app execution alias (the Store's python.exe) is a
        // reparse point the metadata call cannot follow; the entry itself
        // is what CreateProcess starts.
        #[cfg(windows)]
        Err(_) => match std::fs::symlink_metadata(p) {
            Ok(m) if !m.is_dir() => m,
            _ => return false,
        },
        #[cfg(not(windows))]
        Err(_) => return false,
    };
    if m.is_dir() {
        return false;
    }
    #[cfg(not(windows))]
    if !m.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        // What execvp asks: may this process run it? A file whose execute
        // bit belongs to someone else is passed over, as the shell would.
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: a NUL-terminated path, read by access(2).
        unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Where a hop's program is: the file `proc.run` would start for `name`,
/// so `proc.which` and `proc.run` can never disagree. A name with a path
/// separator is a path, made absolute against the hop's working directory
/// (the platform's own answer differs: Windows resolves it against the
/// parent's directory). A bare name is searched on `PATH`: on Unix the
/// first executable file; on Windows each directory in turn with each
/// `PATHEXT` extension (`npm` is `npm.cmd`), never the WSL launcher in
/// System32 for `bash` or `sh`, and never the current directory.
pub fn resolve_program(name: &str, cwd: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    resolve_program_on(name, cwd, std::env::var_os("PATH"))
}

/// `resolve_program` on a given PATH: the one the child will search, so a
/// hop that sets its own PATH starts the program that PATH names.
pub fn resolve_program_on(
    name: &str,
    cwd: Option<&std::path::Path>,
    paths: Option<std::ffi::OsString>,
) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    if name.is_empty() {
        return None;
    }
    let is_path = name.contains('/') || name.contains(std::path::MAIN_SEPARATOR);
    if is_path {
        let given = Path::new(name);
        let base: PathBuf = if given.is_absolute() {
            given.to_path_buf()
        } else {
            let dir = cwd
                .map(Path::to_path_buf)
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_default();
            dir.join(given)
        };
        // Absolute, always: a relative program handed to `Command` with
        // a `cwd` is resolved against that cwd by the child, which is not
        // where it was found.
        let absolute = |p: PathBuf| std::path::absolute(&p).unwrap_or(p);
        // Windows starts `.\gradlew` the way cmd does: `gradlew.bat` and
        // the rest of PATHEXT first, the bare file last. The bare file is
        // usually the Unix shell script beside the batch file, which
        // CreateProcess cannot start (error 193).
        #[cfg(windows)]
        if base.extension().is_none() {
            for ext in pathext() {
                let mut with = base.clone().into_os_string();
                with.push(&ext);
                let with = PathBuf::from(with);
                if startable(&with) {
                    return Some(absolute(with));
                }
            }
        }
        if startable(&base) {
            return Some(absolute(base));
        }
        return None;
    }
    // No PATH at all: the platform's own default search, as execvp uses.
    let paths = paths.unwrap_or_else(default_path);
    // A relative PATH entry (`.`, `bin`) is read from where the program
    // will start: the hop's working directory, as execvp in the child
    // would.
    let here = cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let from_here = |dir: PathBuf| {
        if dir.is_relative() {
            here.join(dir)
        } else {
            dir
        }
    };
    #[cfg(windows)]
    {
        let exts = pathext();
        let lower = name.to_ascii_lowercase();
        let shell_name = matches!(lower.as_str(), "bash" | "sh" | "bash.exe" | "sh.exe");
        let has_known_ext = Path::new(&lower)
            .extension()
            .map(|e| exts.iter().any(|x| x[1..] == e.to_string_lossy()))
            .unwrap_or(false);
        // CreateProcess looks in the system directories whatever PATH
        // says, so `cmd` and `powershell` are found on a hermetic PATH too.
        let mut dirs: Vec<PathBuf> = std::env::split_paths(&paths).collect();
        if let Some(root) = std::env::var_os("SystemRoot") {
            let root = PathBuf::from(root);
            for sys in [root.join("System32"), root] {
                if !dirs
                    .iter()
                    .any(|d| d.as_os_str().eq_ignore_ascii_case(sys.as_os_str()))
                {
                    dirs.push(sys);
                }
            }
        }
        for dir in dirs {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let dir = from_here(dir);
            if shell_name && is_system_dir(&dir) {
                continue;
            }
            if has_known_ext {
                let c = dir.join(name);
                if startable(&c) {
                    return Some(std::path::absolute(&c).unwrap_or(c));
                }
                continue;
            }
            for ext in &exts {
                let c = dir.join(format!("{name}{ext}"));
                if startable(&c) {
                    return Some(std::path::absolute(&c).unwrap_or(c));
                }
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        for dir in std::env::split_paths(&paths) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let c = from_here(dir).join(name);
            if startable(&c) {
                return Some(std::path::absolute(&c).unwrap_or(c));
            }
        }
        None
    }
}

/// The search path when the child has none: what execvp falls back to on
/// Unix, the system directories on Windows.
fn default_path() -> std::ffi::OsString {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
        std::env::join_paths([root.join("System32"), root]).unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        std::ffi::OsString::from("/usr/local/bin:/usr/bin:/bin")
    }
}

/// The file is a native executable (`.exe`, `.com`), the one kind that
/// reads its command line by the C runtime's rules. A batch file (cmd.exe
/// reads the line again), a name nothing resolved and anything else take
/// the standard library's own quoting.
#[cfg(windows)]
fn native_exe(p: &std::path::Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("com"))
}

/// One argument on a Windows command line. The standard library quotes an
/// argument only when the C runtime's rules need it (a space, a tab, a
/// quote), and a program built on the MSYS or Cygwin runtime (Git's `bash`,
/// `sh`, `perl`) parses the line by its own rules instead, where a bare `'`
/// opens a quoted string and `*`, `?` and `[` glob: `it's` arrived as `its`.
/// An argument with any of those characters is quoted the way the C runtime
/// reads it, which both parsers agree on; the rest go as they are.
#[cfg(windows)]
fn windows_arg(c: &mut Command, arg: &str) {
    use std::os::windows::process::CommandExt;
    if arg
        .chars()
        .any(|ch| matches!(ch, ' ' | '\t' | '"' | '\'' | '*' | '?' | '['))
    {
        c.raw_arg(quoted_for_crt(arg));
    } else {
        c.arg(arg);
    }
}

/// `"..."` with the C runtime's escapes: a backslash run before a quote is
/// doubled and the quote escaped; a run at the end is doubled; any other
/// backslash is literal.
#[cfg(windows)]
fn quoted_for_crt(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                out.push(ch);
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// Build a `Command` the same way for every hop: no shell, its own process
/// group, `NO_COLOR` set, the run id passed down, and either the inherited
/// environment plus the script's additions or a clean documented one. The
/// program is resolved first (`resolve_program`), so what starts is what
/// `proc.which` names: a bare name nothing resolves is `None`, and the hop
/// fails as not found (E551) rather than let the platform search again by
/// rules of its own (the current directory among them). A path nothing
/// resolves is handed over as written, so the platform's reason is told.
fn command(cmd: &str, args: &[String], o: &Options) -> Option<Command> {
    let resolved = resolve_program_on(cmd, o.cwd.as_deref(), child_path(o));
    let is_path = cmd.contains('/') || cmd.contains(std::path::MAIN_SEPARATOR);
    if resolved.is_none() && !is_path {
        return None;
    }
    #[cfg(windows)]
    let native = resolved.as_deref().is_some_and(native_exe);
    #[cfg(windows)]
    let is_cmd = resolved
        .as_deref()
        .and_then(|p| p.file_stem())
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("cmd"));
    let program = resolved.unwrap_or_else(|| std::path::PathBuf::from(cmd));
    let mut c = Command::new(program);
    #[cfg(unix)]
    {
        // argv[0] stays the name the script typed (a usage line, a
        // multi-call binary); the file that starts is the resolved one.
        use std::os::unix::process::CommandExt;
        c.arg0(cmd);
    }
    #[cfg(windows)]
    {
        // `cmd /d /s /c <line>`: the line reaches cmd exactly as written,
        // in one pair of quotes that /s strips, instead of re-quoted by
        // the C runtime's rules, which cmd does not read.
        let cmd_line =
            is_cmd && args.len() == 4 && args[0] == "/d" && args[1] == "/s" && args[2] == "/c";
        if cmd_line {
            use std::os::windows::process::CommandExt;
            c.args(&args[..3]);
            c.raw_arg(format!("\"{}\"", args[3]));
        } else if native {
            for a in args {
                windows_arg(&mut c, a);
            }
        } else {
            // A .bat or .cmd runs through cmd.exe with the standard
            // library's own quoting for it; so does anything unresolved.
            c.args(args);
        }
    }
    #[cfg(not(windows))]
    c.args(args);
    c.stdin(if o.stdin.is_some() {
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
            "TEMP",
            "TMP",
            "LOCALAPPDATA",
            "WINDIR",
            "SYSTEMDRIVE",
            "PROGRAMDATA",
            "PROGRAMFILES",
            "PROGRAMFILES(X86)",
            "HOMEDRIVE",
            "HOMEPATH",
            "PROCESSOR_ARCHITECTURE",
            "NUMBER_OF_PROCESSORS",
            "OS",
        ] {
            if let Some(v) = std::env::var_os(key) {
                c.env(key, v);
            }
        }
    }
    c.env("NO_COLOR", "1");
    // The run's id travels to every hop: a cig the hop starts knows which
    // run holds the pack it shares (the pack lock lets a child run in), and
    // a tool can name the run in its own records.
    if let Some(id) = crate::crash::active_run() {
        c.env("CIG_RUN_ID", id);
    } else if let Ok(id) = std::env::var("CIG_RUN_ID") {
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
    Some(c)
}

/// The PATH the child will search: the hop's own when its `env` sets one
/// (the last one given wins, as in the environment), else the parent's,
/// which a clean environment copies too.
fn child_path(o: &Options) -> Option<std::ffi::OsString> {
    o.env
        .iter()
        .rev()
        .find(|(k, _)| is_path_key(k))
        .map(|(_, v)| std::ffi::OsString::from(v.to_string()))
        .or_else(|| std::env::var_os("PATH"))
}

#[cfg(windows)]
fn is_path_key(k: &str) -> bool {
    k.eq_ignore_ascii_case("PATH")
}

#[cfg(not(windows))]
fn is_path_key(k: &str) -> bool {
    k == "PATH"
}

/// A missing working directory is the hop's problem, not the program's:
/// the spawn would report "not found" and blame the command.
fn check_cwd(name: &str, o: &Options, s: Span) -> Result<(), Diagnostic> {
    if let Some(dir) = &o.cwd {
        if !dir.is_dir() {
            return Err(runtime(format!(
                "{name}: the working directory `{}` {}",
                dir.display(),
                if dir.exists() {
                    "is not a directory"
                } else {
                    "does not exist"
                }
            ))
            .code("E550")
            .at(s)
            .with_subject(dir.display().to_string())
            .with_hint("create it first (fs.mkdir inside the burn), or drop the cwd option"));
        }
    }
    Ok(())
}

// ----- signals reach the child's process group ---------------------------------
//
// Every child runs in its own process group (so a timeout can kill all of it),
// which also means a Ctrl-C at the terminal reaches cig and not the child.
// The forwarder sends SIGINT, SIGTERM and SIGHUP on to every live child group,
// then lets the signal end cig the default way.

#[cfg(unix)]
mod forward {
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::OnceLock;

    const SLOTS: usize = 64;
    static ACTIVE: [AtomicI32; SLOTS] = [const { AtomicI32::new(0) }; SLOTS];

    extern "C" fn on_signal(sig: libc::c_int) {
        // Async-signal-safe: atomics, kill, signal, raise.
        for slot in ACTIVE.iter() {
            let pgid = slot.load(Ordering::Relaxed);
            if pgid > 0 {
                unsafe {
                    libc::kill(-pgid, sig);
                }
            }
        }
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }

    pub fn install() {
        static ONCE: OnceLock<()> = OnceLock::new();
        ONCE.get_or_init(|| unsafe {
            for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
                libc::signal(
                    sig,
                    on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t,
                );
            }
        });
    }

    pub fn register(pgid: u32) -> Option<usize> {
        let pgid = pgid as i32;
        (0..SLOTS).find(|&i| {
            ACTIVE[i]
                .compare_exchange(0, pgid, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        })
    }

    pub fn unregister(slot: usize) {
        ACTIVE[slot].store(0, Ordering::SeqCst);
    }
}

#[cfg(unix)]
const SIGPIPE: i32 = libc::SIGPIPE;
#[cfg(not(unix))]
const SIGPIPE: i32 = 13;

fn install_signal_forwarding() {
    #[cfg(unix)]
    forward::install();
}

/// Keeps a child's process group registered with the signal forwarder for
/// as long as the guard lives.
struct ChildGuard {
    #[cfg(unix)]
    slot: Option<usize>,
}

impl ChildGuard {
    fn register(pid: u32) -> Self {
        #[cfg(unix)]
        {
            Self {
                slot: forward::register(pid),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            Self {}
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(slot) = self.slot {
            forward::unregister(slot);
        }
    }
}

fn spawn_error(name: &str, cmd: &str, e: std::io::Error, s: Span) -> Diagnostic {
    // execve refuses a directory with the same "permission denied" it uses
    // for a file without the execute bit; say which it was.
    if e.kind() == std::io::ErrorKind::PermissionDenied && std::path::Path::new(cmd).is_dir() {
        return runtime(format!(
            "{name}: could not start `{cmd}`: it is a directory, not a program"
        ))
        .code("E552")
        .at(s)
        .with_subject(cmd.to_string())
        .with_hint("name the program inside it, or check the path");
    }
    #[cfg(windows)]
    if e.kind() == std::io::ErrorKind::NotFound
        && matches!(
            cmd.to_ascii_lowercase().as_str(),
            "bash" | "sh" | "bash.exe" | "sh.exe"
        )
    {
        return runtime(format!(
            "{name}: could not start `{cmd}`: no shell of that name is on PATH outside System32"
        ))
        .code("E551")
        .at(s)
        .with_subject(cmd.to_string())
        .with_hint("Git for Windows or MSYS2 put bash and sh on PATH; the WSL launcher in System32 is never used for a hop");
    }
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

/// A Windows Job Object around a hop: every process the child starts joins
/// it, so a timeout ends the whole tree the way the process group does on
/// Unix, and closing the last handle (cig itself dying) takes what is left
/// with it (`KILL_ON_JOB_CLOSE`). On other platforms there is no job, and
/// `new` says so.
#[cfg_attr(not(windows), allow(dead_code))]
struct Job {
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

impl Job {
    fn new() -> Option<Job> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::JobObjects::{
                CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            };
            // SAFETY: plain Win32 calls with valid arguments; the handle is
            // owned by the Job and closed in Drop.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    windows_sys::Win32::Foundation::CloseHandle(handle);
                    return None;
                }
                Some(Job { handle })
            }
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// Put a freshly spawned child in the job. The child is still suspended
    /// (see `start`), so nothing it would start can slip out before it is
    /// in. False when Windows refused, which leaves the hop with no job:
    /// the caller says so once.
    fn assign(&self, child: &Child) -> bool {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
            // SAFETY: both handles are valid for the duration of the call.
            unsafe { AssignProcessToJobObject(self.handle, child.as_raw_handle() as _) != 0 }
        }
        #[cfg(not(windows))]
        {
            let _ = child;
            true
        }
    }

    /// End every process in the job.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn terminate(&self) {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;
            // SAFETY: the handle is valid; the exit code is ours to pick.
            unsafe {
                TerminateJobObject(self.handle, 1);
            }
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            // SAFETY: closing a handle this struct owns, once.
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.handle);
            }
        }
    }
}

/// Start a hop's child so that it joins its job before it runs a single
/// instruction: on Windows the process is created suspended, assigned, then
/// resumed, so a grandchild it starts at once is in the job too (a process
/// created before the assignment never joins it). Elsewhere a plain spawn.
fn start(c: &mut Command, job: Option<&Job>) -> std::io::Result<Child> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_SUSPENDED: u32 = 0x0000_0004;
        if job.is_some() {
            c.creation_flags(CREATE_SUSPENDED);
        }
        let mut child = c.spawn()?;
        if let Some(j) = job {
            let joined = j.assign(&child);
            if let Err(e) = resume(&child) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::other(format!(
                    "the process was created but could not be resumed: {e}"
                )));
            }
            if !joined {
                warn_no_job();
            }
        }
        Ok(child)
    }
    #[cfg(not(windows))]
    {
        let child = c.spawn()?;
        if let Some(j) = job {
            j.assign(&child);
        }
        Ok(child)
    }
}

/// Resume every thread of a process created suspended (one, at that
/// point). Toolhelp lists the threads by owner; `ResumeThread` lets them go.
#[cfg(windows)]
fn resume(child: &Child) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    let pid = child.id();
    // SAFETY: plain Win32 calls; every handle opened here is closed here.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = 0u32;
        let mut ok = Thread32First(snap, &mut entry) != 0;
        while ok {
            if entry.th32OwnerProcessID == pid {
                let h = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !h.is_null() {
                    if ResumeThread(h) != u32::MAX {
                        resumed += 1;
                    }
                    CloseHandle(h);
                }
            }
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            ok = Thread32Next(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        if resumed == 0 {
            return Err(std::io::Error::other(
                "no thread of the new process could be resumed",
            ));
        }
    }
    Ok(())
}

/// Said once per process: the hop runs without a job, so a timeout may
/// leave what it started running. A CI log then explains a hang.
#[cfg(windows)]
fn warn_no_job() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        eprintln!(
            "warning: Windows refused to put a hop's child in a job object ({}); what that hop starts may outlive a timeout",
            std::io::Error::last_os_error()
        );
    });
}

/// Kill a child and everything in its process group: SIGTERM, a grace
/// period, then SIGKILL. Returns once the group is gone. On Windows the
/// job the child was put in is terminated, which reaches the grandchildren
/// a plain kill would leave running and holding the pipes.
fn kill_group(child: &mut Child, grace_ms: u64, job: Option<&Job>) {
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
        if let Some(j) = job {
            j.terminate();
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    #[cfg(unix)]
    let _ = job;
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
    let deadline = started + Duration::from_millis(o.timeout_ms);
    check_cwd(name, o, s)?;
    install_signal_forwarding();
    let mut c = command(cmd, args, o).ok_or_else(|| {
        spawn_error(
            name,
            cmd,
            std::io::Error::from(std::io::ErrorKind::NotFound),
            s,
        )
    })?;
    let job = Job::new();
    let mut child = start(&mut c, job.as_ref()).map_err(|e| spawn_error(name, cmd, e, s))?;
    let _guard = ChildGuard::register(child.id());
    // Readers first, then a writer thread for stdin: writing before reading
    // deadlocks as soon as the child fills a pipe while waiting for its input.
    let (out_cap, err_cap) = readers(&mut child);
    let stdin_thread = child
        .stdin
        .take()
        .zip(o.stdin.clone())
        .map(|(mut pipe, text)| {
            std::thread::spawn(move || {
                // A closed pipe is not an error: the child may not read stdin.
                let _ = pipe.write_all(text.as_bytes());
            })
        });
    let label = hop_label(cmd, args);
    let waited = child
        .wait_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|e| runtime(format!("{name}: {e}")).code("E550").at(s))?;
    match waited {
        None => {
            kill_group(&mut child, o.grace_ms, job.as_ref());
            // A reader still blocked after the kill is held by a process
            // that left the group; it is given a moment, then left behind.
            let _ = wait_closed(
                &[&out_cap, &err_cap],
                Instant::now() + Duration::from_secs(1),
            );
            let out = out_cap.bytes();
            let err = err_cap.bytes();
            if let Some(t) = stdin_thread {
                let _ = t.join();
            }
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
            // The hop is over when its output closes, not when its child
            // exits: a process the child left behind may still hold the
            // pipes. It has until the deadline; past it the rest of the
            // group (the job on Windows) is ended and the hop ran too long.
            if !wait_closed(&[&out_cap, &err_cap], deadline) {
                let held = match (out_cap.is_open(), err_cap.is_open()) {
                    (true, true) => "stdout and stderr",
                    (true, false) => "stdout",
                    _ => "stderr",
                };
                kill_group(&mut child, o.grace_ms, job.as_ref());
                let _ = wait_closed(
                    &[&out_cap, &err_cap],
                    Instant::now() + Duration::from_secs(1),
                );
                let out = out_cap.bytes();
                let err = err_cap.bytes();
                if let Some(t) = stdin_thread {
                    let _ = t.join();
                }
                return Err(runtime(format!(
                    "{name}: `{label}` exited {} but a process it left behind kept its {held} open past the {} ms timeout, so the rest of its process group was killed; it had written {} bytes to stdout and {} bytes to stderr{}",
                    status.code().map(|c| c.to_string()).unwrap_or_else(|| "by signal".to_string()),
                    o.timeout_ms,
                    out.len(),
                    err.len(),
                    if err.is_empty() { String::new() } else { format!(" (`{}`)", quote(&err)) },
                ))
                .code("E554")
                .at(s)
                .with_subject(cmd.to_string())
                .with_hint("a program the hop starts in the background must close its stdout and stderr (redirect them) or end before the hop does; nothing the hop wrote was passed on"));
            }
            let out = out_cap.bytes();
            let err = err_cap.bytes();
            if let Some(t) = stdin_thread {
                let _ = t.join();
            }
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

/// One stream of a child, read on its own thread into a buffer the hop can
/// look at whenever it likes. `closed` fires when the stream reaches its
/// end; a stream that never closes (a leftover process holds it) leaves
/// the thread behind, with what was read so far still readable.
struct Capture {
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    closed: std::sync::mpsc::Receiver<()>,
    open: std::cell::Cell<bool>,
}

impl Capture {
    fn new<R: Read + Send + 'static>(pipe: Option<R>) -> Capture {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, rx) = std::sync::mpsc::channel();
        let into = buf.clone();
        std::thread::spawn(move || {
            if let Some(mut p) = pipe {
                let mut chunk = [0u8; 8192];
                loop {
                    match p.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => into
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .extend_from_slice(&chunk[..n]),
                    }
                }
            }
            let _ = tx.send(());
        });
        Capture {
            buf,
            closed: rx,
            open: std::cell::Cell::new(true),
        }
    }

    /// Wait until the stream closes or `deadline` passes.
    fn wait_closed(&self, deadline: Instant) -> bool {
        if !self.open.get() {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match self.closed.recv_timeout(left) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                self.open.set(false);
                true
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
        }
    }

    fn is_open(&self) -> bool {
        self.open.get()
    }

    fn bytes(&self) -> Vec<u8> {
        self.buf.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Wait for every capture to close, or for the deadline. True when all closed.
fn wait_closed(caps: &[&Capture], deadline: Instant) -> bool {
    let mut all = true;
    for c in caps {
        all &= c.wait_closed(deadline);
    }
    all
}

/// Read both streams concurrently: a child filling stderr while stdout is
/// awaited is the classic deadlock.
fn readers(child: &mut Child) -> (Capture, Capture) {
    (
        Capture::new(child.stdout.take()),
        Capture::new(child.stderr.take()),
    )
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
        touches: o.touches.clone(),
    };
    if !o.touches.is_empty() && i.kernel.pack().is_none() {
        return Err(cigscript_burn(format!(
            "{name}: `touches` was given, but the script declares no pack {{ }}; the kernel has nothing to watch"
        ))
        .code("E757")
        .at(s)
        .with_hint("declare pack { \".\" } (or the roots the hop writes under) as the first statement"));
    }
    if i.effect(name, op, s)? == Decision::Simulate {
        return Ok(None);
    }
    let watch = i.kernel.pack_watch_begin(o.cwd.as_deref(), &o.touches);
    let ran = run_child(name, cmd, args, o, s);
    if let Some(w) = watch {
        for warning in &w.warnings {
            let _ = writeln!(i.err, "warning[E757 burn]: {warning}");
        }
        let writes = i.kernel.pack_watch_end(w);
        for warning in i.kernel.take_late_warnings() {
            let _ = writeln!(i.err, "warning[E757 burn]: {warning}");
        }
        if !writes.outside.is_empty() {
            let shown: Vec<String> = writes
                .outside
                .iter()
                .take(5)
                .map(|p| p.display().to_string())
                .collect();
            let _ = writeln!(
                i.err,
                "warning[E752 burn]: `{}` wrote outside the pack: {} path{} ({}{}); the kernel cannot undo them",
                hop_label(cmd, args),
                writes.outside.len(),
                if writes.outside.len() == 1 { "" } else { "s" },
                shown.join(", "),
                if writes.outside.len() > 5 { ", ..." } else { "" }
            );
        }
    }
    let raw = ran?;
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
                // `export FOO=bar`, as `export -p` and .env files write it.
                let line = line
                    .strip_prefix("export ")
                    .map(str::trim_start)
                    .unwrap_or(line);
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
/// The shell `proc.shell` runs a line with, as a program and the arguments
/// before the line. Unix: `sh -c`. Windows: `sh -c` too when an `sh` is on
/// PATH (Git for Windows and MSYS2 ship one; the WSL launcher does not
/// count), else `cmd /d /s /c` with the line reaching cmd exactly as
/// written. The plan names which, so a dry run is honest about it.
pub fn shell_command(cwd: Option<&std::path::Path>) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        if resolve_program("sh", cwd).is_some() {
            ("sh".to_string(), vec!["-c".to_string()])
        } else {
            (
                "cmd".to_string(),
                vec!["/d".to_string(), "/s".to_string(), "/c".to_string()],
            )
        }
    }
    #[cfg(not(windows))]
    {
        let _ = cwd;
        ("sh".to_string(), vec!["-c".to_string()])
    }
}

fn shell(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let script = expect_str(a, 0, "proc.shell", s)?.to_string();
    let o = Options::parse(a, 1, "proc.shell", s, None)?;
    let (cmd, mut args) = shell_command(o.cwd.as_deref());
    args.push(script.clone());
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
        touches: o.touches.clone(),
    };
    if i.effect("proc.pipe", op, s)? == Decision::Simulate {
        return Ok(result_map(0, "", "", 0, true));
    }
    check_cwd("proc.pipe", &o, s)?;
    install_signal_forwarding();
    let started = Instant::now();
    // The clock starts before the first stage does: everything below,
    // feeding stdin included, is inside the timeout.
    let deadline = started + Duration::from_millis(o.timeout_ms);
    let mut children: Vec<Child> = Vec::new();
    // Every stage joins one job on Windows, so one timeout ends them all.
    let job = Job::new();
    // Registered with the signal forwarder for as long as the pipeline runs.
    let mut _group_guard: Option<ChildGuard> = None;
    let mut err_readers: Vec<Capture> = Vec::new();
    let mut stdin_thread: Option<std::thread::JoinHandle<()>> = None;
    let mut previous_out: Option<std::process::ChildStdout> = None;
    for (n, (cmd, args)) in stages.iter().enumerate() {
        let Some(mut c) = command(cmd, args, &o) else {
            for mut earlier in children.drain(..) {
                kill_group(&mut earlier, 0, job.as_ref());
            }
            return Err(spawn_error(
                &format!("proc.pipe (stage {})", n + 1),
                cmd,
                std::io::Error::from(std::io::ErrorKind::NotFound),
                s,
            ));
        };
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
        let mut child = start(&mut c, job.as_ref()).map_err(|e| {
            for mut earlier in children.drain(..) {
                kill_group(&mut earlier, 0, job.as_ref());
            }
            spawn_error(&format!("proc.pipe (stage {})", n + 1), cmd, e, s)
        })?;
        if n == 0 {
            // Fed from a thread of its own: a first stage that never reads
            // its stdin would otherwise hold the pipeline, and its timeout,
            // on a full pipe.
            if let (Some(text), Some(mut pipe_in)) = (o.stdin.clone(), child.stdin.take()) {
                stdin_thread = Some(std::thread::spawn(move || {
                    let _ = pipe_in.write_all(text.as_bytes());
                }));
            }
        }
        err_readers.push(Capture::new(child.stderr.take()));
        if n + 1 < stages.len() {
            previous_out = child.stdout.take();
        }
        if n == 0 {
            _group_guard = Some(ChildGuard::register(child.id()));
        }
        children.push(child);
    }
    let last = children.len() - 1;
    let out_reader = Capture::new(children[last].stdout.take());
    let mut codes: Vec<Option<i32>> = vec![None; children.len()];
    // Only Unix fills this in; Windows reports exit codes alone.
    #[cfg_attr(windows, allow(unused_mut))]
    let mut signals: Vec<i32> = vec![0; children.len()];
    let mut timed_out = false;
    'wait: loop {
        for (n, child) in children.iter_mut().enumerate() {
            if codes[n].is_none() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        codes[n] = Some(status.code().unwrap_or(-1));
                        #[cfg(unix)]
                        {
                            use std::os::unix::process::ExitStatusExt;
                            signals[n] = status.signal().unwrap_or(0);
                        }
                    }
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
    // Every stage has exited; its output has until the deadline to close,
    // in case a stage left a process behind that holds a pipe.
    let mut all_caps: Vec<&Capture> = err_readers.iter().collect();
    all_caps.push(&out_reader);
    let closed = !timed_out && wait_closed(&all_caps, deadline);
    if timed_out || !closed {
        for child in children.iter_mut() {
            kill_group(child, o.grace_ms, job.as_ref());
        }
        let _ = wait_closed(&all_caps, Instant::now() + Duration::from_secs(1));
        if let Some(t) = stdin_thread.take() {
            let _ = t.join();
        }
        return Err(runtime(if timed_out {
            format!(
                "proc.pipe: the pipeline ran longer than {} ms and was killed",
                o.timeout_ms
            )
        } else {
            format!(
                "proc.pipe: every stage exited, but a process one left behind kept the pipeline's output open past the {} ms timeout, so the rest of its process group was killed",
                o.timeout_ms
            )
        })
        .code("E554")
        .at(s)
        .with_hint(
            "raise {timeout_ms: N}, or split the pipeline; nothing it wrote was passed on",
        ));
    }
    if let Some(t) = stdin_thread.take() {
        let _ = t.join();
    }
    let out_bytes = out_reader.bytes();
    let errs: Vec<Vec<u8>> = err_readers.iter().map(Capture::bytes).collect();
    let duration_ms = started.elapsed().as_millis() as i64;
    // Every stage but the last must exit 0; the last honours the contract.
    // A stage ended by SIGPIPE was cut off by its consumer closing early
    // (`yes | head -1`); whatever the consumer did next is the story, never
    // the producer's death.
    let closed_early = |n: usize| -> bool { n < last && signals[n] == SIGPIPE };
    for (n, code) in codes.iter().enumerate() {
        let code = code.unwrap_or(-1);
        let is_last = n == last;
        let allowed: Vec<i64> = if is_last {
            o.ok.clone().unwrap_or_else(|| vec![0])
        } else {
            vec![0]
        };
        if closed_early(n) {
            continue;
        }
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
                .enumerate()
                .map(|(n, ((cmd, args), code))| {
                    let mut sm = IndexMap::new();
                    sm.insert("cmd".to_string(), Value::str(hop_label(cmd, args)));
                    sm.insert("code".to_string(), Value::Int(code.unwrap_or(-1) as i64));
                    if signals[n] != 0 {
                        sm.insert("signal".to_string(), Value::Int(signals[n] as i64));
                    }
                    if closed_early(n) {
                        sm.insert("closed_early".to_string(), Value::Bool(true));
                    }
                    Value::map(sm)
                })
                .collect(),
        ),
    );
    Ok(Value::map(m))
}

fn which(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let name = expect_str(a, 0, "proc.which", s)?;
    // Exactly what proc.run would start: a path against the current
    // directory, a bare name on PATH (with PATHEXT on Windows). A path is
    // answered as it was spelled, unless PATHEXT completed its name.
    let is_path = name.contains('/') || name.contains(std::path::MAIN_SEPARATOR);
    Ok(match resolve_program(name, None) {
        Some(p) => {
            let same_name = p.file_name() == std::path::Path::new(name).file_name();
            if is_path && same_name {
                Value::str(name)
            } else {
                Value::str(p.to_string_lossy())
            }
        }
        None => Value::Null,
    })
}

fn cigscript_burn(msg: String) -> Diagnostic {
    crate::diagnostics::burn(msg)
}
