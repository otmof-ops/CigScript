// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig tin`: what the tin holds, whether it is intact, and clearing what
//! no run still needs.

use super::{exit, Ctx};
use cigscript::burn::tin;

#[derive(clap::Subcommand)]
pub enum TinCommand {
    /// How many objects the tin holds and how many bytes (the default).
    Du,
    /// Re-hash every object against its name.
    Verify,
    /// Remove every object no run in the history still refers to.
    Gc,
}

pub fn tin_cmd(ctx: &Ctx, cmd: Option<TinCommand>) -> i32 {
    match cmd.unwrap_or(TinCommand::Du) {
        TinCommand::Du => {
            let (objects, bytes) = tin::du();
            let referenced = tin::referenced().len();
            if ctx.json {
                outln!(
                    "{}",
                    serde_json::json!({"objects": objects, "bytes": bytes, "referenced": referenced, "dir": tin::dir()})
                );
            } else {
                outln!(
                    "{objects} object{} ({bytes} bytes), {referenced} still referred to by a run, in {}",
                    if objects == 1 { "" } else { "s" },
                    cigscript::crash::tilde(&tin::dir())
                );
            }
            exit::OK
        }
        TinCommand::Verify => {
            let problems = tin::verify();
            let (objects, _) = tin::du();
            if ctx.json {
                outln!(
                    "{}",
                    serde_json::json!({"objects": objects, "problems": problems})
                );
            } else if problems.is_empty() {
                eprintln!(
                    "{} {objects} object{} hash to their names",
                    ctx.green("ok:"),
                    if objects == 1 { "" } else { "s" }
                );
            } else {
                eprintln!(
                    "{} {} problem{} in the tin:",
                    ctx.red("error[E703 burn]:"),
                    problems.len(),
                    if problems.len() == 1 { "" } else { "s" }
                );
                for p in &problems {
                    eprintln!("  - {p}");
                }
                eprintln!("  = hint: a restore from a corrupt object is refused; cig tin gc removes objects no run needs");
                eprintln!("  = explain: cig explain E703");
            }
            if problems.is_empty() {
                exit::OK
            } else {
                exit::SCRIPT_ERROR
            }
        }
        TinCommand::Gc => {
            let (removed, bytes) = tin::gc();
            let (kept, _) = tin::du();
            if ctx.json {
                outln!(
                    "{}",
                    serde_json::json!({"removed": removed, "bytes": bytes, "kept": kept})
                );
            } else {
                eprintln!(
                    "removed {removed} object{} ({bytes} bytes); {kept} kept, each referred to by a run (cig runs --prune N lets more go)",
                    if removed == 1 { "" } else { "s" }
                );
            }
            exit::OK
        }
    }
}
