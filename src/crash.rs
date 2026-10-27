// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! Crash reports: written locally on every panic, sent to the issue tracker
//! only when the user says so.
//!
//! The report is an allowlist. It never contains script source, environment
//! variable values, file contents or tokens. `cig crash show <id>` prints
//! exactly what would be sent.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CrashReport {
    pub id: String,
    pub fingerprint: String,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub timestamp: String,
    pub command: Vec<String>,
    pub cwd: String,
    pub panic_message: String,
    pub panic_location: String,
    pub backtrace: Vec<String>,
    #[serde(default)]
    pub active_run_id: Option<String>,
    #[serde(default)]
    pub sent: Option<String>,
    /// After a crash mid-run: whether the run's reversible burns were put
    /// back before this report was sent. `null` when no run was in progress.
    #[serde(default)]
    pub rolled_back: Option<bool>,
}

static LAST: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
static CONTEXT: OnceLock<Mutex<Option<String>>> = OnceLock::new();

pub fn crashes_dir() -> PathBuf {
    crate::burn::runs::home_dir().join("crashes")
}

/// Remember the run in progress so a crash report can name it.
pub fn set_active_run(id: Option<String>) {
    *CONTEXT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = id;
}

/// The run in progress, as `cig.run_id()` reports it to the script.
pub fn active_run() -> Option<String> {
    CONTEXT
        .get()
        .and_then(|m| m.lock().ok().and_then(|g| g.clone()))
}

/// The report written by the most recent panic in this process, if any.
pub fn last_report() -> Option<PathBuf> {
    LAST.get()
        .and_then(|m| m.lock().ok().and_then(|g| g.clone()))
}

/// Install the panic hook. Writes the report, prints one line, and leaves
/// the decision about sending to the caller after unwinding.
pub fn install_hook() {
    LAST.get_or_init(|| Mutex::new(None));
    std::panic::set_hook(Box::new(|info| {
        let message = match info.payload().downcast_ref::<&str>() {
            Some(s) => s.to_string(),
            None => match info.payload().downcast_ref::<String>() {
                Some(s) => s.clone(),
                None => "panic with a non-string payload".to_string(),
            },
        };
        if message.contains("Broken pipe") {
            // Whoever was reading cig's output went away (`cig ... | head`).
            // That is not a crash, and a report could not be printed anyway:
            // leave the way a C program killed by SIGPIPE does.
            std::process::exit(141);
        }
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", crate_relative(l.file()), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let report = build(&message, &location, &backtrace);
        match write(&report) {
            Ok(path) => {
                if let Some(m) = LAST.get() {
                    if let Ok(mut g) = m.lock() {
                        *g = Some(path.clone());
                    }
                }
                eprintln!(
                    "\nDon't see any cigarettes.\nerror[E901 internal]: cig crashed: {}\n  --> {}\n  = crash report saved to {}\n  = explain: cig explain E901",
                    truncate(&report.panic_message, 200),
                    report.panic_location,
                    path.display()
                );
            }
            Err(e) => {
                eprintln!(
                    "\nDon't see any cigarettes.\nerror[E901 internal]: cig crashed: {} at {}\n  = could not save a crash report: {e}",
                    truncate(&message, 200),
                    location
                );
            }
        }
        if std::env::var_os("RUST_BACKTRACE").is_some() || std::env::var_os("CIG_DEBUG").is_some() {
            eprintln!("{backtrace}");
        }
    }));
}

fn build(message: &str, location: &str, backtrace: &str) -> CrashReport {
    let now = chrono::Utc::now();
    let fingerprint = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(location.as_bytes());
        h.update(b"\n");
        h.update(strip_numbers(message).as_bytes());
        hex::encode(h.finalize())[..8].to_string()
    };
    let active_run_id = CONTEXT
        .get()
        .and_then(|m| m.lock().ok().and_then(|g| g.clone()));
    CrashReport {
        id: format!("{}-{}", now.format("%Y%m%dT%H%M%S"), &fingerprint[..6]),
        fingerprint,
        version: crate::VERSION.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        timestamp: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        command: std::env::args().map(|a| redact_arg(&a)).collect(),
        cwd: std::env::current_dir()
            .map(|p| tilde(&p))
            .unwrap_or_else(|_| "?".to_string()),
        panic_message: redact_text(&truncate(message, 500)),
        panic_location: location.to_string(),
        backtrace: filter_backtrace(backtrace),
        active_run_id,
        sent: None,
        rolled_back: None,
    }
}

