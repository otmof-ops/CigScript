// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

mod cli;

fn main() {
    cigscript::crash::install_hook();
    cigscript::update::cleanup_old();
    // Scripts recurse; give the evaluator a generous stack.
    let handle = std::thread::Builder::new()
        .name("cig".into())
        .stack_size(256 * 1024 * 1024)
        .spawn(|| {
            if std::env::var_os("CIG_INTERNAL_PANIC").is_some() {
                // Test hook for the crash reporter.
                panic!("deliberate test panic (CIG_INTERNAL_PANIC is set)");
            }
            cli::main()
        })
        .expect("could not start the interpreter thread");
    let code = match handle.join() {
        Ok(code) => code,
        Err(_) => {
            cli::after_crash();
            70
        }
    };
    std::process::exit(code);
}
