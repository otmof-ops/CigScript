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
            // Test hook for the crash reporter: `1` panics here, `eval` inside
            // the evaluator, `after-burn` in the kernel after the first burn.
            if let Ok(v) = std::env::var("CIG_INTERNAL_PANIC") {
                if v != "eval" && v != "after-burn" {
                    panic!("deliberate test panic (CIG_INTERNAL_PANIC is set)");
                }
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
