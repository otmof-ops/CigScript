// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! The error-code catalogue.
//!
//! Every diagnostic carries a stable code. The first digit is the layer:
//!
//! | range | layer |
//! |---|---|
//! | E1xx | lexing |
//! | E2xx | parsing |
//! | E3xx | static check (`cig check`) |
//! | E4xx | types and arity |
//! | E5xx | runtime |
//! | E6xx | raised by the script (`cough`, `assert`, chains) |
//! | E7xx | the burn kernel |
//! | E8xx | the command line and the environment |
//! | E9xx | internal: a bug in CigScript itself |
//!
//! `cig explain <code>` prints an entry; `docs/ERRORS.md` is generated from
//! this table so the two can never disagree.

use crate::diagnostics::Kind;

#[derive(Clone, Copy, Debug)]
pub struct ErrorInfo {
    pub code: &'static str,
    pub kind: Kind,
    pub title: &'static str,
    pub meaning: &'static str,
    pub fix: &'static str,
}

const fn e(
    code: &'static str,
    kind: Kind,
    title: &'static str,
    meaning: &'static str,
    fix: &'static str,
) -> ErrorInfo {
    ErrorInfo {
        code,
        kind,
        title,
        meaning,
        fix,
    }
}

/// Generic code for a kind, used when nothing more specific applies.
pub const fn default_code(kind: Kind) -> &'static str {
    match kind {
        Kind::Lex => "E100",
        Kind::Syntax => "E200",
        Kind::Check => "E300",
        Kind::Type => "E400",
        Kind::Runtime => "E500",
        Kind::Cough => "E600",
        Kind::Burn => "E700",
        Kind::Usage => "E800",
        Kind::Internal => "E900",
    }
}

