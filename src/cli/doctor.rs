// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig doctor`: is the installation healthy, and what is missing?
//! `--fix` offers to repair what it can, asking before each change.

use super::{exit, Ctx};
use cigscript::burn::runs;
use cigscript::config::Config;
use cigscript::{crash, update};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

struct Tool {
    name: &'static str,
    enables: &'static str,
    present: bool,
}

pub fn doctor(ctx: &Ctx, fix: bool) -> i32 {
    let home = runs::home_dir();
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cig"));
    let exe_dir = exe.parent().map(Path::to_path_buf);
    let on_path = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .any(|d| d.join("cig").is_file() || d.join("cig.exe").is_file())
        })
        .unwrap_or(false);
    let writable = check_writable(&home);
    let run_count = runs::list().map(|l| l.len()).unwrap_or(0);
    let run_bytes = dir_size(&runs::runs_dir());
    let crashes = crash::list().unwrap_or_default();
    let unsent = crashes.iter().filter(|c| c.sent.is_none()).count();
    let tools = vec![
        Tool {
            name: "curl",
            enables: "cig update, crash reports without gh",
            present: update::has_tool("curl"),
        },
        Tool {
            name: "gh",
            enables: "crash reports and updates from private repositories",
            present: update::has_tool("gh"),
        },
        Tool {
            name: "git",
            enables: "the examples that shell out to git",
            present: update::has_tool("git"),
        },
    ];
    let cfg = Config::load();
    let update_cache = update::read_cache();
    let newer = update::cached_newer();

    let mut problems: Vec<String> = Vec::new();
    if let Err(e) = &writable {
        problems.push(format!(
            "state directory {} is not writable: {e}",
            home.display()
        ));
    }
    if !on_path {
        problems
            .push("`cig` is not on PATH, so `#!/usr/bin/env cig` scripts cannot run".to_string());
    }
    if !tools[0].present && !tools[1].present {
        problems.push("neither curl nor gh is installed: `cig update` and crash reporting cannot reach GitHub".to_string());
    }
    if let Some(n) = &newer {
        problems.push(format!(
            "a newer release is available: {} (you run {}); run `cig update`",
            n.latest, n.current
        ));
    }
    if unsent > 0 {
        problems.push(format!(
            "{unsent} crash report{} not sent yet; `cig crash list`",
            if unsent == 1 { "" } else { "s" }
        ));
    }
    if run_bytes > 1 << 30 {
        problems.push(format!(
            "run records use {} MB; `cig runs --prune 50` frees space",
            run_bytes >> 20
        ));
    }

    if ctx.json {
        let obj = serde_json::json!({
            "version": cigscript::VERSION,
            "executable": exe,
            "on_path": on_path,
            "state_dir": home,
            "state_dir_writable": writable.is_ok(),
            "runs": run_count,
            "runs_bytes": run_bytes,
            "crash_reports": crashes.len(),
            "crash_reports_unsent": unsent,
            "tools": tools.iter().map(|t| serde_json::json!({"name": t.name, "present": t.present})).collect::<Vec<_>>(),
            "config": {"crash_reports": cfg.get("crash_reports"), "issues_repo": cfg.get("issues_repo"), "update_channel": cfg.get("update_channel")},
            "update": update_cache,
            "problems": problems,
        });
        outln!("{}", serde_json::to_string_pretty(&obj).unwrap_or_default());
        return if problems.is_empty() {
            exit::OK
        } else {
            exit::SCRIPT_ERROR
        };
    }

    outln!("cig {}", cigscript::VERSION);
    outln!("  executable    {}", exe.display());
    outln!("  on PATH       {}", yes_no(ctx, on_path));
    outln!(
        "  state dir     {}  {}",
        home.display(),
        ctx.dim(if std::env::var_os("CIGSCRIPT_HOME").is_some() {
            "(from CIGSCRIPT_HOME)"
        } else {
            "(default)"
        })
    );
    outln!("  writable      {}", yes_no(ctx, writable.is_ok()));
    outln!(
        "  runs          {run_count} recorded, {} KB",
        run_bytes >> 10
    );
    outln!("  crash reports {} saved, {unsent} unsent", crashes.len());
    outln!(
        "  config        crash_reports={}  issues_repo={}  update_channel={}",
        cfg.get("crash_reports"),
        cfg.get("issues_repo"),
        cfg.get("update_channel")
    );
    match &update_cache {
        Some(c) => outln!(
            "  update        latest known {} (checked {}){}",
            c.latest,
            c.checked_at,
            if newer.is_some() {
                ctx.yellow("  newer than this build")
            } else {
                String::new()
            }
        ),
        None => outln!(
            "  update        {}",
            ctx.dim("not checked yet: cig update --check")
        ),
    }
    outln!("  tools");
    for t in &tools {
        outln!(
            "    {:<6} {}  {}",
            t.name,
            yes_no(ctx, t.present),
            ctx.dim(t.enables)
        );
    }
    if problems.is_empty() {
        outln!("{}", ctx.green("no problems found"));
        return exit::OK;
    }
    for p in &problems {
        outln!("{} {p}", ctx.yellow("problem:"));
    }
    if !fix {
        outln!(
            "{}",
            ctx.dim("cig doctor --fix offers to repair what it can")
        );
        return exit::SCRIPT_ERROR;
    }

    // ----- --fix: every change is proposed and needs a y -----------------------
    if !io::stdin().is_terminal() {
        eprintln!("--fix needs a terminal to ask questions");
        return exit::USAGE;
    }
    let mut fixed = 0;
    if !on_path {
        if let Some(dir) = &exe_dir {
            if let Some((rc, line)) = path_line(dir) {
                if ask(&format!("append `{line}` to {}?", rc.display())) {
                    match append_line(&rc, &line) {
                        Ok(()) => {
                            eprintln!(
                                "{} added; open a new shell to pick it up",
                                ctx.green("done:")
                            );
                            fixed += 1;
                        }
                        Err(e) => eprintln!("{} {e}", ctx.red("failed:")),
                    }
                }
            }
        }
    }
    for t in tools.iter().filter(|t| !t.present) {
        if let Some(cmd) = install_command(t.name) {
            let shown = cmd.join(" ");
            eprintln!(
                "{} {} is missing; it enables {}",
                ctx.yellow("missing:"),
                t.name,
                t.enables
            );
            if ask(&format!("run `{shown}` now?")) {
                let status = std::process::Command::new(&cmd[0]).args(&cmd[1..]).status();
                match status {
                    Ok(s) if s.success() => {
                        eprintln!("{} {} installed", ctx.green("done:"), t.name);
                        fixed += 1;
                    }
                    Ok(s) => eprintln!("{} exited with {s}", ctx.red("failed:")),
                    Err(e) => eprintln!("{} could not run it: {e}", ctx.red("failed:")),
                }
            }
        } else {
            eprintln!(
                "{} {} is missing and no package manager was recognised; install it by hand",
                ctx.yellow("missing:"),
                t.name
            );
        }
    }
    if newer.is_some() && ask("run `cig update` now?") {
        let code = super::update::update(ctx, false, false, None);
        if code == exit::OK {
            fixed += 1;
        }
    }
    eprintln!("{fixed} fix{} applied", if fixed == 1 { "" } else { "es" });
    exit::OK
}

