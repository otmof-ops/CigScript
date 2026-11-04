// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig tin`: what the tin holds, whether it is intact, and clearing what
//! no run still needs.

use super::{exit, Ctx};
use cigscript::burn::{seal, tin};
use std::path::PathBuf;

#[derive(clap::Subcommand)]
pub enum TinCommand {
    /// How many objects the tin holds and how many bytes (the default).
    Du,
    /// Re-hash every object against its name, check every recipe, open every sealed object.
    Verify {
        /// Only look at each sealed object's shape instead of opening it.
        #[arg(long)]
        quick: bool,
        /// Also check the ledger's head against a value `cig tin head` printed earlier (kept elsewhere).
        #[arg(long, value_name = "HEAD")]
        head: Option<String>,
    },
    /// The ledger's head: one line to keep anywhere else, for `cig tin verify --head`.
    Head,
    /// Remove every object no run in the history still refers to.
    Gc {
        /// Also remove sealed objects the tin's key cannot open (sealed under a key that is gone).
        #[arg(long)]
        sealed_orphans: bool,
    },
    /// Back the tin's key up, or put a backed-up key in place.
    Key {
        #[command(subcommand)]
        cmd: KeyCommand,
    },
}

#[derive(clap::Subcommand)]
pub enum KeyCommand {
    /// Write the key, wrapped with a passphrase, so a backup can carry it.
    Export {
        /// Where to write it (default: cig-tin-key.export).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Put a passphrase-wrapped key back as the fallback key file.
    Import {
        /// The file a previous `cig tin key export` wrote.
        file: PathBuf,
    },
}

/// The passphrase: `CIG_KEY_PASSPHRASE` when set (CI, scripts), else typed.
fn passphrase() -> Result<String, String> {
    if let Ok(p) = std::env::var("CIG_KEY_PASSPHRASE") {
        return Ok(p);
    }
    use std::io::Write;
    eprint!("passphrase (CIG_KEY_PASSPHRASE avoids the echo): ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let p = line.trim_end_matches(['\n', '\r']).to_string();
    if p.is_empty() {
        return Err("an empty passphrase protects nothing".to_string());
    }
    Ok(p)
}

fn key_cmd(ctx: &Ctx, cmd: KeyCommand) -> i32 {
    match cmd {
        KeyCommand::Export { out } => {
            let dest = out.unwrap_or_else(|| PathBuf::from("cig-tin-key.export"));
            let pass = match passphrase() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("{} {e}", ctx.red("error[E801 usage]:"));
                    return exit::USAGE;
                }
            };
            match seal::export_key(&dest, &pass) {
                Ok(()) => {
                    seal::mark_exported();
                    eprintln!(
                        "{} the tin's key is wrapped in {} (keep it somewhere safe; it opens your sealed objects on a restored backup)",
                        ctx.green("ok:"),
                        dest.display()
                    );
                    exit::OK
                }
                Err(e) => {
                    eprintln!("{} {e}", ctx.red("error[E703 burn]:"));
                    exit::SCRIPT_ERROR
                }
            }
        }
        KeyCommand::Import { file } => {
            let pass = match passphrase() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("{} {e}", ctx.red("error[E801 usage]:"));
                    return exit::USAGE;
                }
            };
            match seal::import_key(&file, &pass) {
                Ok(()) => {
                    eprintln!(
                        "{} the key from {} is in place; your sealed objects open again",
                        ctx.green("ok:"),
                        file.display()
                    );
                    exit::OK
                }
                Err(e) => {
                    eprintln!("{} {e}", ctx.red("error[E703 burn]:"));
                    exit::SCRIPT_ERROR
                }
            }
        }
    }
}

pub fn tin_cmd(ctx: &Ctx, cmd: Option<TinCommand>) -> i32 {
    match cmd.unwrap_or(TinCommand::Du) {
        TinCommand::Key { cmd } => key_cmd(ctx, cmd),
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
        TinCommand::Head => match cigscript::burn::ledger::load() {
            Ok(l) => {
                let seq = l.lines.last().map(|x| x.seq).unwrap_or(0);
                if ctx.json {
                    outln!(
                        "{}",
                        serde_json::json!({"seq": seq, "head": l.head, "broken_at": l.broken_at})
                    );
                } else {
                    outln!("{seq} {}", l.head);
                }
                if l.broken_at.is_some() {
                    exit::SCRIPT_ERROR
                } else {
                    exit::OK
                }
            }
            Err(e) => {
                eprintln!("{} {e}", ctx.red("error[E703 burn]:"));
                exit::SCRIPT_ERROR
            }
        },
        TinCommand::Verify { quick, head } => {
            let mut problems = tin::verify(quick);
            match cigscript::burn::ledger::load() {
                Ok(l) => {
                    if let Some(at) = l.broken_at {
                        problems.push(format!(
                            "the ledger's line {at} does not carry cig's signature (E709): it was edited, removed or reordered"
                        ));
                    }
                    if let Some(want) = head.as_deref() {
                        let want = want.split_whitespace().last().unwrap_or(want);
                        if want != l.head {
                            problems.push(format!(
                                "the ledger's head is {} but the value kept elsewhere says {want}: the ledger was cut short or swapped",
                                l.head
                            ));
                        }
                    }
                }
                Err(e) => {
                    if head.is_some() {
                        problems.push(format!("the ledger could not be read: {e}"));
                    }
                }
            }
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
            if sealed_orphans {
                // Without the key nothing opens, and "what nothing opens" would
                // be every sealed object a run still needs.
                if let Err(e) = cigscript::burn::seal::key() {
                    eprintln!(
                        "{} --sealed-orphans needs the tin's key to tell a sealed object that opens from one that does not: {e}",
                        ctx.red("error[E703 burn]:")
                    );
                    eprintln!("  = hint: put the key back, or move the sealed objects away by hand to start over; nothing was removed");
                    eprintln!("  = explain: cig explain E703");
                    return exit::SCRIPT_ERROR;
                }
            }
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
