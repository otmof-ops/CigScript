// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig chains`: list the chains a script declares, without running it.

use super::{exit, Ctx};
use cigscript::syntax::ast::{ExprKind, Stmt};
use cigscript::syntax::parse;
use std::path::Path;

pub struct ChainInfo {
    pub name: String,
    pub line: u32,
    pub steps: Vec<String>,
}

/// Top-level chain declarations, in source order.
pub fn declared(program: &cigscript::syntax::ast::Program) -> Vec<ChainInfo> {
    program
        .body
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Chain(c) => Some(ChainInfo {
                name: c.name.clone(),
                line: c.span.line,
                steps: c
                    .steps
                    .iter()
                    .enumerate()
                    .map(|(n, e)| match &e.kind {
                        ExprKind::Ident(name) => name.clone(),
                        ExprKind::Member { name, .. } => name.clone(),
                        ExprKind::Lambda { .. } => format!("step {} (inline pack)", n + 1),
                        ExprKind::Call { .. } => format!("step {} (call)", n + 1),
                        _ => format!("step {}", n + 1),
                    })
                    .collect(),
            }),
            _ => None,
        })
        .collect()
}

pub fn chains(ctx: &Ctx, file: &Path) -> i32 {
    let source = match super::run::read_source(ctx, file) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let name = file.to_string_lossy().to_string();
    let program = match parse(&source) {
        Ok(p) => p,
        Err(d) => {
            super::run::report(ctx, &d, Some(&name), Some(&source));
            return exit::SYNTAX_ERROR;
        }
    };
    let found = declared(&program);
    if ctx.json {
        let items: Vec<serde_json::Value> = found
            .iter()
            .map(|c| serde_json::json!({"name": c.name, "line": c.line, "steps": c.steps}))
            .collect();
        outln!("{}", serde_json::json!({"file": name, "chains": items}));
        return exit::OK;
    }
    if found.is_empty() {
        eprintln!("{name}: no chains declared");
        eprintln!("{}", ctx.dim("declare one with: chain name { step, step }"));
        return exit::OK;
    }
    for c in &found {
        outln!(
            "{}  {}",
            ctx.bold(&c.name),
            ctx.dim(&format!("(line {})", c.line))
        );
        for (n, s) in c.steps.iter().enumerate() {
            outln!("  {:>2}. {s}", n + 1);
        }
    }
    outln!(
        "{}",
        ctx.dim(&format!("light one with: cig light {name} <chain>"))
    );
    exit::OK
}
