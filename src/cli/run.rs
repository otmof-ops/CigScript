// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! `cig run`, `cig check`, `cig eval`.

use super::{exit, Ctx};
use cigscript::burn::{runs::RunRecord, Kernel, Mode, PlannedOp};
use cigscript::check;
use cigscript::diagnostics::{Diagnostic, Kind};
use cigscript::interp::Interp;
use cigscript::syntax::{parse, parse_expr_src};
use cigscript::NO_CIGARETTES;
use std::io::Write;
use std::path::Path;

pub fn read_source(ctx: &Ctx, file: &Path) -> Result<String, i32> {
    std::fs::read_to_string(file).map_err(|e| {
        let hint = match e.kind() {
            std::io::ErrorKind::NotFound => {
                "check the path; scripts resolve against the current directory"
            }
            std::io::ErrorKind::PermissionDenied => "chmod +r the file, or run as its owner",
            _ => "the operating system's reason is in the message",
        };
        let d = cigscript::diagnostics::usage(format!("cannot read {}: {e}", file.display()))
            .code("E801")
            .with_hint(hint);
        report(ctx, &d, None, None);
        exit::USAGE
    })
}

/// Print a diagnostic the way every command does. This is the one emit
/// point: doctor's automatic diagnosis fires here, after the diagnostic,
/// unless `--no-doctor` or `CIG_DOCTOR=0` switched it off.
pub fn report(ctx: &Ctx, diag: &Diagnostic, file: Option<&str>, source: Option<&str>) {
    let diagnosis = if ctx.doctor {
        cigscript::doctor::diagnose(diag, &cigscript::doctor::Context::here(source))
    } else {
        None
    };
    if ctx.json {
        let mut obj = serde_json::to_value(diag).unwrap_or_default();
        if let Some(f) = file {
            obj["file"] = serde_json::Value::String(f.to_string());
        }
        if let Some(dx) = &diagnosis {
            obj["diagnosis"] = serde_json::to_value(dx).unwrap_or_default();
        }
        outln!("{}", serde_json::to_string(&obj).unwrap_or_default());
        return;
    }
    if !ctx.plain {
        eprintln!("{}", ctx.red(NO_CIGARETTES));
    }
    eprint!("{}", diag.render(file, source));
    if let Some(dx) = &diagnosis {
        eprint!("{}", super::doctor::render_diagnosis(ctx, dx));
    }
}

/// Several diagnostics from one check: the banner once, then each one
/// through the same hook. In JSON mode each error carries its diagnosis.
pub fn report_all(ctx: &Ctx, diags: &[Diagnostic], file: &str, source: &str) {
    if ctx.json {
        return;
    }
    if !ctx.plain {
        eprintln!("{}", ctx.red(NO_CIGARETTES));
    }
    for d in diags {
        eprint!("{}", d.render(Some(file), Some(source)));
        if ctx.doctor {
            if let Some(dx) =
                cigscript::doctor::diagnose(d, &cigscript::doctor::Context::here(Some(source)))
            {
                eprint!("{}", super::doctor::render_diagnosis(ctx, &dx));
            }
        }
    }
}

/// A check report's diagnostics as JSON, each with its diagnosis when
/// doctor is on.
pub fn diags_json(ctx: &Ctx, diags: &[Diagnostic], source: &str) -> Vec<serde_json::Value> {
    diags
        .iter()
        .map(|d| {
            let mut v = serde_json::to_value(d).unwrap_or_default();
            if ctx.doctor {
                if let Some(dx) =
                    cigscript::doctor::diagnose(d, &cigscript::doctor::Context::here(Some(source)))
                {
                    v["diagnosis"] = serde_json::to_value(dx).unwrap_or_default();
                }
            }
            v
        })
        .collect()
}

pub fn check(ctx: &Ctx, file: &Path, deny_warnings: bool) -> i32 {
    let source = match read_source(ctx, file) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let name = file.to_string_lossy().to_string();
    let program = match parse(&source) {
        Ok(p) => p,
        Err(d) => {
            report(ctx, &d, Some(&name), Some(&source));
            return exit::SYNTAX_ERROR;
        }
    };
    let rep = check::check(&program, &[]);
    let denied = deny_warnings && !rep.warnings.is_empty();
    if ctx.json {
        let obj = serde_json::json!({
            "file": name,
            "ok": rep.ok(),
            "errors": diags_json(ctx, &rep.errors, &source),
            "warnings": rep.warnings,
            "warnings_denied": denied,
        });
        outln!("{}", serde_json::to_string(&obj).unwrap_or_default());
        return if rep.ok() && !denied {
            exit::OK
        } else {
            exit::SYNTAX_ERROR
        };
    }
    for w in &rep.warnings {
        eprint!(
            "{}",
            ctx.yellow(
                &w.render(Some(&name), Some(&source))
                    .replacen("error[", "warning[", 1)
            )
        );
    }
    if rep.ok() && denied {
        eprintln!(
            "{} {} has {} warning{} and --deny-warnings is set",
            ctx.red("error[E300 check]:"),
            name,
            rep.warnings.len(),
            if rep.warnings.len() == 1 { "" } else { "s" }
        );
        exit::SYNTAX_ERROR
    } else if rep.ok() {
        eprintln!(
            "{} {} ({} statements, {} warning{})",
            ctx.green("ok:"),
            name,
            program.body.len(),
            rep.warnings.len(),
            if rep.warnings.len() == 1 { "" } else { "s" }
        );
        exit::OK
    } else {
        report_all(ctx, &rep.errors, &name, &source);
        exit::SYNTAX_ERROR
    }
}

