// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/CigScript)
// SPDX-License-Identifier: Apache-2.0

//! End-to-end tests against the built `cig` binary.
//!
//! Every test gets its own state directory through `CIGSCRIPT_HOME`, so
//! nothing here touches the developer's real run records. Golden tests in
//! `tests/scripts/` compare stdout byte for byte.

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

    fn home(&self) -> PathBuf {
        self.path().join("home")
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
            .env("CIGSCRIPT_HOME", self.home())
            .env("NO_COLOR", "1")
            .output()
            .expect("run cig")
    }

    fn run_ok(&self, args: &[&str]) -> String {
        let out = self.cig(args);
        assert!(
            out.status.success(),
            "cig {args:?} failed with {:?}\nstdout:\n{}\nstderr:\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    fn run_err(&self, args: &[&str], code: i32) -> String {
        let out = self.cig(args);
        assert_eq!(
            out.status.code(),
            Some(code),
            "cig {args:?}: expected exit {code}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stderr).to_string()
    }

    fn runs(&self) -> Vec<serde_json::Value> {
        let out = self.run_ok(&["--json", "runs"]);
        serde_json::from_str(&out).expect("runs json")
    }
}

// ----- golden scripts --------------------------------------------------------

#[test]
fn golden_scripts_match_expected_output() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/scripts");
    let mut count = 0;
    let mut entries: Vec<_> = fs::read_dir(&dir).unwrap().flatten().collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "cig") {
            continue;
        }
        let expected_path = path.with_extension("out");
        let expected = fs::read_to_string(&expected_path)
            .unwrap_or_else(|_| panic!("missing {}", expected_path.display()));
        let sb = Sandbox::new();
        let out = sb.cig(&["run", path.to_str().unwrap()]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{} failed:\n{}",
            path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(stdout, expected, "output mismatch for {}", path.display());
        count += 1;
    }
    assert!(count >= 5, "expected golden scripts, found {count}");
}

#[test]
fn examples_pass_check() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let sb = Sandbox::new();
    for entry in fs::read_dir(&dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "cig") {
            sb.run_ok(&["check", path.to_str().unwrap()]);
        }
    }
}

// ----- burn kernel ------------------------------------------------------------

#[test]
fn effect_outside_burn_is_refused_before_running() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "exhale \"start\"\nfs.write_text(\"x.txt\", \"no\")\n",
    );
    let err = sb.run_err(&["run", "s.cig"], 2);
    assert!(err.contains("Don't see any cigarettes."), "{err}");
    assert!(err.contains("must be inside a burn block"), "{err}");
    assert!(!sb.exists("x.txt"));
}

#[test]
fn effect_outside_burn_at_runtime_is_refused() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "pull write_it() {\n  fs.write_text(\"x.txt\", \"no\")\n}\nwrite_it()\n",
    );
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(err.contains("error[E701 burn]"), "{err}");
    assert!(!sb.exists("x.txt"));
    // The same function is fine when called from inside a burn.
    sb.write(
        "ok.cig",
        "pull write_it() {\n  fs.write_text(\"x.txt\", \"yes\")\n}\nburn {\n  write_it()\n}\n",
    );
    sb.run_ok(&["run", "ok.cig"]);
    assert_eq!(sb.read("x.txt"), "yes");
}

