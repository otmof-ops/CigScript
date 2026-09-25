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
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run a script (burns are journaled and rolled back on failure).
    Run(RunArgs),
    /// Parse and statically check a script without running it.
    Check {
        /// Script to check.
        file: PathBuf,
    },
    /// Light a chain: run the script, then its named chain, step by step.
    Light(LightArgs),
    /// List the chains a script declares, without running it.
    Chains {
        /// Script to inspect.
        file: PathBuf,
    },
    /// Evaluate a snippet and print its value.
    Eval {
        /// CigScript source, e.g. `[1,2,3].map(pack(x) => x * 2)`.
        code: String,
    },
    /// Interactive session.
    Repl,
    /// List past runs, or show one.
    Runs {
        /// A run id (or unique prefix) to show in detail.
        id: Option<String>,
        /// Delete run records older than the newest N.
        #[arg(long, value_name = "N")]
        prune: Option<usize>,
    },
    /// Roll back the burns of a past run.
    Unburn {
        /// Run id or unique prefix.
        id: String,
        /// Show what would be restored without touching anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Report on the installation, tools, updates and crash reports.
    Doctor {
        /// Offer to repair what can be repaired, asking before each change.
        #[arg(long)]
        fix: bool,
    },
    /// Print the language legend and the standard library surface.
    Language,
    /// What an error code means and how to fix it (all codes with no argument).
    Explain {
        /// A code such as E502.
        code: Option<String>,
    },
    /// Show or change settings in ~/.cigscript/config.
    Config {
        /// Key to read or set.
        key: Option<String>,
        /// New value.
        value: Option<String>,
        /// Remove the key so the default applies.
        #[arg(long)]
        unset: bool,
    },
    /// Crash reports: list, show, send, delete.
    Crash {
        #[command(subcommand)]
        command: Option<crash::CrashCommand>,
    },
    /// Check for a newer release and install it.
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
                args: a.args,
            },
            Some(a.chain),
        ),
        Command::Chains { file } => chains::chains(&ctx, &file),
        Command::Check { file } => run::check(&ctx, &file),
        Command::Eval { code } => run::eval(&ctx, &code),
        Command::Repl => repl::repl(&ctx),
        Command::Runs { id, prune } => runs::runs(&ctx, id, prune),
        Command::Unburn { id, dry_run } => runs::unburn(&ctx, &id, dry_run),
        Command::Doctor { fix } => doctor::doctor(&ctx, fix),
        Command::Language => language::language(&ctx),
        Command::Explain { code } => explain::explain(&ctx, code),
        Command::Config { key, value, unset } => config::config(&ctx, key, value, unset),
        Command::Crash { command } => crash::crash(&ctx, command),
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
    };
    crash::after_crash(&ctx);
}

#[derive(Clone, Copy)]
pub struct Ctx {
    pub json: bool,
    pub color: bool,
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