pub fn eval(ctx: &Ctx, code: &str) -> i32 {
    let kernel = Kernel::ephemeral(Mode::Run);
    let mut interp = Interp::new(kernel);
    // Try as an expression first so `cig eval "1 + 1"` prints 2.
    if let Ok(expr) = parse_expr_src(code, Default::default()) {
        let program = cigscript::syntax::ast::Program {
            body: vec![cigscript::syntax::ast::Stmt::Expr(expr)],
        };
        return finish_eval(ctx, &mut interp, &program, code);
    }
    match parse(code) {
        Ok(program) => finish_eval(ctx, &mut interp, &program, code),
        Err(d) => {
            report(ctx, &d, None, Some(code));
            exit::SYNTAX_ERROR
        }
    }
}

fn finish_eval(
    ctx: &Ctx,
    interp: &mut Interp,
    program: &cigscript::syntax::ast::Program,
    code: &str,
) -> i32 {
    match interp.eval_program(program) {
        Ok(Some(v)) => {
            if ctx.json {
                match cigscript::stdlib::json::from_value(&v, Default::default()) {
                    Ok(j) => outln!("{j}"),
                    Err(_) => outln!("{}", serde_json::Value::String(v.repr())),
                }
            } else {
                outln!("{}", v.repr());
            }
            interp.exit_requested.unwrap_or(exit::OK)
        }
        Ok(None) => interp.exit_requested.unwrap_or(exit::OK),
        Err(d) => {
            report(ctx, &d, None, Some(code));
            exit::SCRIPT_ERROR
        }
    }
}

pub fn run(ctx: &Ctx, args: super::RunArgs, chain: Option<String>) -> i32 {
    let source = match read_source(ctx, &args.file) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let name = args.file.to_string_lossy().to_string();
    let program = match parse(&source) {
        Ok(p) => p,
        Err(d) => {
            report(ctx, &d, Some(&name), Some(&source));
            return exit::SYNTAX_ERROR;
        }
    };
    if !args.no_check {
        let rep = check::check(&program, &[]);
        if !rep.ok() {
            if ctx.json {
                for e in &rep.errors {
                    report(ctx, e, Some(&name), None);
                }
            } else {
                report_all(ctx, &rep.errors, &name, &source);
            }
            return exit::SYNTAX_ERROR;
        }
        if !ctx.json {
            for w in &rep.warnings {
                eprint!(
                    "{}",
                    ctx.yellow(
                        &w.render(Some(&name), Some(&source))
                            .replacen("error[", "warning[", 1)
                    )
                );
            }
        }
    }

    let mode = if args.dry_run {
        Mode::DryRun
    } else {
        Mode::Run
    };
    let mut record = RunRecord::begin(
        &args.file,
        &source,
        &args.args,
        if args.dry_run { "dry-run" } else { "run" },
    );
    let run_dir = if args.dry_run {
        None
    } else {
        if let Err(e) = record.save() {
            eprintln!(
                "{} cannot create the run record under {}: {e}",
                ctx.red("error[E802 usage]:"),
                cigscript::burn::runs::runs_dir().display()
            );
            eprintln!("  = hint: set CIGSCRIPT_HOME to a directory you can write");
            eprintln!("  = explain: cig explain E802");
            return exit::USAGE;
        }
        Some(record.dir())
    };
    let kernel = match Kernel::new(mode, run_dir) {
        Ok(k) => k,
        Err(d) => {
            report(ctx, &d, Some(&name), None);
            return exit::USAGE;
        }
    };
    cigscript::crash::set_active_run(if args.dry_run {
        None
    } else {
        Some(record.id.clone())
    });
    let mut interp = Interp::new(kernel);
    interp.script_name = Some(name.clone());
    interp.set_args(&args.args);
    if let Some(n) = args.max_steps {
        interp.set_max_steps(n);
    }

    let mut result = interp.run_program(&program);
    if result.is_ok() {
        if let Some(chain_name) = &chain {
            result = light_named_chain(&mut interp, chain_name, &program);
        }
    }
    let _ = interp.out.flush();

    let mut code = match &result {
        Ok(()) => interp.exit_requested.unwrap_or(exit::OK),
        Err(_) => exit::SCRIPT_ERROR,
    };
    let mut rolled_back = None;
    if let Err(d) = &result {
        report(ctx, d, Some(&name), Some(&source));
        if !args.dry_run && !args.no_rollback && !interp.kernel.journal().is_empty() {
            let rep = interp.kernel.rollback();
            if !ctx.json {
                eprintln!("{}", ctx.bold("unburn: rolling back this run's burns"));
                for r in &rep.restored {
                    eprintln!("  {} {r}", ctx.green("restored"));
                }
                for r in &rep.irreversible {
                    eprintln!("  {} {r}", ctx.yellow("cannot undo"));
                }
                for r in &rep.failed {
                    eprintln!("  {} {r}", ctx.red("failed"));
                }
            }
            rolled_back = Some(rep);
        }
    }

    if args.dry_run {
        print_plan(
            ctx,
            interp.kernel.dry_run_plan().collect(),
            "dry-run plan",
            "would burn",
        );
    }
    let intents: Vec<&PlannedOp> = interp.kernel.intents().collect();
    if !intents.is_empty() {
        print_plan(
            ctx,
            intents.clone(),
            "unlit intents",
            "declared, never burned",
        );
    }

    if !args.dry_run {
        record.burns = interp.kernel.executed;
        record.irreversible = interp.kernel.irreversible;
        record.rolled_back = rolled_back.as_ref().is_some_and(|r| !r.restored.is_empty());
        let (status, error) = match &result {
            Ok(()) => ("ok", None),
            Err(d) => (
                if record.rolled_back {
                    "rolled_back"
                } else {
                    "error"
                },
                Some(d.message.clone()),
            ),
        };
        if let Err(d) = &result {
            let mut v = serde_json::to_value(d).unwrap_or_default();
            v["file"] = serde_json::Value::String(name.clone());
            record.diagnostic = Some(v);
        }
        record.finish(status, error);
        if let Err(e) = record.save() {
            eprintln!(
                "{} could not finish the run record: {e}",
                ctx.yellow("warning:")
            );
        }
        save_intents(&record, &interp);
        if ctx.json {
            let obj = serde_json::json!({
                "run": record,
                "rollback": rolled_back,
                "intents": interp.kernel.intents().collect::<Vec<_>>(),
            });
            outln!("{}", serde_json::to_string(&obj).unwrap_or_default());
        } else if result.is_ok() && interp.kernel.executed > 0 {
            eprintln!(
                "{}",
                ctx.dim(&format!(
                    "burned {} op{} ({} irreversible), run {}",
                    interp.kernel.executed,
                    if interp.kernel.executed == 1 { "" } else { "s" },
                    interp.kernel.irreversible,
                    record.id
                ))
            );
        }
    } else if ctx.json {
        let obj = serde_json::json!({
            "mode": "dry-run",
            "ok": result.is_ok(),
            "plan": interp.kernel.dry_run_plan().collect::<Vec<_>>(),
            "intents": interp.kernel.intents().collect::<Vec<_>>(),
        });
        outln!("{}", serde_json::to_string(&obj).unwrap_or_default());
    }
    if let Err(d) = &result {
        if d.kind == Kind::Burn {
            code = exit::SCRIPT_ERROR;
        }
    }
    code
}