#[test]
fn dry_run_changes_nothing_and_prints_a_plan() {
    let sb = Sandbox::new();
    sb.write("keep.txt", "original");
    sb.write(
        "s.cig",
        "burn {\n  fs.write_text(\"keep.txt\", \"changed\")\n  fs.mkdir(\"d\")\n  fs.write_text(\"d/new.txt\", \"n\")\n  fs.mv(\"d/new.txt\", \"d/moved.txt\")\n  stick r = proc.run(\"echo\", [\"hi\"])\n  exhale r.simulated, r.out\n}\n",
    );
    let out = sb.cig(&["run", "--dry-run", "s.cig"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("dry-run plan: 5 ops"), "{stderr}");
    assert!(stderr.contains("write keep.txt"), "{stderr}");
    assert!(stderr.contains("irreversible  run echo hi"), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "true \n");
    assert_eq!(sb.read("keep.txt"), "original");
    assert!(!sb.exists("d"));
    assert!(sb.runs().is_empty(), "dry-run must not leave a run record");
}

#[test]
fn failed_run_rolls_back_every_journaled_op() {
    let sb = Sandbox::new();
    sb.write("keep.txt", "original");
    sb.write("gone.txt", "to be deleted");
    fs::create_dir_all(sb.path().join("tree/sub")).unwrap();
    sb.write("tree/sub/deep.txt", "deep");
    sb.write(
        "s.cig",
        concat!(
            "burn {\n",
            "  fs.write_text(\"keep.txt\", \"changed\")\n",
            "  fs.append_text(\"keep.txt\", \"!\")\n",
            "  fs.rm(\"gone.txt\")\n",
            "  fs.rm(\"tree\")\n",
            "  fs.mkdir(\"made/nested\")\n",
            "  fs.write_text(\"made/nested/f.txt\", \"x\")\n",
            "  fs.cp(\"keep.txt\", \"copy.txt\")\n",
            "  fs.mv(\"copy.txt\", \"moved.txt\")\n",
            "}\n",
            "cough \"abort\"\n"
        ),
    );
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(err.contains("error[E600 cough]: abort"), "{err}");
    assert!(err.contains("unburn: rolling back"), "{err}");
    assert!(!err.contains("failed"), "{err}");
    assert_eq!(sb.read("keep.txt"), "original");
    assert_eq!(sb.read("gone.txt"), "to be deleted");
    assert_eq!(sb.read("tree/sub/deep.txt"), "deep");
    assert!(!sb.exists("made"));
    assert!(!sb.exists("copy.txt"));
    assert!(!sb.exists("moved.txt"));
    let runs = sb.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["status"], "rolled_back");
    assert_eq!(runs[0]["burns"], 8);
}

#[test]
fn unburn_restores_a_successful_run_later() {
    let sb = Sandbox::new();
    sb.write("a.txt", "one");
    sb.write("s.cig", "burn {\n  fs.write_text(\"a.txt\", \"two\")\n  fs.write_text(\"b.txt\", \"new\")\n  fs.mv(\"a.txt\", \"c.txt\")\n}\n");
    sb.run_ok(&["run", "s.cig"]);
    assert_eq!(sb.read("c.txt"), "two");
    assert!(sb.exists("b.txt"));
    let runs = sb.runs();
    let id = runs[0]["id"].as_str().unwrap().to_string();
    assert_eq!(runs[0]["status"], "ok");
    assert_eq!(runs[0]["burns"], 3);

    // A unique prefix is enough, and --dry-run touches nothing.
    let prefix = &id[..id.len() - 2];
    sb.run_ok(&["unburn", prefix, "--dry-run"]);
    assert_eq!(sb.read("c.txt"), "two");

    sb.run_ok(&["unburn", prefix]);
    assert_eq!(sb.read("a.txt"), "one");
    assert!(!sb.exists("b.txt"));
    assert!(!sb.exists("c.txt"));
    assert_eq!(sb.runs()[0]["status"], "unburned");
}

#[test]
fn unlit_burns_never_execute_but_record_intents() {
    let sb = Sandbox::new();
    sb.write("precious.txt", "keep me");
    sb.write(
        "s.cig",
        "burn unlit {\n  fs.rm(\"precious.txt\")\n  proc.run(\"rm\", [\"-rf\", \"/\"])\n}\nexhale \"still here:\", fs.exists(\"precious.txt\")\n",
    );
    let out = sb.cig(&["run", "s.cig"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "still here: true\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unlit intents: 2 ops"), "{stderr}");
    let runs = sb.runs();
    let dir = sb.home().join("runs").join(runs[0]["id"].as_str().unwrap());
    let intents = fs::read_to_string(dir.join("intents.jsonl")).unwrap();
    assert_eq!(intents.lines().count(), 2);
    assert!(intents.contains("\"class\":\"unlit\""));
}

#[test]
fn no_rollback_flag_leaves_burns_in_place() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "burn {\n  fs.write_text(\"x.txt\", \"kept\")\n}\ncough \"boom\"\n",
    );
    sb.run_err(&["run", "--no-rollback", "s.cig"], 1);
    assert_eq!(sb.read("x.txt"), "kept");
    assert_eq!(sb.runs()[0]["status"], "error");
}

