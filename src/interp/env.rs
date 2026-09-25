// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! Lexical scopes.

use crate::value::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub type Env = Rc<Scope>;

pub struct Scope {
    vars: RefCell<HashMap<String, Slot>>,
    parent: Option<Env>,
}

struct Slot {
    value: Value,
    mutable: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AssignError {
    NotFound,
    Immutable,
}

impl Scope {
    pub fn root() -> Env {
        Rc::new(Scope {
            vars: RefCell::new(HashMap::new()),
            parent: None,
        })
    }

    pub fn child(parent: &Env) -> Env {
        Rc::new(Scope {
            vars: RefCell::new(HashMap::new()),
            parent: Some(parent.clone()),
        })
    }

    /// Declare a name in this scope. Returns `false` if it already exists here.
    pub fn declare(&self, name: &str, value: Value, mutable: bool) -> bool {
        let mut vars = self.vars.borrow_mut();
        if vars.contains_key(name) {
            return false;
        }
        vars.insert(name.to_string(), Slot { value, mutable });
        true
    }

    /// Declare or overwrite, used by the prelude and the REPL.
    pub fn force_declare(&self, name: &str, value: Value, mutable: bool) {
        self.vars
            .borrow_mut()
            .insert(name.to_string(), Slot { value, mutable });
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        if let Some(slot) = self.vars.borrow().get(name) {
            return Some(slot.value.clone());
        }
        self.parent.as_ref().and_then(|p| p.get(name))
    }

    pub fn assign(&self, name: &str, value: Value) -> Result<(), AssignError> {
        if let Some(slot) = self.vars.borrow_mut().get_mut(name) {
            if !slot.mutable {
                return Err(AssignError::Immutable);
            }
            slot.value = value;
            return Ok(());
        }
        match &self.parent {
            Some(p) => p.assign(name, value),
            None => Err(AssignError::NotFound),
        }
    }

    /// All names visible from this scope, for "did you mean" hints.
    pub fn visible_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.vars.borrow().keys().cloned().collect();
        if let Some(p) = &self.parent {
            names.extend(p.visible_names());
        }
        names
    }
}