fn save_intents(record: &RunRecord, interp: &Interp) -> Option<()> {
    let intents: Vec<&PlannedOp> = interp.kernel.intents().collect();
    if intents.is_empty() {
        return None;
    }
    let mut text = String::new();
    for i in intents {
        text.push_str(&serde_json::to_string(i).ok()?);
        text.push('\n');
    }
    std::fs::write(record.dir().join("intents.jsonl"), text).ok()
}

fn print_plan(ctx: &Ctx, ops: Vec<&PlannedOp>, title: &str, verb: &str) {
    if ctx.json {
        return;
    }
    eprintln!(
        "{}",
        ctx.bold(&format!(
            "{title}: {} op{}",
            ops.len(),
            if ops.len() == 1 { "" } else { "s" }
        ))
    );
    for op in ops {
        let tag = if op.reversible {
            ctx.dim("reversible  ")
        } else {
            ctx.yellow("irreversible")
        };
        eprintln!("  {:>4}  {tag}  {}", op.seq, op.summary);
    }
    if verb == "would burn" {
        eprintln!("{}", ctx.dim("nothing was changed"));
    }
}

/// After the script has run, find the named chain in its globals and light it.
fn light_named_chain(
    interp: &mut Interp,
    name: &str,
    program: &cigscript::syntax::ast::Program,
) -> Result<(), Diagnostic> {
    let declared: Vec<String> = super::chains::declared(program)
        .into_iter()
        .map(|c| c.name)
        .collect();
    let value = match interp.globals.get(name) {
        Some(v @ cigscript::value::Value::Chain(_)) => v,
        Some(other) => {
            return Err(Diagnostic::new(
                Kind::Runtime,
                format!("`{name}` is a {}, not a chain", other.type_name()),
            ))
        }
        None => {
            let mut d = Diagnostic::new(
                Kind::Usage,
                format!("no chain named `{name}` in this script"),
            )
            .code("E804");
            if declared.is_empty() {
                d = d.with_hint("declare one with: chain name { step, step }");
            } else {
                d = d.with_hint(format!("chains here: {}", declared.join(", ")));
            }
            return Err(d);
        }
    };
    let light = interp
        .globals
        .get("light")
        .expect("light is in the prelude");
    interp
        .call(&light, vec![value], Default::default())
        .map(|_| ())
}
