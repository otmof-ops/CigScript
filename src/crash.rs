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
    /// What kind of report: a crash (a panic), an internal error (E900), a
    /// tripwire (a promise cig checked after the run and found broken), or a
    /// run bundle `cig report --send` filed.
    #[serde(default)]
    pub kind: Kind,
    /// The tool that made it: `cig`, or another of the family handing its
    /// report through `cig crash submit --origin`.
    #[serde(default = "default_origin")]
    pub origin: String,
    /// Where it goes, when the tool that made it said; cig's `issues_repo`
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// One line for a report that is not a panic: what the tripwire found,
    /// what the internal error said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Kind-specific detail, redacted: a tripwire's mismatches, a bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// The anonymised repro built from the run's journal, when one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repro: Option<crate::repro::Repro>,
    /// The issue it was filed on, or added to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue: Option<Issue>,
    /// The ashtray's bookkeeping: whether sending was asked for, how often
    /// it was tried, why it failed last, when to try again.
    #[serde(default)]
    pub queue: Queue,
    /// The release that fixed it, once known; `announced` once said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_in: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub announced: bool,
}

fn default_origin() -> String {
    "cig".to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Crash,
    Internal,
    Tripwire,
    Report,
}

impl Kind {
    /// The word in an issue title and a label.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Crash => "crash",
            Kind::Internal => "internal",
            Kind::Tripwire => "tripwire",
            Kind::Report => "report",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Issue {
    pub repo: String,
    pub number: u64,
    pub url: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Queue {
    /// The person asked for it to be sent (`cig crash send`, `--send`, or
    /// the one yes that set `reports = always`).
    #[serde(default)]
    pub requested: bool,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_try: Option<String>,
}

/// What the person said about sending reports: the `reports` setting, the
/// older `crash_reports` when that alone is set, and `never` under CI with
/// nothing set, so no pipeline files public issues by surprise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consent {
    Ask,
    Always,
    Never,
}

pub fn under_ci() -> bool {
    use std::io::IsTerminal;
    std::env::var("CI").is_ok_and(|v| !matches!(v.as_str(), "" | "0" | "false"))
        && !std::io::stdin().is_terminal()
}

pub fn consent(cfg: &crate::config::Config) -> Consent {
    let explicit = cfg.is_set("reports")
        || std::env::var_os("CIG_REPORTS").is_some()
        || cfg.is_set("crash_reports")
        || std::env::var_os("CIG_CRASH_REPORTS").is_some();
    let value = if cfg.is_set("reports") || std::env::var_os("CIG_REPORTS").is_some() {
        cfg.get("reports")
    } else if explicit {
        cfg.get("crash_reports")
    } else if under_ci() {
        return Consent::Never;
    } else {
        "ask".to_string()
    };
    match value.as_str() {
        "always" => Consent::Always,
        "never" => Consent::Never,
        _ => Consent::Ask,
    }
}

impl CrashReport {
    /// A report that is not a panic: what cig found (`title`), where
    /// (`location`: a tripwire's name, an error code and place), with its
    /// detail, redacted as everything else is.
    pub fn new(
        kind: Kind,
        location: &str,
        title: &str,
        details: Option<serde_json::Value>,
        run_id: Option<String>,
    ) -> CrashReport {
        let now = chrono::Utc::now();
        let fingerprint = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(kind.word().as_bytes());
            h.update(b"\n");
            h.update(location.as_bytes());
            h.update(b"\n");
            h.update(strip_numbers(title).as_bytes());
            hex::encode(h.finalize())[..8].to_string()
        };
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
            panic_message: redact_text(&truncate(title, 500)),
            panic_location: location.to_string(),
            backtrace: Vec::new(),
            active_run_id: run_id,
            sent: None,
            rolled_back: None,
            kind,
            origin: default_origin(),
            repo: None,
            title: Some(redact_text(&truncate(title, 500))),
            details,
            repro: None,
            issue: None,
            queue: Queue::default(),
            fixed_in: None,
            announced: false,
        }
    }

    /// The one line that says what happened.
    pub fn headline(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.panic_message)
    }

    /// Where it stands: `filed`, `pending`, `failed (reason)` or `local`.
    pub fn state(&self) -> String {
        if self.issue.is_some() || self.sent.is_some() {
            return "filed".to_string();
        }
        if let Some(e) = &self.queue.last_error {
            return format!("failed: {}", truncate(e, 60));
        }
        if self.queue.requested {
            return "pending".to_string();
        }
        "local".to_string()
    }
}

