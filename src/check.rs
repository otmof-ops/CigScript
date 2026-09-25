// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! Static checks that run before any code executes (`cig check`, and
//! always before `cig run`).
//!
//! The checker resolves names lexically, so it catches typos, assignment to
//! sticks, `snuff` outside a function, `break` outside a loop, and effectful
//! calls that can never be inside a burn.

use crate::diagnostics::{Diagnostic, Kind};
use crate::syntax::ast::*;
use crate::syntax::span::Span;
use crate::value::{Effect, Value};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Binding {
    Mutable,
    Immutable,
    Function,
}

struct Scope {
    names: HashMap<String, Binding>,
}

pub struct Checker {
    scopes: Vec<Scope>,
    /// Qualified builtin names with an effect that needs a burn, e.g. `fs.rm`.
    effectful: HashMap<String, Effect>,
    modules: HashMap<String, Vec<String>>,
    burn_depth: u32,
    fn_depth: u32,
    loop_depth: u32,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

pub struct Report {
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Check a whole program. `extra_globals` are names the host will provide
/// (for the REPL, everything already defined).
pub fn check(program: &Program, extra_globals: &[String]) -> Report {
    let mut c = Checker::new(extra_globals);
    c.block_stmts(&program.body);
    Report {
        errors: c.errors,
        warnings: c.warnings,
    }
}

impl Checker {
    fn new(extra_globals: &[String]) -> Self {
        let mut root = Scope {
            names: HashMap::new(),
        };
        let mut effectful = HashMap::new();
        let mut modules = HashMap::new();
        for (name, value) in crate::stdlib::prelude() {
            root.names.insert(name.to_string(), Binding::Immutable);
            match &value {
                Value::Module(m) => {
                    let mut items = Vec::new();
                    for (k, v) in &m.items {
                        items.push(k.to_string());
                        if let Value::Builtin(b) = v {
                            if b.effect.needs_burn() {
                                effectful.insert(format!("{name}.{k}"), b.effect);
                            }
                        }
                    }
                    modules.insert(name.to_string(), items);
                }
                Value::Builtin(b) if b.effect.needs_burn() => {
                    effectful.insert(name.to_string(), b.effect);
                }
                _ => {}
            }
        }
        root.names.insert("args".to_string(), Binding::Immutable);
        for g in extra_globals {
            root.names.insert(g.clone(), Binding::Mutable);
        }
        Self {
            scopes: vec![
                root,
                Scope {
                    names: HashMap::new(),
                },
            ],
            effectful,
            modules,
            burn_depth: 0,
            fn_depth: 0,
            loop_depth: 0,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn error_code(
        &mut self,
        code: &'static str,
        msg: impl Into<String>,
        span: Span,
    ) -> &mut Diagnostic {
        self.errors
            .push(Diagnostic::new(Kind::Check, msg).code(code).at(span));
        self.errors.last_mut().expect("just pushed")
    }

    fn warn_code(
        &mut self,
        code: &'static str,
        msg: impl Into<String>,
        span: Span,
    ) -> &mut Diagnostic {
        self.warnings
            .push(Diagnostic::new(Kind::Check, msg).code(code).at(span));
        self.warnings.last_mut().expect("just pushed")
    }

    fn push(&mut self) {
        self.scopes.push(Scope {
            names: HashMap::new(),
        });
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &str, binding: Binding, span: Span) {
        let scope = self.scopes.last_mut().expect("scope stack is never empty");
        if scope.names.contains_key(name) {
            self.error_code(
                "E306",
                format!("`{name}` is already declared in this scope"),
                span,
            )
            .hint = Some(format!("assign with `{name} = ...`, or pick another name"));
            return;
        }
        scope.names.insert(name.to_string(), binding);
    }

    fn lookup(&self, name: &str) -> Option<Binding> {
        self.scopes
            .iter()
            .rev()
            .find_map(|s| s.names.get(name).copied())
    }

    fn all_names(&self) -> Vec<String> {
        self.scopes
            .iter()
            .flat_map(|s| s.names.keys().cloned())
            .collect()
    }

    fn unknown(&mut self, name: &str, span: Span) {
        let names = self.all_names();
        let hint = crate::interp::closest(name, names.iter().map(String::as_str))
            .map(|s| format!("did you mean `{s}`?"));
        let d = self.error_code("E301", format!("unknown name `{name}`"), span);
        d.hint = hint;
    }

    fn block_stmts(&mut self, stmts: &[Stmt]) {
        // Hoist functions so calls before the definition resolve.
        for stmt in stmts {
            if let Stmt::Pull(f) = stmt {
                self.declare(&f.name, Binding::Function, f.span);
            }
        }
        for stmt in stmts {
            self.stmt(stmt);
        }
    }

    fn block(&mut self, block: &Block) {
        self.push();
        self.block_stmts(&block.stmts);
        self.pop();
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Declare {
                name,
                mutable,
                value,
                span,
            } => {
                self.expr(value);
                self.declare(
                    name,
                    if *mutable {
                        Binding::Mutable
                    } else {
                        Binding::Immutable
                    },
                    *span,
                );
            }
            Stmt::Assign {
                target,
                value,
                span,
                ..
            } => {
                self.expr(value);
                match target {
                    AssignTarget::Var(name) => match self.lookup(name) {
                        None => {
                            self.unknown(name, *span);
                            if let Some(last) = self.errors.last_mut() {
                                if last.hint.is_none() {
                                    last.hint =
                                        Some(format!("declare it first: roll {name} = ..."));
                                }
                            }
                        }
                        Some(Binding::Immutable) => {
                            self.error_code(
                                "E302",
                                format!("`{name}` is a stick and cannot be reassigned"),
                                *span,
                            )
                            .hint = Some(format!(
                                "declare it with `roll {name} = ...` if it needs to change"
                            ));
                        }
                        Some(Binding::Function) => {
                            self.error_code(
                                "E302",
                                format!("`{name}` is a pull and cannot be reassigned"),
                                *span,
                            );
                        }
                        Some(Binding::Mutable) => {}
                    },
                    AssignTarget::Index { object, index } => {
                        self.expr(object);
                        self.expr(index);
                    }
                    AssignTarget::Member { object, .. } => self.expr(object),
                }
            }
            Stmt::Pull(f) => self.function(&f.params, &f.body.stmts),
            Stmt::Chain(ch) => {
                for step in &ch.steps {
                    self.expr(step);
                }
                self.declare(&ch.name, Binding::Immutable, ch.span);
            }
            Stmt::Snuff { value, span } => {
                if self.fn_depth == 0 {
                    self.error_code("E304", "`snuff` outside of a pull or pack", *span)
                        .hint = Some("to stop the whole script use exit(code)".to_string());
                }
                if let Some(v) = value {
                    self.expr(v);
                }
            }
            Stmt::Exhale { values, .. } => values.iter().for_each(|v| self.expr(v)),
            Stmt::Cough { value, .. } => self.expr(value),
            Stmt::If {
                branches,
                otherwise,
                ..
            } => {
                for (cond, body) in branches {
                    self.expr(cond);
                    self.block(body);
                }
                if let Some(b) = otherwise {
                    self.block(b);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond);
                self.loop_depth += 1;
                self.block(body);
                self.loop_depth -= 1;
            }
            Stmt::For {
                var,
                iter,
                body,
                span,
            } => {
                self.expr(iter);
                self.loop_depth += 1;
                self.push();
                self.declare(var, Binding::Mutable, *span);
                self.block_stmts(&body.stmts);
                self.pop();
                self.loop_depth -= 1;
            }
            Stmt::Break(span) | Stmt::Continue(span) => {
                if self.loop_depth == 0 {
                    let what = if matches!(stmt, Stmt::Break(_)) {
                        "break"
                    } else {
                        "continue"
                    };
                    self.error_code("E305", format!("`{what}` outside of a loop"), *span);
                }
            }
            Stmt::Try {
                body,
                catch_var,
                handler,
                span,
            } => {
                self.block(body);
                self.push();
                if let Some(v) = catch_var {
                    self.declare(v, Binding::Immutable, *span);
                }
                self.block_stmts(&handler.stmts);
                self.pop();
            }
            Stmt::Burn { body, .. } => {
                self.burn_depth += 1;
                self.block(body);
                self.burn_depth -= 1;
            }
            Stmt::Expr(e) => self.expr(e),
        }
    }

    fn function(&mut self, params: &[String], body: &[Stmt]) {
        self.push();
        self.fn_depth += 1;
        let saved_loop = self.loop_depth;
        let saved_burn = self.burn_depth;
        self.loop_depth = 0;
        // A function body may run inside a burn at the call site, so the
        // burn check inside it can only warn.
        for p in params {
            self.declare(p, Binding::Mutable, Span::default());
        }
        self.block_stmts(body);
        self.loop_depth = saved_loop;
        self.burn_depth = saved_burn;
        self.fn_depth -= 1;
        self.pop();
    }

    fn expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Null | ExprKind::Bool(_) | ExprKind::Int(_) | ExprKind::Float(_) => {}
            ExprKind::Str(pieces) => {
                for p in pieces {
                    if let StrPiece::Expr(e) = p {
                        self.expr(e);
                    }
                }
            }
            ExprKind::Ident(name) => {
                if self.lookup(name).is_none() {
                    self.unknown(name, expr.span);
                }
            }
            ExprKind::List(items) => items.iter().for_each(|i| self.expr(i)),
            ExprKind::Map(entries) => {
                for (k, v) in entries {
                    if let MapKey::Dynamic(e) = k {
                        self.expr(e);
                    }
                    self.expr(v);
                }
            }
            ExprKind::Unary { expr: inner, .. } => self.expr(inner),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Call { callee, args } => {
                self.effect_check(callee);
                self.expr(callee);
                args.iter().for_each(|a| self.expr(a));
            }
            ExprKind::Method {
                receiver,
                name,
                args,
                ..
            } => {
                // `fs.rm(...)` parses as a method call on the module ident.
                if let ExprKind::Ident(module) = &receiver.kind {
                    if self.modules.contains_key(module)
                        && self.lookup(module) == Some(Binding::Immutable)
                    {
                        let qualified = format!("{module}.{name}");
                        if let Some(items) = self.modules.get(module) {
                            if !items.contains(name) {
                                let hint =
                                    crate::interp::closest(name, items.iter().map(String::as_str))
                                        .map(|s| format!("did you mean `{module}.{s}`?"));
                                self.error_code(
                                    "E307",
                                    format!("module `{module}` has no `{name}`"),
                                    expr.span,
                                )
                                .hint = hint;
                            }
                        }
                        self.effect_by_name(&qualified, expr.span);
                    }
                }
                self.expr(receiver);
                args.iter().for_each(|a| self.expr(a));
            }
            ExprKind::Member { object, name, .. } => {
                if let ExprKind::Ident(module) = &object.kind {
                    if let Some(items) = self.modules.get(module) {
                        if self.lookup(module) == Some(Binding::Immutable) && !items.contains(name)
                        {
                            let hint =
                                crate::interp::closest(name, items.iter().map(String::as_str))
                                    .map(|s| format!("did you mean `{module}.{s}`?"));
                            self.error_code(
                                "E307",
                                format!("module `{module}` has no `{name}`"),
                                expr.span,
                            )
                            .hint = hint;
                        }
                    }
                }
                self.expr(object);
            }
            ExprKind::Index { object, index } => {
                self.expr(object);
                self.expr(index);
            }
            ExprKind::Lambda { params, body } => match body {
                LambdaBody::Expr(e) => {
                    self.push();
                    self.fn_depth += 1;
                    for p in params {
                        self.declare(p, Binding::Mutable, Span::default());
                    }
                    self.expr(e);
                    self.fn_depth -= 1;
                    self.pop();
                }
                LambdaBody::Block(b) => self.function(params, &b.stmts),
            },
            ExprKind::Pipe { lhs, rhs } => {
                self.expr(lhs);
                if let ExprKind::Call { callee, args } = &rhs.kind {
                    self.effect_check(callee);
                    self.expr(callee);
                    args.iter().for_each(|a| self.expr(a));
                } else {
                    self.expr(rhs);
                }
            }
        }
    }

