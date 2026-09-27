// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! The `cig` command line.

/// `println!` that never panics when stdout is a closed pipe.
macro_rules! outln {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

mod chains;
mod config;
mod crash;
mod doctor;
mod explain;
mod language;
mod repl;
mod report;
mod run;
mod runs;
mod update;

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// Exit codes, stable for automation.
pub mod exit {
    pub const OK: i32 = 0;
    pub const SCRIPT_ERROR: i32 = 1;
    pub const SYNTAX_ERROR: i32 = 2;
    pub const USAGE: i32 = 3;
}

#[derive(Parser)]
#[command(
    name = "cig",
    version,
    about = "CigScript: nothing real happens unless you burn",
    long_about = None,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Machine-readable output on stdout where a command supports it.
    #[arg(long, global = true)]
    pub json: bool,
    /// Never use colour (also honoured: NO_COLOR).
    #[arg(long, global = true)]
    pub no_color: bool,
    /// Do not run doctor's automatic diagnosis on errors (also: CIG_DOCTOR=0).
    #[arg(long, global = true)]
    pub no_doctor: bool,
    /// Same codes, spans, hints and facts, without the catchphrases (also: CIG_PLAIN=1).
    #[arg(long, global = true)]
    pub plain: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run a script for real: every burn journaled, rolled back if it fails (execute).
    Run(RunArgs),
    /// Typos, sticks, effects: refuse before anything runs (static analysis).
    Check {
        /// Script to check.
        file: PathBuf,
        /// Exit non-zero when there are warnings (for CI).
        #[arg(long)]
        deny_warnings: bool,
    },
    /// Light a chain: each stick lit from the last, timed, stopping at the first failure (task runner).
    Light(LightArgs),
    /// What can I light? The chains a script declares, without running it.
    Chains {
        /// Script to inspect.
        file: PathBuf,
    },
    /// Evaluate a snippet and print its value.
    Eval {
        /// CigScript source, e.g. `[1,2,3].map(pack(x) => x * 2)`.
        code: String,
    },
    /// Interactive session (read, eval, print, loop).
    Repl,
    /// The lab notebook: every run, its status, its burns; or one run with its journal (run history).
    Runs {
        /// A run id (or unique prefix) to show in detail.
        id: Option<String>,
        /// Delete run records older than the newest N.
        #[arg(long, value_name = "N")]
        prune: Option<usize>,
    },
    /// Put the world back the way it was, newest op first (rollback).
    Unburn {
        /// Run id or unique prefix.
        id: String,
        /// Show what would be restored without touching anything.
        #[arg(long)]
        dry_run: bool,
        /// Restore even if this run was rolled back already or a snapshot is missing.
        #[arg(long)]
        force: bool,
    },
    /// Look the install over, or diagnose a past run again; asks before it fixes anything (diagnostics).
    Doctor {
        /// Offer to repair what can be repaired, asking before each change.
        #[arg(long)]
        fix: bool,
        /// A run id (or prefix): diagnose that run's recorded error again, read-only.
        run: Option<String>,
    },
    /// The whole language and library on one screen (reference).
    Language,
    /// The long version of an error code: which kind of no, what to type next (error documentation).
    Explain {
        /// A code such as E502.
        code: Option<String>,
        /// Print the JSON Schema for a diagnostic (generated from the registry).
        #[arg(long)]
        schema: bool,
    },
    /// Show or change the few settings there are, in ~/.cigscript/config.
    Config {
        /// Key to read or set.
        key: Option<String>,
        /// New value.
        value: Option<String>,
        /// Remove the key so the default applies.
        #[arg(long)]
        unset: bool,
    },
    /// Crash reports: list, show, send, delete; nothing leaves without a yes.
    Crash {
        #[command(subcommand)]
        command: Option<crash::CrashCommand>,
    },
    /// Bundle a run for a bug report: plan, journal, diagnostic and doctor's diagnosis, redacted (reproducible report).
    Report {
        /// The run id, or a unique prefix of it.
        id: String,
        /// Write here instead of ~/.cigscript/reports/<id>.json; `-` prints to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Get the newer binary from the repo, verified before it is installed (self-update).
    Update {
        /// Only report whether a newer release exists.
        #[arg(long)]
        check: bool,
        /// Permit installing an older release.
        #[arg(long)]
        allow_downgrade: bool,
        /// Install this release instead of the newest.
        #[arg(long, value_name = "VERSION")]
        to: Option<String>,
    },
}

#[derive(Args)]
pub struct RunArgs {
    /// Script to run.
    pub file: PathBuf,
    /// Simulate every burn and print the plan instead of executing it.
    #[arg(long)]
    pub dry_run: bool,
    /// Leave burns in place if the script fails.
    #[arg(long)]
    pub no_rollback: bool,
    /// Skip the static check before running.
    #[arg(long)]
    pub no_check: bool,
    /// Step budget before "your loop never ends" (E515); 0 disables it (also: CIG_MAX_STEPS).
    #[arg(long, value_name = "N")]
    pub max_steps: Option<u64>,
    /// Arguments passed to the script as `args`.
    #[arg(last = true)]
    pub args: Vec<String>,
}

#[derive(Args)]
pub struct LightArgs {
    /// Script that declares the chain.
    pub file: PathBuf,
    /// Name of the chain to light.
    pub chain: String,
    /// Simulate every burn and print the plan instead of executing it.
    #[arg(long)]
    pub dry_run: bool,
    /// Leave burns in place if the chain fails.
    #[arg(long)]
    pub no_rollback: bool,
    /// Skip the static check before running.
    #[arg(long)]
    pub no_check: bool,
    /// Step budget before "your loop never ends" (E515); 0 disables it (also: CIG_MAX_STEPS).
    #[arg(long, value_name = "N")]
    pub max_steps: Option<u64>,
    /// Arguments passed to the script as `args`.
    #[arg(last = true)]
    pub args: Vec<String>,
}

pub fn main() -> i32 {
    let cli = Cli::parse();
    let color = !cli.no_color && std::env::var_os("NO_COLOR").is_none() && stderr_is_tty();
    let ctx = Ctx {
        json: cli.json,
        color,
        doctor: !cli.no_doctor && cigscript::doctor::enabled_by_env(),
        plain: cli.plain || plain_by_env(),
    };
    match cli.command {
        Command::Run(args) => run::run(&ctx, args, None),
        Command::Light(a) => run::run(
            &ctx,
            RunArgs {
                file: a.file,
                dry_run: a.dry_run,
                no_rollback: a.no_rollback,
                no_check: a.no_check,
                max_steps: a.max_steps,
                args: a.args,
            },
            Some(a.chain),
        ),
        Command::Chains { file } => chains::chains(&ctx, &file),
        Command::Check {
            file,
            deny_warnings,
        } => run::check(&ctx, &file, deny_warnings),
        Command::Eval { code } => run::eval(&ctx, &code),
        Command::Repl => repl::repl(&ctx),
        Command::Runs { id, prune } => runs::runs(&ctx, id, prune),
        Command::Unburn { id, dry_run, force } => runs::unburn(&ctx, &id, dry_run, force),
        Command::Doctor { fix, run } => match run {
            Some(id) => doctor::rediagnose(&ctx, &id),
            None => doctor::doctor(&ctx, fix),
        },
        Command::Language => language::language(&ctx),
        Command::Explain { code, schema } => explain::explain(&ctx, code, schema),
        Command::Config { key, value, unset } => config::config(&ctx, key, value, unset),
        Command::Crash { command } => crash::crash(&ctx, command),
        Command::Report { id, out } => report::report(&ctx, &id, out),
        Command::Update {
            check,
            allow_downgrade,
            to,
        } => update::update(&ctx, check, allow_downgrade, to),
    }
}

/// Runs after the interpreter thread panicked.
pub fn after_crash() {
    let ctx = Ctx {
        json: false,
        color: std::env::var_os("NO_COLOR").is_none() && stderr_is_tty(),
        doctor: false,
        plain: plain_by_env(),
    };
    crash::after_crash(&ctx);
}

/// `CIG_PLAIN=1` (or anything but 0/false/off) selects plain mode.
pub fn plain_by_env() -> bool {
    match std::env::var("CIG_PLAIN") {
        Ok(v) => !matches!(v.as_str(), "" | "0" | "false" | "off"),
        Err(_) => false,
    }
}

#[derive(Clone, Copy)]
pub struct Ctx {
    pub json: bool,
    pub color: bool,
    /// Run doctor's automatic diagnosis when a diagnostic is reported.
    pub doctor: bool,
    /// Plain mode: the same information without the catchphrases.
    pub plain: bool,
}

impl Ctx {
    pub fn red(&self, s: &str) -> String {
        self.paint("31", s)
    }
    pub fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.paint("33", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    pub fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
}

fn stderr_is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal()
}
