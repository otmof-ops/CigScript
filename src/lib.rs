// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! CigScript: a small scripting language where nothing real happens unless
//! you `burn`.
//!
//! The crate is organised as a pipeline:
//!
//! * [`syntax`] turns source text into an AST,
//! * [`check`] walks the AST for static mistakes,
//! * [`interp`] evaluates it, calling into the [`stdlib`],
//! * [`burn`] is the kernel: it gates side effects, journals them and rolls
//!   them back.
//!
//! The `cig` binary in `src/main.rs` is a thin CLI over these modules.

pub mod burn;
pub mod check;
pub mod config;
pub mod crash;
pub mod diagnostics;
pub mod errors;
pub mod interp;
pub mod stdlib;
pub mod syntax;
pub mod update;
pub mod value;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The one line every failed run leads with.
pub const NO_CIGARETTES: &str = "Don't see any cigarettes.";