pub fn write(report: &CrashReport) -> io::Result<PathBuf> {
    let dir = crashes_dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", report.id));
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}

pub fn load(path: &Path) -> io::Result<CrashReport> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// All reports, newest first.
pub fn list() -> io::Result<Vec<CrashReport>> {
    let dir = crashes_dir();
    let mut out = Vec::new();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.extension().is_some_and(|e| e == "json") {
            if let Ok(r) = load(&p) {
                out.push(r);
            }
        }
    }
    out.sort_by(|a, b| b.id.cmp(&a.id));
    Ok(out)
}

pub fn find(id_or_prefix: &str) -> io::Result<Option<CrashReport>> {
    let all = list()?;
    if let Some(r) = all.iter().find(|r| r.id == id_or_prefix) {
        return Ok(Some(r.clone()));
    }
    let m: Vec<&CrashReport> = all
        .iter()
        .filter(|r| r.id.starts_with(id_or_prefix))
        .collect();
    match m.len() {
        0 => Ok(None),
        1 => Ok(Some(m[0].clone())),
        n => Err(io::Error::other(format!(
            "`{id_or_prefix}` matches {n} reports; give more of the id"
        ))),
    }
}

pub fn path_of(report: &CrashReport) -> PathBuf {
    crashes_dir().join(format!("{}.json", report.id))
}

// ----- redaction ---------------------------------------------------------------------

const SECRET_FLAGS: &[&str] = &["token", "key", "secret", "password", "passwd", "auth"];

/// Redact a command-line argument: secret-shaped values and anything that
/// looks like a path or URL are replaced or reduced to a basename.
pub fn redact_arg(arg: &str) -> String {
    let lower = arg.to_ascii_lowercase();
    if let Some((flag, value)) = arg.split_once('=') {
        if SECRET_FLAGS
            .iter()
            .any(|s| flag.to_ascii_lowercase().contains(s))
        {
            return format!("{flag}=<redacted>");
        }
        let _ = value;
    }
    if SECRET_FLAGS.iter().any(|s| lower.contains(s)) && !arg.starts_with('-') {
        return "<redacted>".to_string();
    }
    if looks_like_token(arg) {
        return "<redacted>".to_string();
    }
    if arg.contains("://") {
        return "<url>".to_string();
    }
    if arg.contains('/') || arg.contains('\\') {
        let base = Path::new(arg)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "<path>".to_string());
        return format!(".../{base}");
    }
    arg.to_string()
}

fn looks_like_token(s: &str) -> bool {
    let prefixes = ["ghp_", "gho_", "ghs_", "github_pat_", "sk-", "xox", "AKIA"];
    if prefixes.iter().any(|p| s.starts_with(p)) {
        return true;
    }
    s.len() >= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The home directory as written in the environment and as the operating
/// system resolves it (macOS keeps `/var` as a link to `/private/var`, so a
/// working directory can come back in either spelling); longest first.
fn home_spellings() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
        return Vec::new();
    };
    let raw = home.to_string_lossy().to_string();
    if raw.is_empty() {
        return Vec::new();
    }
    let mut out = vec![raw.clone()];
    if let Ok(canonical) = fs::canonicalize(&raw) {
        let c = canonical.to_string_lossy().to_string();
        if c != raw {
            out.push(c);
        }
    }
    out.sort_by_key(|h| std::cmp::Reverse(h.len()));
    out
}

/// Replace the home directory with `~` in a path.
pub fn tilde(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    for home in home_spellings() {
        if s.starts_with(&home) {
            return format!("~{}", &s[home.len()..]);
        }
    }
    s
}

