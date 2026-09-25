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

/// Replace the home directory with `~` in a path.
pub fn tilde(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = home.to_string_lossy().to_string();
        if !home.is_empty() && s.starts_with(&home) {
            return format!("~{}", &s[home.len()..]);
        }
    }
    s
}

/// Redact free text: home paths become `~`, token-shaped words are removed.
pub fn redact_text(text: &str) -> String {
    let mut out = text.to_string();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = home.to_string_lossy().to_string();
        if !home.is_empty() {
            out = out.replace(&home, "~");
        }
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

/// Keep only frames from this crate, with paths made crate-relative.
fn filter_backtrace(bt: &str) -> Vec<String> {
    let mut frames = Vec::new();
    let lines: Vec<&str> = bt.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();
        let is_frame = line
            .split_once(": ")
            .is_some_and(|(n, _)| n.trim().chars().all(|c| c.is_ascii_digit()));
        if is_frame {
            let symbol = line.split_once(": ").map(|(_, s)| s).unwrap_or(line);
            let at = lines
                .get(i + 1)
                .map(|l| l.trim())
                .filter(|l| l.starts_with("at "))
                .unwrap_or("");
            if symbol.contains("cigscript")
                || symbol.starts_with("cig::")
                || at.contains("cigscript/src")
                || at.contains("/src/cli/")
            {
                let location = at
                    .strip_prefix("at ")
                    .map(crate_relative)
                    .unwrap_or_default();
                frames.push(if location.is_empty() {
                    symbol.to_string()
                } else {
                    format!("{symbol} ({location})")
                });
            }
        }
        i += 1;
    }
    frames.truncate(40);
    frames
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
}