#[test]
fn proc_run_captures_output_and_honours_check_and_timeout() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        concat!(
            "burn {\n",
            "  stick r = proc.run(\"sh\", [\"-c\", \"echo out; echo err 1>&2; exit 3\"])\n",
            "  exhale r.code, r.out.trim(), r.err.trim()\n",
            "  try {\n",
            "    proc.run(\"sh\", [\"-c\", \"exit 7\"], {check: true})\n",
            "  } ashtray e {\n",
            "    exhale \"caught\", e.code\n",
            "  }\n",
            "  try {\n",
            "    proc.run(\"sleep\", [\"5\"], {timeout_ms: 200})\n",
            "  } ashtray e {\n",
            "    exhale e.message.contains(\"killed\")\n",
            "  }\n",
            "}\n"
        ),
    );
    let out = sb.run_ok(&["run", "s.cig"]);
    assert_eq!(out, "3 out err\ncaught 7\ntrue\n");
}

// ----- diagnostics and modes -----------------------------------------------------

#[test]
fn check_reports_errors_and_warnings_with_json() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "pull f() {\n  fs.rm(\"x\")\n}\nroll a = 1\nexhale b\nstick c = 1\nc = 2\n",
    );
    let out = sb.cig(&["--json", "check", "s.cig"]);
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["errors"].as_array().unwrap().len(), 2);
    assert_eq!(v["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(v["errors"][0]["message"], "unknown name `b`");
    assert_eq!(v["errors"][0]["hint"], "did you mean `a`?");
}

#[test]
fn syntax_errors_exit_2_and_point_at_the_problem() {
    let sb = Sandbox::new();
    sb.write("s.cig", "roll x = 1\nif x {\n  exhale x\n");
    let err = sb.run_err(&["run", "s.cig"], 2);
    assert!(err.contains("never closed"), "{err}");
    assert!(err.contains("s.cig:2:6"), "{err}");
    sb.write("t.cig", "exhael 1\n");
    let err = sb.run_err(&["check", "t.cig"], 2);
    assert!(err.contains("did you mean `exhale`?"), "{err}");
}

#[test]
fn runtime_errors_exit_1_and_are_catchable() {
    let sb = Sandbox::new();
    sb.write("s.cig", "roll m = {a: 1}\nexhale m.b\n");
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(err.contains("map has no key `b`"), "{err}");
    sb.write("t.cig", "try {\n  roll m = {a: 1}\n  exhale m.b\n} ashtray e {\n  exhale e.kind, e.line, e.message\n}\nexit(4)\n");
    let out = sb.cig(&["run", "t.cig"]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "runtime 3 map has no key `b`\n"
    );
}

#[test]
fn eval_prints_values_and_json() {
    let sb = Sandbox::new();
    assert_eq!(
        sb.run_ok(&["eval", "[1, 2, 3].map(pack(x) => x * x)"]),
        "[1, 4, 9]\n"
    );
    assert_eq!(sb.run_ok(&["eval", "roll a = 2; a * 21"]), "42\n");
    assert_eq!(
        sb.run_ok(&["--json", "eval", "{a: 1.5, b: \"x\"}"]),
        "{\"a\":1.5,\"b\":\"x\"}\n"
    );
    let err = sb.run_err(&["eval", "fs.rm(\"x\")"], 1);
    assert!(err.contains("error[E701 burn]"), "{err}");
}

#[test]
fn args_reach_the_script() {
    let sb = Sandbox::new();
    sb.write("s.cig", "exhale args.len(), args.join(\"|\")\n");
    assert_eq!(sb.run_ok(&["run", "s.cig", "--", "a", "b c"]), "2 a|b c\n");
}

#[test]
fn language_and_doctor_work_in_json() {
    let sb = Sandbox::new();
    let v: serde_json::Value = serde_json::from_str(&sb.run_ok(&["--json", "language"])).unwrap();
    assert!(v["library"].as_array().unwrap().len() > 10);
    let out = sb.cig(&["--json", "doctor"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["state_dir_writable"], true);
    assert_eq!(v["runs"], 0);
}

#[test]
fn runs_prune_keeps_the_newest() {
    let sb = Sandbox::new();
    sb.write("s.cig", "exhale 1\n");
    for _ in 0..4 {
        sb.run_ok(&["run", "s.cig"]);
    }
    assert_eq!(sb.runs().len(), 4);
    sb.run_ok(&["runs", "--prune", "2"]);
    assert_eq!(sb.runs().len(), 2);
}

#[test]
fn deep_recursion_is_an_error_not_a_crash() {
    let sb = Sandbox::new();
    sb.write("s.cig", "pull down(n) {\n  snuff down(n + 1)\n}\ndown(0)\n");
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(err.contains("call depth exceeded"), "{err}");
}

// ----- 2.1: chains, error codes, config, crash reports, updates -----------------------

#[test]
fn chains_list_and_light_from_the_cli() {
    let sb = Sandbox::new();
    sb.write(
        "tasks.cig",
        concat!(
            "roll log = []\n",
            "stick prepare = pack() {\n  burn { fs.write_text(\"out.txt\", \"prepared\") }\n  log.push(\"prepare\")\n}\n",
            "stick verify = pack() {\n  log.push(\"verify\")\n  snuff fs.read_text(\"out.txt\")\n}\n",
            "stick explode = pack() { cough \"kaboom\" }\n",
            "chain deploy { prepare, verify }\n",
            "chain broken { prepare, explode, verify }\n",
            "exhale \"top level ran\"\n"
        ),
    );
    let out = sb.run_ok(&["chains", "tasks.cig"]);
    assert!(
        out.contains("deploy") && out.contains("1. prepare") && out.contains("broken"),
        "{out}"
    );
    let v: serde_json::Value =
        serde_json::from_str(&sb.run_ok(&["--json", "chains", "tasks.cig"])).unwrap();
    assert_eq!(
        v["chains"][0]["steps"],
        serde_json::json!(["prepare", "verify"])
    );

    let out = sb.cig(&["light", "tasks.cig", "deploy"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("chain deploy: step 2/2 verify ok"),
        "{stderr}"
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "top level ran\n");
    assert_eq!(sb.read("out.txt"), "prepared");

    // A failing chain fails the run and rolls back what it burned.
    fs::remove_file(sb.path().join("out.txt")).unwrap();
    let err = sb.run_err(&["light", "tasks.cig", "broken"], 1);
    assert!(err.contains("error[E602 cough]"), "{err}");
    assert!(err.contains("failed at step 2 (explode)"), "{err}");
    assert!(
        !sb.exists("out.txt"),
        "the burn from step 1 must be rolled back"
    );

    let err = sb.run_err(&["light", "tasks.cig", "nope"], 1);
    assert!(
        err.contains("E804") && err.contains("chains here: deploy, broken"),
        "{err}"
    );

    // Dry-run lights the chain without burning.
    let out = sb.cig(&["light", "--dry-run", "tasks.cig", "deploy"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("dry-run plan: 1 op"), "{stderr}");
    assert!(!sb.exists("out.txt"));
}

#[test]
fn error_codes_render_and_explain() {
    let sb = Sandbox::new();
    let err = sb.run_err(&["eval", "[1][9]"], 1);
    assert!(
        err.contains("error[E504 runtime]: index 9 is out of range"),
        "{err}"
    );
    assert!(err.contains("= explain: cig explain E504"), "{err}");
    let out = sb.run_ok(&["explain", "E504"]);
    assert!(out.contains("index out of range"), "{out}");
    let all: serde_json::Value = serde_json::from_str(&sb.run_ok(&["--json", "explain"])).unwrap();
    assert!(all.as_array().unwrap().len() > 40);
    sb.run_err(&["explain", "E9999"], 3);
    let v: serde_json::Value =
        serde_json::from_slice(&sb.cig(&["--json", "check", "nope.cig"]).stdout)
            .unwrap_or_default();
    let _ = v;
    sb.write("s.cig", "roll a = 1\nexhale b\n");
    let v: serde_json::Value =
        serde_json::from_slice(&sb.cig(&["--json", "check", "s.cig"]).stdout).unwrap();
    assert_eq!(v["errors"][0]["code"], "E301");
}

#[test]
fn config_reads_validates_and_persists() {
    let sb = Sandbox::new();
    assert_eq!(sb.run_ok(&["config", "crash_reports"]), "ask\n");
    sb.run_ok(&["config", "crash_reports", "never"]);
    assert_eq!(sb.run_ok(&["config", "crash_reports"]), "never\n");
    assert!(sb.read("home/config").contains("crash_reports = never"));
    sb.run_err(&["config", "crash_reports", "sometimes"], 3);
    sb.run_err(&["config", "nonsense", "1"], 3);
    sb.run_ok(&["config", "crash_reports", "--unset"]);
    assert_eq!(sb.run_ok(&["config", "crash_reports"]), "ask\n");
    let v: serde_json::Value = serde_json::from_str(&sb.run_ok(&["--json", "config"])).unwrap();
    assert_eq!(v["config"]["issues_repo"]["value"], "otmof-ops/CigScript");
}

#[test]
fn a_panic_writes_a_redacted_crash_report_and_never_sends_without_consent() {
    let sb = Sandbox::new();
    sb.write("s.cig", "exhale 1\n");
    let out = Command::new(env!("CARGO_BIN_EXE_cig"))
        .args(["run", "s.cig", "--token=supersecret"])
        .current_dir(sb.path())
        .env("CIGSCRIPT_HOME", sb.home())
        .env("NO_COLOR", "1")
        .env("CIG_INTERNAL_PANIC", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(70));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error[E901 internal]: cig crashed"),
        "{stderr}"
    );
    assert!(stderr.contains("cig crash send"), "{stderr}");
    let reports: serde_json::Value =
        serde_json::from_str(&sb.run_ok(&["--json", "crash", "list"])).unwrap();
    assert_eq!(reports.as_array().unwrap().len(), 1);
    let r = &reports[0];
    assert_eq!(r["sent"], serde_json::Value::Null);
    let cmd = r["command"].as_array().unwrap();
    assert!(cmd.iter().any(|a| a == "--token=<redacted>"), "{cmd:?}");
    assert!(!serde_json::to_string(r).unwrap().contains("supersecret"));
    assert!(r["panic_location"].as_str().unwrap().starts_with("main.rs"));
    let id = r["id"].as_str().unwrap().to_string();
    let shown = sb.run_ok(&["crash", "show", &id]);
    assert!(shown.contains("\"fingerprint\""));
    // never mode: no prompt, no send, report stays local.
    sb.run_ok(&["config", "crash_reports", "never"]);
    let out = Command::new(env!("CARGO_BIN_EXE_cig"))
        .args(["run", "s.cig"])
        .current_dir(sb.path())
        .env("CIGSCRIPT_HOME", sb.home())
        .env("CIG_INTERNAL_PANIC", "1")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("crash_reports is \"never\""));
    sb.run_ok(&["crash", "delete", "--all"]);
    assert_eq!(sb.run_ok(&["--json", "crash", "list"]).trim(), "[]");
}

#[test]
fn update_checks_verifies_and_installs_from_a_local_manifest() {
    let sb = Sandbox::new();
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_cig"));
    // A private, user-owned install dir (0755) holding a copy of the binary.
    let inst = sb.path().join("inst");
    fs::create_dir_all(&inst).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&inst, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::copy(&bin, inst.join("cig")).unwrap();
    // The "release": the same binary under a huge version, with its checksum.
    let asset = sb.path().join("cig-v99.0.0-asset");
    fs::copy(&bin, &asset).unwrap();
    let sum = cigscript::burn::journal::sha256_file(&asset).unwrap();
    sb.write("asset.sha256", &format!("{sum}  cig\n"));
    let name = cigscript::update::asset_name(&cigscript::update::Version(99, 0, 0));
    let manifest = serde_json::json!([{
        "tag_name": "v99.0.0", "prerelease": false, "draft": false, "html_url": "https://example.invalid/r", "published_at": "",
        "assets": [
            {"name": name, "browser_download_url": format!("file://{}", asset.display())},
            {"name": format!("{name}.sha256"), "browser_download_url": format!("file://{}", sb.path().join("asset.sha256").display())}
        ]
    }]);
    sb.write("manifest.json", &manifest.to_string());
    let run = |args: &[&str]| {
        Command::new(inst.join("cig"))
            .args(args)
            .current_dir(sb.path())
            .env("CIGSCRIPT_HOME", sb.home())
            .env("NO_COLOR", "1")
            .env("CIG_UPDATE_MANIFEST", sb.path().join("manifest.json"))
            .output()
            .unwrap()
    };
    let out = run(&["--json", "update", "--check"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["latest"], "99.0.0");
    assert_eq!(v["newer"], true);
    assert!(sb.exists("home/update-check.json"));
    // doctor reports the cached newer version
    let d = String::from_utf8_lossy(&run(&["doctor"]).stdout).to_string();
    assert!(d.contains("newer than this build"), "{d}");

    let out = run(&["update"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("verified: sha256 matches") && stderr.contains("updated:"),
        "{stderr}"
    );
    assert!(!inst.join(".cig.new").exists());

    // A corrupted checksum is refused and nothing is installed.
    sb.write("asset.sha256", "deadbeef  cig\n");
    let out = run(&["update"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&out.stderr).contains("checksum mismatch"));
}

#[test]
fn installer_script_parses() {
    let out = Command::new("sh")
        .args(["-n", concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh")])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn lab_examples_run_end_to_end() {
    let ex = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
    let sb = Sandbox::new();

    // measurements: a citable report with the input's hash
    sb.write("readings.csv", "series,value\ntemperature,21.4\ntemperature,21.9\ntemperature,22.1\npressure,101.2\npressure,101.5\n");
    let out = sb.run_ok(&[
        "run",
        ex.join("lab-measurements.cig").to_str().unwrap(),
        "--",
        "readings.csv",
        "report.json",
    ]);
    assert!(out.contains("temperature") && out.contains("n=3"), "{out}");
    let report: serde_json::Value = serde_json::from_str(&sb.read("report.json")).unwrap();
    assert_eq!(report["series"]["temperature"]["n"], 3);
    assert_eq!(report["series"]["pressure"]["n"], 2);
    assert_eq!(report["input_sha256"].as_str().unwrap().len(), 64);

    // manifest: write, verify clean, detect drift
    fs::create_dir_all(sb.path().join("data/sub")).unwrap();
    sb.write("data/a.txt", "alpha");
    sb.write("data/sub/b.txt", "beta");
    let manifest = ex.join("lab-manifest.cig");
    sb.run_ok(&["run", manifest.to_str().unwrap(), "--", "data"]);
    assert!(sb.exists("data/MANIFEST.json"));
    let out = sb.run_ok(&["run", manifest.to_str().unwrap(), "--", "data", "verify"]);
    assert!(out.contains("0 changed, 0 missing, 0 new"), "{out}");
    sb.write("data/a.txt", "ALPHA");
    fs::remove_file(sb.path().join("data/sub/b.txt")).unwrap();
    sb.write("data/c.txt", "new");
    let err_out = sb.cig(&["run", manifest.to_str().unwrap(), "--", "data", "verify"]);
    assert_eq!(err_out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&err_out.stdout);
    assert!(text.contains("1 changed, 1 missing, 1 new"), "{text}");

    // pipeline: light it for real, then dry-run it
    let pipeline = ex.join("lab-pipeline.cig");
    let out = sb.run_ok(&["light", pipeline.to_str().unwrap(), "analysis"]);
    assert!(out.contains("| temperature | 3 |"), "{out}");
    assert!(out.contains("1 out-of-range row(s) dropped"), "{out}");
    let report: serde_json::Value = serde_json::from_str(&sb.read("work/report.json")).unwrap();
    assert_eq!(report["result"]["stats"]["temperature"]["n"], 3);
    assert_eq!(report["result"]["dropped"], 1);
    fs::remove_dir_all(sb.path().join("work")).unwrap();
    let out = sb.cig(&["light", "--dry-run", pipeline.to_str().unwrap(), "analysis"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("dry-run plan: 5 ops"), "{stderr}");
    assert!(!sb.exists("work"));
}