/// Redact free text: home paths become `~`, token-shaped words are removed.
pub fn redact_text(text: &str) -> String {
    let mut out = text.to_string();
    for home in home_spellings() {
        out = out.replace(&home, "~");
    }
    out.split_whitespace()
        .map(|w| if looks_like_token(w) { "<redacted>" } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_numbers(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .collect()
}

fn crate_relative(path: &str) -> String {
    let p = path.replace('\\', "/");
    if p.starts_with("src/") {
        // A panic location inside this crate is already crate-relative.
        return p;
    }
    if let Some(i) = p.find("/src/") {
        return p[i + 1..].to_string();
    }
    if let Some(i) = p.rfind("/rustc/") {
        return format!("rustc{}", &p[i + 6..]);
    }
    Path::new(&p)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or(p)
}

/// The frames a report keeps: every frame of cig's own code (a symbol that
/// mentions `cigscript::` or `cig::` anywhere, so a closure inlined into a
/// `std` frame under LTO still counts), never the panic hook itself, and the
/// one library frame our code called into when the panic came from `std`
/// or `core` (an `unwrap` on `None`, an index out of range). Paths are made
/// crate-relative.
fn filter_backtrace(bt: &str) -> Vec<String> {
    let lines: Vec<&str> = bt.lines().collect();
    let mut frames: Vec<(String, String, String)> = Vec::new(); // symbol, raw at, location
    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        let Some((n, symbol)) = line.split_once(": ") else {
            continue;
        };
        let n = n.trim();
        if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let at = lines
            .get(i + 1)
            .map(|l| l.trim())
            .filter(|l| l.starts_with("at "))
            .map(|l| l[3..].to_string())
            .unwrap_or_default();
        let location = if at.is_empty() {
            String::new()
        } else {
            crate_relative(&at)
        };
        frames.push((symbol.to_string(), at, location));
    }
    // A direct frame is one of our functions. An indirect one is a `std`
    // frame that carries our code as a generic parameter, which is all that
    // is left of a closure inlined under LTO; it stands in only when no
    // direct frame survived.
    let direct = |symbol: &str, at: &str| {
        let at = at.to_ascii_lowercase();
        (symbol.starts_with("cigscript::")
            || symbol.starts_with("cig::")
            || symbol.starts_with("<cigscript::")
            || symbol.starts_with("<cig::")
            || at.contains("cigscript/src")
            || at.contains("/src/cli/"))
            && !symbol.contains("cigscript::crash::")
    };
    let indirect = |symbol: &str| {
        (symbol.contains("cigscript::") || symbol.contains("cig::"))
            && !symbol.contains("cigscript::crash::")
    };
    let any_direct = frames.iter().any(|f| direct(&f.0, &f.1));
    let ours = |symbol: &str, at: &str| {
        if any_direct {
            direct(symbol, at)
        } else {
            indirect(symbol)
        }
    };
    let machinery = |symbol: &str| {
        symbol.contains("panicking")
            || symbol.contains("rust_panic")
            || symbol.contains("rust_begin_unwind")
            || symbol.contains("lang_start")
            || symbol.contains("std::rt::")
            || symbol.contains("__rust_end_short_backtrace")
    };
    let render = |f: &(String, String, String)| {
        if f.2.is_empty() {
            f.0.clone()
        } else {
            format!("{} ({})", f.0, f.2)
        }
    };
    let mut out = Vec::new();
    if let Some(k) = frames.iter().position(|f| ours(&f.0, &f.1)) {
        let library = (0..k).rev().find(|&j| {
            let s = frames[j].0.as_str();
            (s.starts_with("std::") || s.starts_with("core::") || s.starts_with("alloc::"))
                && !machinery(s)
        });
        if let Some(j) = library {
            out.push(render(&frames[j]));
        }
    }
    out.extend(frames.iter().filter(|f| ours(&f.0, &f.1)).map(render));
    out.truncate(40);
    out
}

pub fn truncate(s: &str, max: usize) -> String {
    crate::value::truncate(s, max)
}

// ----- rendering and sending ------------------------------------------------------------

pub fn issue_title(r: &CrashReport) -> String {
    format!(
        "crash [{}]: {} (cig {}, {})",
        r.fingerprint,
        truncate(&r.panic_message, 60),
        r.version,
        r.os
    )
}

pub fn issue_body(r: &CrashReport) -> String {
    let json = serde_json::to_string_pretty(r).unwrap_or_default();
    format!(
        "Automated crash report from `cig`. The person who hit this chose to send it.\n\n\
         - version: {}\n- os/arch: {}/{}\n- location: `{}`\n- fingerprint: `{}`\n\n\
         <details><summary>Full report</summary>\n\n```json\n{}\n```\n</details>\n",
        r.version, r.os, r.arch, r.panic_location, r.fingerprint, json
    )
}

pub enum SendOutcome {
    Filed(String),
    Commented(String),
    /// No tool could file it; the user gets a URL or a file to attach.
    Manual(String),
}

fn which(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .any(|d| d.join(name).is_file() || d.join(format!("{name}.exe")).is_file())
        })
        .unwrap_or(false)
}

