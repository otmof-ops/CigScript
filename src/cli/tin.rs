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
    /// Re-hash every object against its name, check every recipe, open every sealed object.
    Verify {
        /// Only look at each sealed object's shape instead of opening it.
        #[arg(long)]
        quick: bool,
    },
    /// Remove every object no run in the history still refers to.
    Gc {
        /// Also remove sealed objects the tin's key cannot open (sealed under a key that is gone).
        #[arg(long)]
        sealed_orphans: bool,
    },
}

pub fn tin_cmd(ctx: &Ctx, cmd: Option<TinCommand>) -> i32 {
    match cmd.unwrap_or(TinCommand::Du) {
        TinCommand::Du => {
            let u = tin::du();
            let referenced = tin::referenced().len();
            if ctx.json {
                outln!(
                    "{}",
                    serde_json::json!({"objects": u.objects, "bytes": u.stored, "stored": u.stored, "recipes": u.recipes, "logical": u.logical, "sealed": u.sealed, "sealed_bytes": u.sealed_bytes, "referenced": referenced, "dir": tin::dir()})
                );
            } else {
                outln!(
                    "{} object{} ({} bytes stored) keeping {} bytes of files{}{}; {referenced} still referred to by a run, in {}",
                    u.objects,
                    if u.objects == 1 { "" } else { "s" },
                    u.stored,
                    u.logical,
                    if u.recipes == 0 {
                        String::new()
                    } else {
                        format!(", {} of them chunked under a recipe", u.recipes)
                    },
                    if u.sealed == 0 {
                        String::new()
                    } else {
                        format!(", {} sealed ({} bytes)", u.sealed, u.sealed_bytes)
                    },
                    cigscript::crash::tilde(&tin::dir())
                );
            }
            exit::OK
        }
        TinCommand::Verify { quick } => {
            let problems = tin::verify(quick);
            let objects = tin::du().objects;
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
        TinCommand::Gc { sealed_orphans } => {
            let (removed, bytes) = tin::gc(sealed_orphans);
            let kept = tin::du().objects;
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