    /// A direct call of an effectful global outside any burn.
    fn effect_check(&mut self, callee: &Expr) {
        if let ExprKind::Ident(name) = &callee.kind {
            if self.lookup(name) == Some(Binding::Immutable) {
                self.effect_by_name(name, callee.span);
            }
        }
    }

    fn effect_by_name(&mut self, qualified: &str, span: Span) {
        if !self.effectful.contains_key(qualified) || self.burn_depth > 0 {
            return;
        }
        let msg = format!("{qualified} changes the world, so it must be inside a burn block");
        let hint = format!("wrap it: burn {{ {qualified}(...) }}");
        if self.fn_depth > 0 {
            self.warn_code("E303", msg, span).hint =
                Some(format!("{hint}, or call this function from inside one"));
        } else {
            self.error_code("E303", msg, span).hint = Some(hint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    fn run(src: &str) -> Report {
        check(&parse(src).unwrap(), &[])
    }

    #[test]
    fn catches_unknown_names_and_stick_assignment() {
        let r = run("roll a = 1\nexhale b\nstick c = 2\nc = 3\n");
        assert_eq!(r.errors.len(), 2);
        assert!(r.errors[0].message.contains("unknown name `b`"));
        assert!(r.errors[1].message.contains("stick"));
    }

    #[test]
    fn effects_outside_burn_are_errors_at_top_level_and_warnings_in_functions() {
        let r = run("fs.rm(\"x\")\npull f() {\n  fs.rm(\"y\")\n}\nburn {\n  fs.rm(\"z\")\n}\n");
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert_eq!(r.warnings.len(), 1);
        assert!(run("fs.read_text(\"x\")").ok());
    }

    #[test]
    fn hoisting_and_scopes() {
        assert!(run("exhale f()\npull f() {\n  snuff 1\n}\n").ok());
        let r = run("if true {\n  roll inner = 1\n}\nexhale inner\n");
        assert_eq!(r.errors.len(), 1);
        let r = run("snuff 1\n");
        assert!(r.errors[0].message.contains("snuff"));
        let r = run("break\n");
        assert!(r.errors[0].message.contains("break"));
    }

    #[test]
    fn module_typos_are_caught() {
        let r = run("exhale fs.read_txt(\"x\")");
        assert!(r.errors[0].hint.as_deref().unwrap().contains("read_text"));
    }
}