/// Send a report to `repo` (owner/name). Tries `gh`, then a token with
/// curl, then hands back a prefilled URL.
pub fn send(report: &mut CrashReport, repo: &str) -> Result<SendOutcome, String> {
    let title = issue_title(report);
    let body = issue_body(report);
    let outcome = if which("gh") {
        send_with_gh(report, repo, &title, &body)?
    } else if std::env::var("CIG_GITHUB_TOKEN").is_ok_and(|t| !t.is_empty()) && which("curl") {
        send_with_curl(repo, &title, &body)?
    } else {
        let url = format!(
            "https://github.com/{repo}/issues/new?title={}&body={}",
            urlencode(&title),
            urlencode(&truncate(&body, 5000))
        );
        SendOutcome::Manual(url)
    };
    match &outcome {
        SendOutcome::Filed(url) | SendOutcome::Commented(url) => {
            report.sent = Some(url.clone());
            let _ = write(report);
        }
        SendOutcome::Manual(_) => {}
    }
    Ok(outcome)
}

fn send_with_gh(
    report: &CrashReport,
    repo: &str,
    title: &str,
    body: &str,
) -> Result<SendOutcome, String> {
    // Duplicate control: one issue per fingerprint, one comment per day after that.
    let search = std::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "-R",
            repo,
            "--state",
            "all",
            "--search",
            &format!("[{}] in:title", report.fingerprint),
            "--json",
            "number,url",
            "--limit",
            "1",
        ])
        .output()
        .map_err(|e| format!("gh: {e}"))?;
    if search.status.success() {
        let text = String::from_utf8_lossy(&search.stdout);
        if let Ok(serde_json::Value::Array(items)) =
            serde_json::from_str::<serde_json::Value>(&text)
        {
            if let Some(first) = items.first() {
                let number = first["number"].as_i64().unwrap_or(0);
                let url = first["url"].as_str().unwrap_or("").to_string();
                if number > 0 {
                    if recently_commented(&report.fingerprint) {
                        return Ok(SendOutcome::Commented(url));
                    }
                    let comment = format!(
                        "Seen again: cig {} on {}/{} at {} (report id `{}`).",
                        report.version, report.os, report.arch, report.timestamp, report.id
                    );
                    let out = std::process::Command::new("gh")
                        .args([
                            "issue",
                            "comment",
                            &number.to_string(),
                            "-R",
                            repo,
                            "--body",
                            &comment,
                        ])
                        .output()
                        .map_err(|e| format!("gh: {e}"))?;
                    if out.status.success() {
                        mark_commented(&report.fingerprint);
                        return Ok(SendOutcome::Commented(url));
                    }
                }
            }
        }
    }
    let tmp = std::env::temp_dir().join(format!("cig-crash-{}.md", report.id));
    fs::write(&tmp, body).map_err(|e| e.to_string())?;
    let out = std::process::Command::new("gh")
        .args([
            "issue",
            "create",
            "-R",
            repo,
            "--title",
            title,
            "--body-file",
        ])
        .arg(&tmp)
        .output()
        .map_err(|e| format!("gh: {e}"))?;
    let _ = fs::remove_file(&tmp);
    if out.status.success() {
        Ok(SendOutcome::Filed(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("Could not resolve") || err.contains("404") {
            Err(format!("gh cannot reach {repo}; if the repository is private you need access to file issues ({err})"))
        } else {
            Err(format!("gh issue create failed: {err}"))
        }
    }
}

fn send_with_curl(repo: &str, title: &str, body: &str) -> Result<SendOutcome, String> {
    let token = std::env::var("CIG_GITHUB_TOKEN").map_err(|_| "no token".to_string())?;
    // The token travels in a config file, never on the command line.
    let cfg = std::env::temp_dir().join(format!("cig-curl-{}.cfg", std::process::id()));
    fs::write(&cfg, format!("header = \"Authorization: Bearer {token}\"\nheader = \"Accept: application/vnd.github+json\"\n"))
        .map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&cfg, fs::Permissions::from_mode(0o600));
    }
    let payload = serde_json::json!({"title": title, "body": body}).to_string();
    let out = std::process::Command::new("curl")
        .args([
            "-sS",
            "-f",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--max-time",
            "30",
            "-K",
        ])
        .arg(&cfg)
        .args(["-X", "POST", "-d", &payload])
        .arg(format!("https://api.github.com/repos/{repo}/issues"))
        .output();
    let _ = fs::remove_file(&cfg);
    let out = out.map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "curl failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    Ok(SendOutcome::Filed(
        v["html_url"].as_str().unwrap_or("").to_string(),
    ))
}

