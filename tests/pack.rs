// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! The pack (a declared scope), compensations (`burn { } unburn { }`), and
//! the fold checks: HAMMER.md sections 5 and 6. The four plans the package
//! asks for (inside, outside, cough, unburn) are here. Unix: the children
//! are `sh` scripts.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn write(&self, name: &str, content: &str) -> PathBuf {
        let p = self.path().join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, content).unwrap();
        p
    }
    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
    }
    fn exists(&self, name: &str) -> bool {
        self.path().join(name).exists()
    }
    fn cig(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cig"))
            .args(args)
            .current_dir(self.path())
            .env("CIGSCRIPT_HOME", self.path().join("home"))
            .env("NO_COLOR", "1")
            .env("HOME", self.path())
            .output()
            .expect("run cig")
    }
    fn stderr(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.cig(args).stderr)
            .replace(self.path().to_str().unwrap(), "<sb>")
    }
    fn stdout(&self, args: &[&str]) -> String {
        String::from_utf8_lossy(&self.cig(args).stdout).to_string()
    }
    fn run_id(&self) -> String {
        let v: Vec<serde_json::Value> =
            serde_json::from_slice(&self.cig(&["--json", "runs"]).stdout).unwrap();
        v[0]["id"].as_str().unwrap().to_string()
    }
}

/// A script with a pack, a native write inside it, a child that writes
/// inside it, and (when asked) a child that writes outside it.
fn packed(outside: bool, tail: &str) -> String {
    format!(
        concat!(
            "pack {{ \"./build\" }}\n",
            "burn (s) {{\n",
            "  fs.write_text(\"build/note.txt\", \"native\")\n",
            "  proc.run(\"sh\", [\"-c\", \"echo child > build/made.txt\"])\n",
            "{outside}",
            "  s.made = \"build/made.txt\"\n",
            "}} unburn {{\n",
            "  fs.write_text(\"build/undone.txt\", \"compensated \" + s.made)\n",
            "}}\n",
            "{tail}"
        ),
        outside = if outside {
            "  proc.run(\"sh\", [\"-c\", \"echo stray > stray.txt\"])\n"
        } else {
            ""
        },
        tail = tail
    )
}

#[test]
fn plan_one_child_writes_inside_the_pack() {
    let sb = Sandbox::new();
    fs::create_dir_all(sb.path().join("build")).unwrap();
    sb.write("s.cig", &packed(false, "exhale \"planned\"\n"));
    let out = sb.cig(&["run", "--dry-run", "s.cig"]);
    let err = String::from_utf8_lossy(&out.stderr).replace(sb.path().to_str().unwrap(), "<sb>");
    assert!(out.status.success(), "{err}");
    assert_eq!(
        err,
        concat!(
            "dry-run plan: 3 ops\n",
            "  pack: ~/build\n",
            "     1  reversible    write build/note.txt\n",
            "     2  compensated   run sh -c 'echo child > build/made.txt'\n",
            "     3  reversible    compensation for the burn at line 6\n",
            "nothing was changed\n"
        )
    );
    assert!(!sb.exists("build/note.txt") && !sb.exists("build/made.txt"));
    // For real: the hop's file is observed and journaled as reversible.
    let out = sb.cig(&["run", "s.cig"]);
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{err}");
    assert!(
        err.contains("burned 3 ops (0 irreversible)"),
        "the hop is compensated, the created file reversible: {err}"
    );
    assert_eq!(sb.read("build/made.txt"), "child\n");
    let shown = sb.stdout(&["runs", &sb.run_id()]);
    assert!(
        shown.contains("hop created") && shown.contains("build/made.txt"),
        "{shown}"
    );
}