fn ask(question: &str) -> bool {
    eprint!("{question} [y/N] ");
    let _ = io::stderr().flush();
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The shell rc file and the line that would put `dir` on PATH.
fn path_line(dir: &Path) -> Option<(PathBuf, String)> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let shell = std::env::var("SHELL").unwrap_or_default();
    let shown = crash::tilde(dir).replace('~', "$HOME");
    if shell.ends_with("fish") {
        return Some((
            home.join(".config/fish/config.fish"),
            format!("fish_add_path {shown}"),
        ));
    }
    let rc = if shell.ends_with("zsh") {
        ".zshrc"
    } else {
        ".bashrc"
    };
    Some((home.join(rc), format!("export PATH=\"{shown}:$PATH\"")))
}

fn append_line(rc: &Path, line: &str) -> io::Result<()> {
    if let Ok(existing) = std::fs::read_to_string(rc) {
        if existing.lines().any(|l| l.trim() == line) {
            return Ok(());
        }
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(rc)?;
    writeln!(f, "\n# added by cig doctor --fix\n{line}")
}

/// The exact package-manager command for a tool, or none if unrecognised.
/// Shown in full and run only after a `y`; sudo is part of the command the
/// user sees, never something added silently.
fn install_command(tool: &str) -> Option<Vec<String>> {
    let has = |m: &str| update::has_tool(m);
    let pkg = match tool {
        "gh" => "gh",
        "curl" => "curl",
        "git" => "git",
        _ => return None,
    };
    let cmd: Vec<&str> = if has("apt-get") {
        vec!["sudo", "apt-get", "install", "-y", pkg]
    } else if has("dnf") {
        vec!["sudo", "dnf", "install", "-y", pkg]
    } else if has("pacman") {
        vec![
            "sudo",
            "pacman",
            "-S",
            "--noconfirm",
            if pkg == "gh" { "github-cli" } else { pkg },
        ]
    } else if has("zypper") {
        vec!["sudo", "zypper", "install", "-y", pkg]
    } else if has("apk") {
        vec![
            "sudo",
            "apk",
            "add",
            if pkg == "gh" { "github-cli" } else { pkg },
        ]
    } else if has("brew") {
        vec!["brew", "install", pkg]
    } else if has("winget") {
        vec![
            "winget",
            "install",
            if pkg == "gh" {
                "GitHub.cli"
            } else if pkg == "git" {
                "Git.Git"
            } else {
                "cURL.cURL"
            },
        ]
    } else {
        return None;
    };
    Some(cmd.into_iter().map(String::from).collect())
}

fn yes_no(ctx: &Ctx, v: bool) -> String {
    if v {
        ctx.green("yes")
    } else {
        ctx.yellow("no")
    }
}

fn check_writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(".doctor-probe");
    std::fs::write(&probe, b"ok")?;
    std::fs::remove_file(&probe)
}

fn dir_size(dir: &Path) -> u64 {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}
