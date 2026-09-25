// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! The kernel: gates side effects, journals them, rolls them back.
//!
//! Every effectful builtin describes what it is about to do as an [`Op`]
//! and asks the [`Kernel`] for a [`Decision`] before touching the world.
//! In `run` mode the kernel snapshots whatever the op will change and
//! appends a journal entry; in dry-run mode (and inside `burn unlit`) it
//! records the intent and tells the builtin to simulate.

pub mod journal;
pub mod runs;

use crate::diagnostics::{burn as burn_error, Diagnostic};
use journal::Journal;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A side effect a builtin wants to perform.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    Write {
        path: PathBuf,
    },
    Append {
        path: PathBuf,
    },
    Delete {
        path: PathBuf,
    },
    Move {
        from: PathBuf,
        to: PathBuf,
    },
    Copy {
        from: PathBuf,
        to: PathBuf,
    },
    Mkdir {
        path: PathBuf,
    },
    Proc {
        cmd: String,
        args: Vec<String>,
        cwd: Option<PathBuf>,
    },
    EnvSet {
        key: String,
    },
    EnvUnset {
        key: String,
    },
}

impl Op {
    pub fn label(&self) -> &'static str {
        match self {
            Op::Write { .. } => "write",
            Op::Append { .. } => "append",
            Op::Delete { .. } => "delete",
            Op::Move { .. } => "move",
            Op::Copy { .. } => "copy",
            Op::Mkdir { .. } => "mkdir",
            Op::Proc { .. } => "proc",
            Op::EnvSet { .. } => "env_set",
            Op::EnvUnset { .. } => "env_unset",
        }
    }

    /// Whether the kernel can undo this on its own. Environment changes
    /// die with the process, so they count as reversible; a spawned
    /// process does not.
    pub fn reversible(&self) -> bool {
        !matches!(self, Op::Proc { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Op::Write { path } => format!("write {}", path.display()),
            Op::Append { path } => format!("append to {}", path.display()),
            Op::Delete { path } => format!("delete {}", path.display()),
            Op::Move { from, to } => format!("move {} -> {}", from.display(), to.display()),
            Op::Copy { from, to } => format!("copy {} -> {}", from.display(), to.display()),
            Op::Mkdir { path } => format!("mkdir {}", path.display()),
            Op::Proc { cmd, args, cwd } => {
                let mut s = format!("run {}", shell_quote(cmd));
                for a in args {
                    s.push(' ');
                    s.push_str(&shell_quote(a));
                }
                if let Some(c) = cwd {
                    s.push_str(&format!("  (in {})", c.display()));
                }
                s
            }
            Op::EnvSet { key } => format!("set env {key}"),
            Op::EnvUnset { key } => format!("unset env {key}"),
        }
    }
}

fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Which block class an op was requested under.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BurnClass {
    Burn,
    Unlit,
}

/// An op that was recorded but not executed (dry-run or unlit).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlannedOp {
    pub seq: u64,
    pub class: BurnClass,
    pub reversible: bool,
    pub summary: String,
    pub op: Op,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Perform the effect; the kernel has journaled it.
    Execute,
    /// Do not touch the world; return a plausible result.
    Simulate,
}

/// Run-wide effect policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Execute burns, journal them.
    Run,
    /// Simulate every burn, produce a plan.
    DryRun,
}

pub struct Kernel {
    pub mode: Mode,
    journal: Journal,
    pub planned: Vec<PlannedOp>,
    seq: u64,
    pub executed: usize,
    pub irreversible: usize,
}

impl Kernel {
    /// A kernel that journals into `run_dir`, or in memory only when `None`.
    pub fn new(mode: Mode, run_dir: Option<PathBuf>) -> Result<Self, Diagnostic> {
        Ok(Self {
            mode,
            journal: Journal::open(run_dir)
                .map_err(|e| burn_error(format!("could not open the run journal: {e}")))?,
            planned: Vec::new(),
            seq: 0,
            executed: 0,
            irreversible: 0,
        })
    }

    pub fn ephemeral(mode: Mode) -> Self {
        Self::new(mode, None).expect("in-memory journal cannot fail")
    }

    pub fn run_dir(&self) -> Option<&Path> {
        self.journal.run_dir()
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    /// Ask permission to perform `op`. `unlit` is true inside `burn unlit`.
    pub fn decide(&mut self, op: Op, unlit: bool) -> Result<Decision, Diagnostic> {
        self.seq += 1;
        let simulate = unlit || self.mode == Mode::DryRun;
        if simulate {
            self.planned.push(PlannedOp {
                seq: self.seq,
                class: if unlit {
                    BurnClass::Unlit
                } else {
                    BurnClass::Burn
                },
                reversible: op.reversible(),
                summary: op.describe(),
                op,
            });
            return Ok(Decision::Simulate);
        }
        if !op.reversible() {
            self.irreversible += 1;
        }
        self.executed += 1;
        self.journal.record(self.seq, op).map_err(|e| {
            burn_error(format!("could not journal the burn: {e}"))
                .code("E702")
                .with_hint("nothing was changed; the effect is refused when it cannot be journaled")
        })?;
        Ok(Decision::Execute)
    }

    /// Undo every journaled op, newest first.
    pub fn rollback(&mut self) -> journal::RollbackReport {
        self.journal.rollback()
    }

    pub fn intents(&self) -> impl Iterator<Item = &PlannedOp> {
        self.planned.iter().filter(|p| p.class == BurnClass::Unlit)
    }

    pub fn dry_run_plan(&self) -> impl Iterator<Item = &PlannedOp> {
        self.planned.iter().filter(|p| p.class == BurnClass::Burn)
    }
}