#[test]
fn plan_two_child_writes_outside_the_pack_is_labelled_not_hidden() {
    let sb = Sandbox::new();
    fs::create_dir_all(sb.path().join("build")).unwrap();
    sb.write("s.cig", &packed(true, ""));
    let out = sb.cig(&["run", "s.cig"]);
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{err}");
    assert!(
        err.contains(
            "warning[E752 burn]: `sh -c echo stray > stray.txt` wrote outside the pack: 1 path"
        ),
        "{err}"
    );
    assert!(
        err.contains("stray.txt") && err.contains("cannot undo them"),
        "{err}"
    );
    assert!(sb.exists("stray.txt"));
    // A native write outside the pack is refused outright, before or at run time.
    sb.write(
        "lit.cig",
        "pack { \"./build\" }\nburn {\n  fs.write_text(\"elsewhere.txt\", \"no\")\n}\n",
    );
    let err = sb.stderr(&["run", "lit.cig"]);
    assert!(
        err.contains("error[E750 check]: `elsewhere.txt` is outside the pack (<sb>/build)"),
        "{err}"
    );
    assert!(!sb.exists("elsewhere.txt"));
    sb.write("dyn.cig", "pack { \"./build\" }\nstick p = \"else\" + \"where.txt\"\nburn {\n  fs.write_text(p, \"no\")\n}\n");
    let err = sb.stderr(&["run", "dyn.cig"]);
    assert!(
        err.contains("error[E751 burn]: elsewhere.txt is outside the pack (<sb>/build)"),
        "{err}"
    );
    assert!(!sb.exists("elsewhere.txt"));
    // The refusal holds in the plan too.
    let err = sb.stderr(&["run", "--dry-run", "dyn.cig"]);
    assert!(err.contains("E751"), "{err}");
}

#[test]
fn plan_three_a_cough_rolls_back_and_runs_the_compensation() {
    let sb = Sandbox::new();
    fs::create_dir_all(sb.path().join("build")).unwrap();
    sb.write("s.cig", &packed(false, "cough \"abort\"\n"));
    let out = sb.cig(&["run", "s.cig"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        err.contains("unburn: rolling back this run's burns"),
        "{err}"
    );
    assert!(
        err.contains("compensated ran the compensation for the burn at line 6"),
        "{err}"
    );
    assert!(
        err.contains("compensated run sh -c"),
        "the hop is listed as compensated, not cannot-undo: {err}"
    );
    assert!(
        err.contains("restored hop created") && err.contains("restored write build/note.txt"),
        "{err}"
    );
    assert!(
        !sb.exists("build/note.txt"),
        "the native write was restored"
    );
    assert!(
        !sb.exists("build/made.txt"),
        "the file the child created was removed"
    );
    assert_eq!(sb.read("build/undone.txt"), "compensated build/made.txt");
}

#[test]
fn plan_four_unburn_after_a_success_unfolds_everything_in_reverse() {
    let sb = Sandbox::new();
    fs::create_dir_all(sb.path().join("build")).unwrap();
    sb.write("s.cig", &packed(false, ""));
    assert!(sb.cig(&["run", "s.cig"]).status.success());
    assert_eq!(sb.read("build/made.txt"), "child\n");
    let id = sb.run_id();
    let plan = sb.stdout(&["unburn", &id, "--dry-run"]);
    assert!(
        plan.contains("compensate  compensation for the burn at line 6"),
        "{plan}"
    );
    assert!(plan.contains("compensated  run sh -c"), "{plan}");
    let out = sb.cig(&["unburn", &id]);
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<&str> = text.lines().collect();
    // Newest first: the compensation, then the hop's created file, then the native write.
    assert!(
        lines[0].contains("compensated ran the compensation"),
        "{text}"
    );
    assert!(
        lines.iter().any(|l| l.contains("restored hop created")),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("restored write build/note.txt")),
        "{text}"
    );
    assert!(!sb.exists("build/made.txt") && !sb.exists("build/note.txt"));
    assert_eq!(sb.read("build/undone.txt"), "compensated build/made.txt");
    // The compensation ran from the journal alone: the script is gone.
    sb.write("t.cig", &packed(false, ""));
    assert!(sb.cig(&["run", "t.cig"]).status.success());
    let id = sb.run_id();
    fs::remove_file(sb.path().join("t.cig")).unwrap();
    fs::remove_file(sb.path().join("build/undone.txt")).unwrap();
    assert!(sb.cig(&["unburn", &id]).status.success());
    assert_eq!(sb.read("build/undone.txt"), "compensated build/made.txt");
}