fn marker(fp: &str) -> PathBuf {
    crashes_dir().join(format!(".commented-{fp}"))
}

fn recently_commented(fp: &str) -> bool {
    fs::metadata(marker(fp))
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().map(|d| d.as_secs() < 86_400).unwrap_or(false))
        .unwrap_or(false)
}

fn mark_commented(fp: &str) {
    let _ = fs::write(marker(fp), b"");
}

pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_rules() {
        assert_eq!(redact_arg("--token=abc"), "--token=<redacted>");
        assert_eq!(
            redact_arg("ghp_0123456789abcdef0123456789abcdef0123"),
            "<redacted>"
        );
        assert_eq!(redact_arg("https://example.com/x"), "<url>");
        assert_eq!(redact_arg("/home/someone/scripts/tidy.cig"), ".../tidy.cig");
        assert_eq!(redact_arg("run"), "run");
        assert_eq!(redact_arg("--dry-run"), "--dry-run");
        assert_eq!(urlencode("a b&c"), "a%20b%26c");
    }

    #[test]
    fn backtrace_keeps_only_our_frames() {
        let bt = "   0: std::panicking::begin_panic\n             at /rustc/abc/library/std/src/panicking.rs:1\n   1: cigscript::interp::eval::x\n             at /home/u/CigScript/src/interp/eval.rs:10:5\n   2: core::ops::function::FnOnce::call_once\n             at /rustc/abc/library/core/src/ops/function.rs:2\n";
        let frames = filter_backtrace(bt);
        assert_eq!(frames.len(), 1);
        assert_eq!(
            frames[0],
            "cigscript::interp::eval::x (src/interp/eval.rs:10:5)"
        );
    }

    #[test]
    fn the_panic_hook_is_not_in_the_backtrace_it_reports() {
        let bt = "   0: cigscript::crash::install_hook::{closure#1}\n   1: std::panicking::panic_with_hook\n   2: std::panicking::begin_panic::<&str>\n   3: cigscript::interp::Interp::run_program\n";
        let frames = filter_backtrace(bt);
        assert_eq!(frames, vec!["cigscript::interp::Interp::run_program"]);
    }

    #[test]
    fn an_inlined_closure_frame_is_kept() {
        // Under LTO the closure in `main` is inlined into a `std` frame and
        // survives only as a generic parameter.
        let bt = "   0: cigscript::crash::install_hook::{closure#1}\n   1: std::panicking::panic_with_hook\n   2: std::panicking::begin_panic_handler::{closure#0}\n   3: std::sys::backtrace::__rust_end_short_backtrace::<std::panicking::begin_panic_handler::{closure#0}, !>\n   4: std::panicking::begin_panic::<&str>\n   5: std::sys::backtrace::__rust_begin_short_backtrace::<cig::main::{closure#0}, i32>\n   6: std::thread::Builder::spawn_unchecked_::<cig::main::{closure#0}, i32>::{closure#1}\n";
        let frames = filter_backtrace(bt);
        assert_eq!(
            frames[0],
            "std::sys::backtrace::__rust_begin_short_backtrace::<cig::main::{closure#0}, i32>"
        );
        assert!(
            frames.iter().all(|f| !f.contains("install_hook")),
            "{frames:?}"
        );
    }

    #[test]
    fn a_panic_inside_a_library_keeps_the_frame_our_code_called() {
        let bt = "   0: cigscript::crash::install_hook::{closure#1}\n   1: std::panicking::panic_with_hook\n   2: core::panicking::panic_fmt\n   3: core::option::unwrap_failed\n   4: cigscript::interp::eval::Interp::eval_expr\n             at /home/u/CigScript/src/interp/eval.rs:10:5\n";
        let frames = filter_backtrace(bt);
        assert_eq!(
            frames,
            vec![
                "core::option::unwrap_failed".to_string(),
                "cigscript::interp::eval::Interp::eval_expr (src/interp/eval.rs:10:5)".to_string()
            ]
        );
    }
}
