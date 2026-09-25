// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig crash`: local crash reports, and the prompt that follows a crash.

use super::{exit, Ctx};
use cigscript::config::Config;
use cigscript::crash::{self, CrashReport, SendOutcome};
use std::io::{self, BufRead, IsTerminal, Write};

#[derive(clap::Subcommand)]
pub enum CrashCommand {
    /// List saved crash reports.
    List,
    /// Print a report exactly as it would be sent.
    Show {
        /// Report id or unique prefix.
        id: String,
    },
    /// File a report on the issue tracker (asks nothing further).
    Send {
        /// Report id or unique prefix.
        id: String,
    },
    /// Delete a report, or all of them.
    Delete {
        /// Report id or unique prefix.
        id: Option<String>,
        /// Delete every saved report.
        #[arg(long)]
        all: bool,
    },
}

pub fn crash(ctx: &Ctx, cmd: Option<CrashCommand>) -> i32 {
    match cmd.unwrap_or(CrashCommand::List) {
        CrashCommand::List => list(ctx),
        CrashCommand::Show { id } => match find(ctx, &id) {
            Some(r) => {
                if ctx.json {
                    outln!("{}", serde_json::to_string(&r).unwrap_or_default());
                } else {
                    outln!("{}", ctx.bold(&format!("crash report {}", r.id)));
                    outln!("{}", ctx.dim("this is exactly what `cig crash send` would file, inside a JSON block:"));
                    outln!();
                    outln!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
                }
                exit::OK
            }
            None => exit::USAGE,
        },
        CrashCommand::Send { id } => match find(ctx, &id) {
            Some(mut r) => send(ctx, &mut r),
            None => exit::USAGE,
        },
        CrashCommand::Delete { id, all } => {
            if all {
                let n = crash::list().unwrap_or_default().len();
                let _ = std::fs::remove_dir_all(crash::crashes_dir());
                eprintln!("deleted {n} crash report{}", if n == 1 { "" } else { "s" });
                return exit::OK;
            }
            let Some(id) = id else {
                eprintln!(
                    "{} give a report id, or --all",
                    ctx.red("error[E800 usage]:")
                );
                return exit::USAGE;
            };
            match find(ctx, &id) {
                Some(r) => {
                    let _ = std::fs::remove_file(crash::path_of(&r));
                    eprintln!("deleted {}", r.id);
                    exit::OK
                }
                None => exit::USAGE,
            }
        }
    }
}

fn find(ctx: &Ctx, id: &str) -> Option<CrashReport> {
    match crash::find(id) {
        Ok(Some(r)) => Some(r),
        Ok(None) => {
            eprintln!(
                "{} no crash report matches `{id}`",
                ctx.red("error[E800 usage]:")
            );
            None
        }
        Err(e) => {
            eprintln!("{} {e}", ctx.red("error[E800 usage]:"));
            None
        }
    }
}

fn list(ctx: &Ctx) -> i32 {
    let all = crash::list().unwrap_or_default();
    if ctx.json {
        outln!("{}", serde_json::to_string(&all).unwrap_or_default());
        return exit::OK;
    }
    if all.is_empty() {
        eprintln!("no crash reports saved (that is good)");
        return exit::OK;
    }
    outln!(
        "{:<24} {:<10} {:<8} {}",
        "report",
        "version",
        "sent",
        "where"
    );
    for r in &all {
        outln!(
            "{:<24} {:<10} {:<8} {}",
            r.id,
            r.version,
            if r.sent.is_some() { "yes" } else { "no" },
            r.panic_location
        );
    }
    outln!(
        "{}",
        ctx.dim("cig crash show <id> to read one, cig crash send <id> to file it")
    );
    exit::OK
}

fn send(ctx: &Ctx, r: &mut CrashReport) -> i32 {
    let repo = Config::load().get("issues_repo");
    match crash::send(r, &repo) {
        Ok(SendOutcome::Filed(url)) => {
            eprintln!("{} filed at {url}", ctx.green("sent:"));
            exit::OK
        }
        Ok(SendOutcome::Commented(url)) => {
            eprintln!(
                "{} this crash is already known; noted on {url}",
                ctx.green("sent:")
            );
            exit::OK
        }
        Ok(SendOutcome::Manual(url)) => {
            eprintln!(
                "{} no `gh` and no CIG_GITHUB_TOKEN, so nothing was sent automatically.",
                ctx.yellow("note:")
            );
            eprintln!("open this link to file it yourself (the report is prefilled):");
            eprintln!("  {url}");
            eprintln!(
                "and attach {} if the form truncated it.",
                crash::path_of(r).display()
            );
            exit::OK
        }
        Err(e) => {
            eprintln!("{} {e}", ctx.red("error[E805 usage]:"));
            eprintln!("the report stays at {}", crash::path_of(r).display());
            exit::USAGE
        }
    }
}

/// Runs in `main` after the interpreter thread panicked.
pub fn after_crash(ctx: &Ctx) {
    let Some(path) = crash::last_report() else {
        return;
    };
    let Ok(mut report) = crash::load(&path) else {
        return;
    };
    let cfg = Config::load();
    let repo = cfg.get("issues_repo");
    match cfg.get("crash_reports").as_str() {
        "never" => {
            eprintln!(
                "crash_reports is \"never\": the report stays local at {}. `cig config crash_reports ask` changes this.",
                path.display()
            );
        }
        "always" => {
            eprintln!(
                "crash_reports is \"always\": sending the report below to github.com/{repo}."
            );
            eprintln!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
            send(ctx, &mut report);
        }
        _ => {
            if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
                eprintln!(
                    "To file it: cig crash show {}   then   cig crash send {}",
                    report.id, report.id
                );
                return;
            }
            eprintln!();
            eprintln!("cig hit a bug and had to stop. A crash report was saved locally:");
            eprintln!("  {}", path.display());
            eprintln!(
                "View the full contents first:  cig crash show {}",
                report.id
            );
            eprintln!();
            eprintln!("It contains: cig version, OS/arch, the command line (token- and path-looking values");
            eprintln!(
                "redacted), the working directory relative to your home, a timestamp, the panic"
            );
            eprintln!(
                "message and where it happened in cig's own code, a backtrace of cig's own frames,"
            );
            eprintln!(
                "and the id of the run in progress, if any. It does NOT contain your script's"
            );
            eprintln!("source, environment variable values, or file contents.");
            eprintln!();
            eprint!("Send this report to github.com/{repo} as a public, permanent issue? [y/N] ");
            let _ = io::stderr().flush();
            let mut line = String::new();
            let _ = io::stdin().lock().read_line(&mut line);
            if matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                send(ctx, &mut report);
            } else {
                eprintln!("not sent. Later: cig crash send {}", report.id);
            }
        }
    }
}