#[test]
fn fold_checks_refuse_with_codes() {
    let sb = Sandbox::new();
    // E753: the pack must be first, and once.
    sb.write("late.cig", "roll x = 1\npack { \"./build\" }\n");
    let err = sb.stderr(&["check", "late.cig"]);
    assert!(
        err.contains("error[E753 check]: pack { } must be the first statement"),
        "{err}"
    );
    sb.write("twice.cig", "pack { \"./build\" }\npack { \"./dist\" }\n");
    let err = sb.stderr(&["check", "twice.cig"]);
    assert!(
        err.contains("E753") && err.contains("a second pack"),
        "{err}"
    );
    // E754: a compensation may only use its state, modules and builtins.
    sb.write(
        "comp.cig",
        "stick remote = \"origin\"\nburn (s) {\n  s.sha = \"abc\"\n  proc.run(\"true\", [])\n} unburn {\n  proc.run(\"git\", [\"push\", remote, s.sha])\n}\n",
    );
    let err = sb.stderr(&["check", "comp.cig"]);
    assert!(
        err.contains("error[E754 check]: the compensation uses `remote` from outside its state"),
        "{err}"
    );
    assert!(err.contains("state.remote = remote"), "{err}");
    sb.write(
        "ok.cig",
        "stick remote = \"origin\"\nburn (s) {\n  s.remote = remote\n  s.sha = \"abc\"\n} unburn {\n  exhale \"would push\", s.remote, s.sha, str(len(s.sha))\n}\n",
    );
    assert!(
        sb.cig(&["check", "ok.cig"]).status.success(),
        "{}",
        sb.stderr(&["check", "ok.cig"])
    );
    // E604: a chain that lights itself.
    sb.write(
        "loop.cig",
        "stick step = pack() => 1\nchain a { step, pack() => light(b) }
chain b { step, pack() => light(a) }\nlight(a)\n",
    );
    let err = sb.stderr(&["run", "loop.cig"]);
    assert!(
        err.contains("error[E604 cough]: chain `a` lights itself: a -> b -> a"),
        "{err}"
    );
    // Every fold code has a page.
    for code in ["E604", "E750", "E751", "E752", "E753", "E754"] {
        assert!(sb.cig(&["explain", code]).status.success(), "{code}");
    }
}

#[test]
fn the_plan_tags_ops_with_the_chain_and_step_that_own_them() {
    let sb = Sandbox::new();
    sb.write(
        "tasks.cig",
        concat!(
            "stick fetch = pack() { burn { fs.write_text(\"src.txt\", \"a\") } }\n",
            "stick build = pack() { burn { fs.write_text(\"out.txt\", fs.read_text(\"src.txt\")) } }\n",
            "chain release { fetch, build }\n"
        ),
    );
    let err = sb.stderr(&["light", "--dry-run", "tasks.cig", "release"]);
    assert!(err.contains("write src.txt  [release/fetch]"), "{err}");
    assert!(err.contains("write out.txt  [release/build]"), "{err}");
    // Unlit burns and compensations inside a block: the compensation is not
    // recorded for a rehearsal.
    sb.write(
        "u.cig",
        "burn unlit (s) {\n  s.x = 1\n  proc.run(\"true\", [])\n} unburn {\n  exhale s.x\n}\n",
    );
    let err = sb.stderr(&["run", "u.cig"]);
    assert!(
        err.contains("unlit intents: 1 op") && !err.contains("compensation"),
        "{err}"
    );
}