static LAST: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
static CONTEXT: OnceLock<Mutex<Option<String>>> = OnceLock::new();

pub fn crashes_dir() -> PathBuf {
    crate::burn::runs::home_dir().join("crashes")
}

/// Where a report lands before anything is sent, and stays once filed.
/// Reports from before 1.7.0 are read from `crashes/` too.
pub fn ashtray_dir() -> PathBuf {
    crate::burn::runs::home_dir().join("ashtray")
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
        kind: Kind::Crash,
        origin: default_origin(),
        repo: None,
        title: None,
        details: None,
        repro: None,
        issue: None,
        queue: Queue::default(),
        fixed_in: None,
        announced: false,
    }
}

pub fn write(report: &CrashReport) -> io::Result<PathBuf> {
    let path = path_of(report);
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(ashtray_dir);
    fs::create_dir_all(&dir)?;
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

/// All reports, newest first: the ashtray and the old crashes directory.
pub fn list() -> io::Result<Vec<CrashReport>> {
    let mut out = Vec::new();
    for dir in [ashtray_dir(), crashes_dir()] {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
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
    let name = format!("{}.json", report.id);
    let old = crashes_dir().join(&name);
    if old.is_file() {
        return old;
    }
    ashtray_dir().join(name)
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

/// The markers around the live stats in an issue body, and before the first
/// report's own section, which later reports leave as it is.
const STATS_OPEN: &str = "<!-- cig-stats ";
const STATS_CLOSE: &str = " -->";
const REPORT_MARK: &str = "<!-- cig-report -->";

/// One issue per fingerprint; this block in its body counts the reports.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Stats {
    pub count: u64,
    pub versions: Vec<String>,
    pub oses: Vec<String>,
    pub first: String,
    pub last: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smallest_repro_ops: Option<usize>,
}

/// What a repeated report brought that the issue did not have.
#[derive(Debug, Default)]
pub struct News {
    pub version: Option<String>,
    pub os: Option<String>,
    pub smaller_repro: Option<usize>,
}

impl News {
    pub fn any(&self) -> bool {
        self.version.is_some() || self.os.is_some() || self.smaller_repro.is_some()
    }
}

impl Stats {
    pub fn first(r: &CrashReport) -> Stats {
        Stats {
            count: 1,
            versions: vec![r.version.clone()],
            oses: vec![r.os.clone()],
            first: r.timestamp.clone(),
            last: r.timestamp.clone(),
            smallest_repro_ops: r.repro.as_ref().filter(|x| x.verified).map(|x| x.ops),
        }
    }

    /// Count one more report; say what was new about it.
    pub fn absorb(&mut self, r: &CrashReport) -> News {
        let mut news = News::default();
        self.count += 1;
        if !self.versions.contains(&r.version) {
            self.versions.push(r.version.clone());
            news.version = Some(r.version.clone());
        }
        if !self.oses.contains(&r.os) {
            self.oses.push(r.os.clone());
            news.os = Some(r.os.clone());
        }
        if r.timestamp > self.last {
            self.last = r.timestamp.clone();
        }
        if self.first.is_empty() || r.timestamp < self.first {
            self.first = r.timestamp.clone();
        }
        if let Some(ops) = r.repro.as_ref().filter(|x| x.verified).map(|x| x.ops) {
            if self.smallest_repro_ops.is_none_or(|s| ops < s) {
                self.smallest_repro_ops = Some(ops);
                news.smaller_repro = Some(ops);
            }
        }
        news
    }

    pub fn parse(body: &str) -> Option<Stats> {
        let start = body.find(STATS_OPEN)? + STATS_OPEN.len();
        let end = body[start..].find(STATS_CLOSE)? + start;
        serde_json::from_str(body[start..end].trim()).ok()
    }

    pub fn render(&self) -> String {
        format!(
            "{STATS_OPEN}{}{STATS_CLOSE}\n\n| seen | versions | OSes | first | last | smallest repro |\n|---|---|---|---|---|---|\n| {} | {} | {} | {} | {} | {} |\n",
            serde_json::to_string(self).unwrap_or_default(),
            self.count,
            self.versions.join(", "),
            self.oses.join(", "),
            self.first,
            self.last,
            self.smallest_repro_ops
                .map(|n| format!("{n} op{}", if n == 1 { "" } else { "s" }))
                .unwrap_or_else(|| "none verified".to_string())
        )
    }
}

pub fn issue_title(r: &CrashReport) -> String {
    format!(
        "{} [{}]: {} (cig {}, {})",
        r.kind.word(),
        r.fingerprint,
        truncate(r.headline(), 60),
        r.version,
        r.os
    )
}

/// The labels an issue gets: the kind, and the area the location or the
/// message names.
pub fn labels(r: &CrashReport) -> Vec<String> {
    let mut out = vec![r.kind.word().to_string()];
    let text = format!("{} {}", r.panic_location, r.headline()).to_ascii_lowercase();
    for (word, label) in [
        ("tin", "tin"),
        ("seal", "tin"),
        ("box", "box"),
        ("overlay", "box"),
        ("hop", "hop"),
        ("proc", "hop"),
        ("parcel", "parcel"),
        ("journal", "journal"),
        ("rollback", "journal"),
        ("chain", "journal"),
    ] {
        if text.contains(word) && !out.iter().any(|l| l == label) {
            out.push(label.to_string());
        }
    }
    out
}

fn header(r: &CrashReport) -> String {
    let what = match r.kind {
        Kind::Crash => "Automated crash report",
        Kind::Internal => "Automated report of an internal error",
        Kind::Tripwire => "Automated report: a tripwire found a promise broken",
        Kind::Report => "Run bundle filed with `cig report --send`",
    };
    format!(
        "{what} from `{}`. The person who hit this chose to send it.\n\n- {}: {}\n- os/arch: {}/{}\n- location: `{}`\n- fingerprint: `{}`\n\n",
        r.origin, r.origin, r.version, r.os, r.arch, r.panic_location, r.fingerprint
    )
}

fn report_section(r: &CrashReport) -> String {
    let json = serde_json::to_string_pretty(r).unwrap_or_default();
    let mut s = format!(
        "{REPORT_MARK}\n### What {} found\n\n{}\n\n<details><summary>Full report</summary>\n\n```json\n{}\n```\n</details>\n",
        r.origin,
        r.headline(),
        json
    );
    match &r.repro {
        Some(x) if x.verified => {
            s.push_str(&format!(
                "\n<details><summary>Repro (verified in a sandbox, {} op{})</summary>\n\n{}\n\n```cig\n{}```\n",
                x.ops,
                if x.ops == 1 { "" } else { "s" },
                if x.expect.is_empty() { String::new() } else { format!("Expected: {}", x.expect) },
                x.script
            ));
            if !x.seeds.is_empty() {
                s.push_str("\n| seed | kind | bytes | mode |\n|---|---|---|---|\n");
                for seed in &x.seeds {
                    s.push_str(&format!(
                        "| `{}` | {} | {} | {} |\n",
                        seed.path,
                        seed.kind,
                        seed.bytes,
                        seed.mode.map(|m| format!("{m:o}")).unwrap_or_default()
                    ));
                }
            }
            for (name, text) in &x.stubs {
                s.push_str(&format!("\n`{name}`:\n\n```sh\n{text}```\n"));
            }
            s.push_str("</details>\n");
        }
        Some(x) => {
            s.push_str(&format!(
                "\nRepro: none attached. {}\n",
                if x.note.is_empty() {
                    "It did not reproduce in the sandbox.".to_string()
                } else {
                    x.note.clone()
                }
            ));
        }
        None => {}
    }
    s
}

pub fn issue_body(r: &CrashReport) -> String {
    format!(
        "{}{}\n{}",
        header(r),
        Stats::first(r).render(),
        report_section(r)
    )
}

/// The existing body with its stats block replaced; a body from before the
/// stats block gets one appended.
fn with_stats(existing: &str, stats: &Stats) -> String {
    match (existing.find(STATS_OPEN), existing.find(REPORT_MARK)) {
        (Some(a), Some(b)) if a < b => {
            format!("{}{}\n{}", &existing[..a], stats.render(), &existing[b..])
        }
        _ => format!("{}\n\n{}", existing.trim_end(), stats.render()),
    }
}

pub enum SendOutcome {
    Filed(String),
    /// Added to the issue already open for this fingerprint.
    Commented(String),
    /// No tool could file it; the user gets a URL or a file to attach.
    Manual(String),
}

/// Send a report to `repo` (owner/name): one issue per fingerprint, the
/// stats block of a known one updated instead of a comment, a comment only
/// when the report brings a version, an OS or a smaller repro the issue did
/// not have. Hands back a prefilled link when nothing here can file.
pub fn send(report: &mut CrashReport, repo: &str) -> Result<SendOutcome, String> {
    let Some(tracker) = crate::tracker::pick() else {
        let url = format!(
            "https://github.com/{repo}/issues/new?title={}&body={}",
            urlencode(&issue_title(report)),
            urlencode(&truncate(&issue_body(report), 5000))
        );
        return Ok(SendOutcome::Manual(url));
    };
    file_with(tracker.as_ref(), report, repo)
}

pub fn file_with(
    tracker: &dyn crate::tracker::Tracker,
    report: &mut CrashReport,
    repo: &str,
) -> Result<SendOutcome, String> {
    let outcome = match tracker.find(repo, &report.fingerprint)? {
        Some(existing) => {
            let mut stats = Stats::parse(&existing.body).unwrap_or_default();
            if stats.count == 0 {
                // An issue from before the stats block: count it as the first.
                stats.count = 1;
                stats.first = existing.closed_at.clone().unwrap_or_default();
            }
            let news = stats.absorb(report);
            tracker.edit_body(repo, existing.number, &with_stats(&existing.body, &stats))?;
            if news.any() {
                let mut parts = Vec::new();
                if let Some(v) = &news.version {
                    parts.push(format!("a new version, {} {v}", report.origin));
                }
                if let Some(o) = &news.os {
                    parts.push(format!("a new OS, {o}"));
                }
                if let Some(n) = news.smaller_repro {
                    parts.push(format!(
                        "a smaller repro, {n} op{}",
                        if n == 1 { "" } else { "s" }
                    ));
                }
                let mut text = format!(
                    "Seen again with {} (report `{}`, {}).",
                    parts.join("; "),
                    report.id,
                    report.timestamp
                );
                if let (Some(_), Some(x)) = (news.smaller_repro, &report.repro) {
                    text.push_str(&format!("\n\n```cig\n{}```\n", x.script));
                }
                tracker.comment(repo, existing.number, &text)?;
            }
            report.issue = Some(Issue {
                repo: repo.to_string(),
                number: existing.number,
                url: existing.url.clone(),
            });
            report.sent = Some(existing.url.clone());
            SendOutcome::Commented(existing.url)
        }
        None => {
            let issue = tracker.create(
                repo,
                &issue_title(report),
                &issue_body(report),
                &labels(report),
            )?;
            report.issue = Some(Issue {
                repo: repo.to_string(),
                number: issue.number,
                url: issue.url.clone(),
            });
            report.sent = Some(issue.url.clone());
            SendOutcome::Filed(issue.url)
        }
    };
    let _ = write(report);
    Ok(outcome)
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
    fn the_stats_block_counts_and_says_what_is_new() {
        let mut r = CrashReport::new(
            Kind::Tripwire,
            "tripwire rollback",
            "mode wrong",
            None,
            None,
        );
        let mut stats = Stats::first(&r);
        assert_eq!(stats.count, 1);
        // The same again: counted, nothing new.
        let news = stats.absorb(&r);
        assert_eq!(stats.count, 2);
        assert!(!news.any());
        // Another OS and version: both news.
        r.os = "windows".to_string();
        r.version = "9.9.9".to_string();
        let news = stats.absorb(&r);
        assert_eq!(news.os.as_deref(), Some("windows"));
        assert_eq!(news.version.as_deref(), Some("9.9.9"));
        assert_eq!(stats.oses.len(), 2);
        // A verified repro smaller than any before: news; a bigger one: not.
        r.repro = Some(crate::repro::Repro {
            script: String::new(),
            seeds: Vec::new(),
            stubs: Vec::new(),
            ops: 3,
            verified: true,
            expect: String::new(),
            check: String::new(),
            note: String::new(),
        });
        assert_eq!(stats.absorb(&r).smaller_repro, Some(3));
        r.repro.as_mut().unwrap().ops = 5;
        assert_eq!(stats.absorb(&r).smaller_repro, None);
        // The block survives a round trip through an issue body.
        let body = format!("head\n{}\n{REPORT_MARK}\nrest", stats.render());
        let parsed = Stats::parse(&body).unwrap();
        assert_eq!(parsed.count, 5);
        assert_eq!(parsed.smallest_repro_ops, Some(3));
        let replaced = with_stats(&body, &parsed);
        assert!(replaced.starts_with("head\n") && replaced.ends_with("rest"));
    }

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
