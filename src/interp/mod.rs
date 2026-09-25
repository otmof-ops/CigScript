// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! The tree-walking evaluator.

pub mod env;
mod eval;

pub use eval::{closest, error_value, normalize_index};

use crate::burn::{Decision, Kernel, Op};
use crate::diagnostics::{burn as burn_error, Diagnostic, Kind};
use crate::syntax::ast::Program;
use crate::syntax::span::Span;
use crate::value::Value;
use env::{Env, Scope};
use std::io::Write;

/// Deepest nesting of script calls before the interpreter refuses.
pub const MAX_CALL_DEPTH: usize = 4000;

pub struct Interp {
    pub globals: Env,
    pub kernel: Kernel,
    pub out: Box<dyn Write>,
    pub err: Box<dyn Write>,
    burn_depth: u32,
    unlit_depth: u32,
    call_depth: usize,
    /// Set by `cough`, consumed by the nearest `ashtray`.
    pub(crate) cough_payload: Option<Value>,
    /// Name of the script being run, for diagnostics.
    pub script_name: Option<String>,
    /// Exit code requested by `exit(n)`, if any.
    pub exit_requested: Option<i32>,
}

impl Interp {
    pub fn new(kernel: Kernel) -> Self {
        let prelude = Scope::root();
        for (name, value) in crate::stdlib::prelude() {
            prelude.force_declare(name, value, false);
        }
        let globals = Scope::child(&prelude);
        Self {
            globals,
            kernel,
            out: Box::new(std::io::stdout()),
            err: Box::new(std::io::stderr()),
            burn_depth: 0,
            unlit_depth: 0,
            call_depth: 0,
            cough_payload: None,
            script_name: None,
            exit_requested: None,
        }
    }

    /// Expose the script's own arguments as `args`.
    pub fn set_args(&mut self, args: &[String]) {
        let list = Value::list(args.iter().map(Value::str).collect());
        self.globals.force_declare("args", list, false);
    }

    pub fn in_burn(&self) -> bool {
        self.burn_depth > 0
    }

    pub fn in_unlit(&self) -> bool {
        self.unlit_depth > 0
    }

    /// Called by effectful builtins before they act.
    pub fn effect(&mut self, name: &str, op: Op, span: Span) -> Result<Decision, Diagnostic> {
        if self.burn_depth == 0 {
            return Err(burn_error(format!(
                "{name} changes the world, so it must be inside a burn block"
            ))
            .code("E701")
            .at(span)
            .with_hint(format!("wrap it: burn {{ {name}(...) }}")));
        }
        self.kernel
            .decide(op, self.unlit_depth > 0)
            .map_err(|e| e.or_at(span))
    }

    pub fn run_program(&mut self, program: &Program) -> Result<(), Diagnostic> {
        let globals = self.globals.clone();
        let result = self.exec_block_in(&program.body, &globals);
        self.out.flush().ok();
        match result {
            Ok(()) => Ok(()),
            Err(eval::Signal::Error(_)) if self.exit_requested.is_some() => Ok(()),
            Err(eval::Signal::Error(d)) => Err(d),
            Err(eval::Signal::Return(_)) => Ok(()),
            Err(eval::Signal::Break(span)) => {
                Err(Diagnostic::new(Kind::Runtime, "`break` outside of a loop").at(span))
            }
            Err(eval::Signal::Continue(span)) => {
                Err(Diagnostic::new(Kind::Runtime, "`continue` outside of a loop").at(span))
            }
        }
    }

    /// Run statements in the global scope and return the value of the last
    /// expression statement, for `cig eval` and the REPL.
    pub fn eval_program(&mut self, program: &Program) -> Result<Option<Value>, Diagnostic> {
        let globals = self.globals.clone();
        let result = self.eval_block_last(&program.body, &globals);
        self.out.flush().ok();
        match result {
            Ok(v) => Ok(v),
            Err(eval::Signal::Error(_)) if self.exit_requested.is_some() => Ok(None),
            Err(eval::Signal::Error(d)) => Err(d),
            Err(eval::Signal::Return(v)) => Ok(Some(v)),
            Err(eval::Signal::Break(span)) => {
                Err(Diagnostic::new(Kind::Runtime, "`break` outside of a loop").at(span))
            }
            Err(eval::Signal::Continue(span)) => {
                Err(Diagnostic::new(Kind::Runtime, "`continue` outside of a loop").at(span))
            }
        }
    }
}