pub static CATALOGUE: &[ErrorInfo] = &[
    // lexing
    e("E100", Kind::Lex, "lexing error", "The source contains something the lexer cannot turn into tokens.", "Read the message; it names the character or literal."),
    e("E101", Kind::Lex, "unterminated string", "A string literal has no closing quote on its line.", "Close the string, or use \\n for a newline inside it."),
    e("E102", Kind::Lex, "unknown escape", "A backslash sequence in a double-quoted string is not one CigScript knows.", "Use \\n \\t \\r \\0 \\\\ \\\" \\$ \\u{hex}; for regexes use a raw 'single-quoted' string."),
    e("E103", Kind::Lex, "unexpected character", "A character cannot start any token, often a C-style operator.", "Spell logic as `and`, `or`, `not`; comparisons as == != < <= > >=."),
    e("E104", Kind::Lex, "malformed number", "A numeric literal does not fit in 64 bits or is not well formed.", "Use a float for very large values, or fix the literal."),
    // parsing
    e("E200", Kind::Syntax, "syntax error", "The source does not follow the grammar.", "The caret marks where parsing stopped; `cig language` shows the forms."),
    e("E201", Kind::Syntax, "expected an expression", "A value was expected here and something else was found.", "Complete the expression; check for a stray operator or comma."),
    e("E202", Kind::Syntax, "unclosed block", "A `{` has no matching `}`.", "Add the closing brace; each block opener needs one."),
    e("E203", Kind::Syntax, "expected end of statement", "Two statements share a line without a separator.", "Put each statement on its own line or separate them with `;`."),
    e("E204", Kind::Syntax, "not a statement", "A word that is not a keyword starts the line, usually a typo.", "Check the spelling against the hint; `cig language` lists the keywords."),
    e("E205", Kind::Syntax, "invalid assignment target", "The left side of `=` is not something that can hold a value.", "Assign to a name, `list[i]` or `map.key`."),
    e("E206", Kind::Syntax, "chained comparison", "Comparisons cannot be chained like `a < b < c`.", "Write `a < b and b < c`."),
    e("E207", Kind::Syntax, "empty chain", "A chain declares no steps.", "List the sticks to light: chain name { fetch, build }."),
    // static check
    e("E300", Kind::Check, "check error", "The static checker found a mistake before running anything.", "Read the message; run `cig check` to see all of them."),
    e("E301", Kind::Check, "unknown name", "A name is used that was never declared in a visible scope.", "Declare it with `roll` or `stick`, or fix the spelling (see the hint)."),
    e("E302", Kind::Check, "stick reassigned", "A `stick` is a constant and cannot be assigned again.", "Declare it with `roll` if it needs to change."),
    e("E303", Kind::Check, "effect outside burn", "A function that changes the world is called where no `burn` block encloses it.", "Wrap the call: burn { ... }. Inside a pull this is a warning, since the caller may burn."),
    e("E304", Kind::Check, "snuff outside a function", "`snuff` returns from a pull or pack and was used at the top level.", "Use exit(code) to stop the script."),
    e("E305", Kind::Check, "break outside a loop", "`break` or `continue` appears outside `while` or `for`.", "Move it inside the loop, or restructure with `if`."),
    e("E306", Kind::Check, "already declared", "A name is declared twice in the same scope.", "Assign with `name = ...`, or choose another name."),
    e("E307", Kind::Check, "unknown module member", "A module such as `fs` has no function by that name.", "See the hint, or `cig language` for the full list."),
    // types
    e("E400", Kind::Type, "type error", "A value of the wrong type reached an operation.", "Convert with str(), int(), float(), or check type_of()."),
    e("E401", Kind::Type, "wrong number of arguments", "A call passes more or fewer arguments than the function takes.", "Check the signature; CigScript has no optional parameters for packs."),
    e("E402", Kind::Type, "wrong argument type", "A library function received an argument of the wrong type.", "The message names the argument and the expected type."),
    e("E403", Kind::Type, "operator not applicable", "An operator was applied to types it does not support.", "Strings join with +, numbers add; convert first with str() or int()."),
    e("E404", Kind::Type, "not callable", "Something that is not a function was called.", "Only packs, pulls and library functions can be called."),
    e("E405", Kind::Type, "not indexable or iterable", "Indexing or iteration was attempted on an unsupported type.", "Lists, strings and maps support this; check type_of()."),
    // runtime
    e("E500", Kind::Runtime, "runtime error", "Something failed while the script was running.", "The message says what; wrap risky code in try/ashtray to handle it."),
    e("E501", Kind::Runtime, "unknown name at run time", "A name was looked up that is not in scope.", "Usually only reachable with --no-check; run `cig check`."),
    e("E502", Kind::Runtime, "division by zero", "An integer or float was divided by zero, or the modulus was zero.", "Guard the divisor: if d != 0 { ... }."),
    e("E503", Kind::Runtime, "integer overflow", "An integer operation left the 64-bit range.", "Use floats for very large magnitudes."),
    e("E504", Kind::Runtime, "index out of range", "A list or string index is outside its length.", "Check .len() first, or use .get(i, default)."),
    e("E505", Kind::Runtime, "missing key", "A map has no entry for the key that was read.", "Use m?.key for null, or m.get(\"key\", default)."),
    e("E506", Kind::Runtime, "call depth exceeded", "Recursion went deeper than 4,000 calls.", "Add a base case, or rewrite as a loop."),
    e("E507", Kind::Runtime, "stick reassigned at run time", "A constant was assigned through a path the checker could not see.", "Declare it with `roll` if it needs to change."),
    e("E508", Kind::Runtime, "file system failure", "A file or directory operation failed.", "The message shows the path and the operating-system reason."),
    e("E509", Kind::Runtime, "process failure", "A program could not be started, or exceeded its timeout and was killed.", "Check it is installed (proc.which), or raise {timeout_ms}."),
    e("E510", Kind::Runtime, "invalid regular expression", "A pattern passed to the text module does not compile.", "Use a raw 'single-quoted' string and check the pattern."),
    e("E511", Kind::Runtime, "parse failure", "Text that should be JSON or CSV is not.", "Validate the input; the message includes the parser's reason."),
    e("E512", Kind::Runtime, "range too large", "A range or list construction would exceed 10,000,000 items.", "Iterate in smaller pieces."),
    e("E513", Kind::Runtime, "output closed", "The reader of stdout went away (for example `| head`).", "Nothing to fix; the script stopped cleanly."),
    // raised by the script
    e("E600", Kind::Cough, "raised by the script", "The script called `cough` and nothing caught it.", "Catch it with try/ashtray, or let it stop the run; burns are rolled back."),
    e("E601", Kind::Cough, "assertion failed", "assert() was given a false value.", "The optional second argument becomes the message."),
    e("E602", Kind::Cough, "chain failed", "A step of a chain raised an error and the chain stopped.", "The error map carries chain, step, index and cause; or light with {continue_on_error: true}."),
    e("E603", Kind::Cough, "process check failed", "proc.run with {check: true} saw a non-zero exit code.", "The error map holds code, out and err."),
    // kernel
    e("E700", Kind::Burn, "burn refused", "The kernel refused a side effect.", "Read the message; effects need a burn block."),
    e("E701", Kind::Burn, "effect outside burn", "A world-changing call ran with no burn block active on the call stack.", "Wrap the call, or the call to the function that makes it, in burn { }."),
    e("E702", Kind::Burn, "journal failure", "The kernel could not record a burn before performing it, so it refused it.", "Check disk space and permissions under ~/.cigscript (or CIGSCRIPT_HOME)."),
    // usage
    e("E800", Kind::Usage, "usage error", "The command line or the environment is wrong.", "See `cig --help`."),
    e("E801", Kind::Usage, "cannot read script", "The script file could not be opened.", "Check the path and permissions."),
    e("E802", Kind::Usage, "state directory unavailable", "The state directory cannot be created or written.", "Set CIGSCRIPT_HOME to a writable location."),
    e("E803", Kind::Usage, "no such run", "No run record matches the id or prefix given.", "`cig runs` lists them; give more of the id if it is ambiguous."),
    e("E804", Kind::Usage, "no such chain", "The script declares no chain by that name.", "`cig chains <file>` lists them."),
    e("E805", Kind::Usage, "network tool missing", "Neither `gh` nor `curl` is available for the update or report.", "Install one of them; `cig doctor --fix` offers to."),
    e("E806", Kind::Usage, "update failed", "The release could not be fetched, verified or installed.", "The message says which step; retry, or install manually from the releases page."),
    // internal
    e("E900", Kind::Internal, "internal error", "CigScript hit a condition it believes impossible.", "Please report it: `cig crash send` after the crash, or file an issue."),
    e("E901", Kind::Internal, "crash", "CigScript panicked. A crash report was written locally.", "Run `cig crash list` and `cig crash send <id>` to file it, or say yes at the prompt."),
];

pub fn lookup(code: &str) -> Option<&'static ErrorInfo> {
    let wanted = code.trim().to_ascii_uppercase();
    CATALOGUE.iter().find(|e| e.code == wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for e in CATALOGUE {
            assert!(e.code.len() == 4 && e.code.starts_with('E'), "{}", e.code);
            assert!(seen.insert(e.code), "duplicate {}", e.code);
            assert_eq!(
                &e.code[1..2],
                &default_code(e.kind)[1..2],
                "{} is in the wrong range for {:?}",
                e.code,
                e.kind
            );
        }
        for kind in [
            Kind::Lex,
            Kind::Syntax,
            Kind::Check,
            Kind::Type,
            Kind::Runtime,
            Kind::Cough,
            Kind::Burn,
            Kind::Usage,
            Kind::Internal,
        ] {
            assert!(
                lookup(default_code(kind)).is_some(),
                "no catalogue entry for {kind:?}"
            );
        }
        assert_eq!(lookup("e502").unwrap().title, "division by zero");
    }
}
