// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! Environment variables. Reading is free; changing them affects every
//! process this script starts, so it is a burn.

use super::b;
use crate::burn::{Decision, Op};
use crate::diagnostics::{runtime, Diagnostic};
use crate::interp::Interp;
use crate::syntax::span::Span;
use crate::value::{expect_str, Builtin, Effect, Value};
use indexmap::IndexMap;

pub static TABLE: &[Builtin] = &[
    b("env.get", Effect::Read, 1, Some(2), get),
    b("env.has", Effect::Read, 1, Some(1), has),
    b("env.all", Effect::Read, 0, Some(0), all),
    b("env.set", Effect::Env, 2, Some(2), set),
    b("env.unset", Effect::Env, 1, Some(1), unset),
];

fn get(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let key = expect_str(a, 0, "env.get", s)?;
    Ok(match std::env::var(key) {
        Ok(v) => Value::str(v),
        Err(_) => a.get(1).cloned().unwrap_or(Value::Null),
    })
}

fn has(_: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    Ok(Value::Bool(
        std::env::var_os(expect_str(a, 0, "env.has", s)?).is_some(),
    ))
}

fn all(_: &mut Interp, _: &[Value], _: Span) -> Result<Value, Diagnostic> {
    let mut vars: Vec<(String, String)> = std::env::vars().collect();
    vars.sort();
    let mut m = IndexMap::with_capacity(vars.len());
    for (k, v) in vars {
        m.insert(k, Value::str(v));
    }
    Ok(Value::map(m))
}

fn valid_key(key: &str, name: &str, s: Span) -> Result<(), Diagnostic> {
    if key.is_empty() || key.contains('=') || key.contains('\0') {
        return Err(runtime(format!("{name}: `{key}` is not a valid variable name")).at(s));
    }
    Ok(())
}

fn set(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let key = expect_str(a, 0, "env.set", s)?.to_string();
    valid_key(&key, "env.set", s)?;
    let value = a[1].display();
    if i.effect("env.set", Op::EnvSet { key: key.clone() }, s)? == Decision::Execute {
        // SAFETY: the interpreter is single-threaded while a script runs;
        // reader threads spawned by proc.run only read pipes.
        unsafe { std::env::set_var(&key, &value) };
    }
    Ok(Value::Null)
}

fn unset(i: &mut Interp, a: &[Value], s: Span) -> Result<Value, Diagnostic> {
    let key = expect_str(a, 0, "env.unset", s)?.to_string();
    valid_key(&key, "env.unset", s)?;
    if i.effect("env.unset", Op::EnvUnset { key: key.clone() }, s)? == Decision::Execute {
        // SAFETY: see `set`.
        unsafe { std::env::remove_var(&key) };
    }
    Ok(Value::Null)
}
