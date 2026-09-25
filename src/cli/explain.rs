// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig explain <code>`: what an error code means and how to fix it.

use super::{exit, Ctx};
use cigscript::errors::{lookup, CATALOGUE};

pub fn explain(ctx: &Ctx, code: Option<String>) -> i32 {
    match code {
        None => {
            if ctx.json {
                let all: Vec<serde_json::Value> = CATALOGUE
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "code": e.code, "kind": e.kind.as_str(), "title": e.title,
                            "meaning": e.meaning, "fix": e.fix,
                        })
                    })
                    .collect();
                outln!("{}", serde_json::to_string_pretty(&all).unwrap_or_default());
                return exit::OK;
            }
            outln!("{}", ctx.bold("CigScript error codes"));
            outln!("{}", ctx.dim("E1xx lex, E2xx syntax, E3xx check, E4xx types, E5xx runtime, E6xx raised by the script, E7xx kernel, E8xx usage, E9xx internal"));
            let mut last = ' ';
            for e in CATALOGUE {
                let group = e.code.chars().nth(1).unwrap_or(' ');
                if group != last {
                    outln!();
                    last = group;
                }
                outln!(
                    "  {}  {:<32} {}",
                    ctx.bold(e.code),
                    e.title,
                    ctx.dim(e.kind.as_str())
                );
            }
            outln!();
            outln!(
                "{}",
                ctx.dim("cig explain <code> for the meaning and the fix")
            );
            exit::OK
        }
        Some(code) => match lookup(&code) {
            Some(e) => {
                if ctx.json {
                    outln!(
                        "{}",
                        serde_json::json!({
                            "code": e.code, "kind": e.kind.as_str(), "title": e.title,
                            "meaning": e.meaning, "fix": e.fix,
                        })
                    );
                    return exit::OK;
                }
                outln!(
                    "{} {}  {}",
                    ctx.bold(e.code),
                    ctx.bold(e.title),
                    ctx.dim(&format!("({})", e.kind.as_str()))
                );
                outln!();
                outln!("  {}", e.meaning);
                outln!();
                outln!("  {} {}", ctx.green("fix:"), e.fix);
                exit::OK
            }
            None => {
                eprintln!(
                    "{} no error code `{code}`; codes look like E502. `cig explain` lists them all.",
                    ctx.red("error[E800 usage]:")
                );
                exit::USAGE
            }
        },
    }
}
