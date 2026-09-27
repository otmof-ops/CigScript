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

    fn cig_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cig"))
            .args(args)
            .current_dir(self.path())
            .env("CIGSCRIPT_HOME", self.home())
            .env("NO_COLOR", "1")
            .envs(envs.iter().copied())
            .output()
            .expect("run cig")
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

// ----- the Hammer update: verified bugs and the rollback contract -----------

#[test]
fn interrupted_run_is_recognised_counted_from_the_journal_and_unburnable() {
    let sb = Sandbox::new();
    sb.write("a.txt", "one");
    sb.write(
        "s.cig",
        "burn {\n  fs.write_text(\"a.txt\", \"two\")\n  fs.write_text(\"b.txt\", \"new\")\n}\n",
    );
    sb.run_ok(&["run", "s.cig"]);
    let runs = sb.runs();
    let id = runs[0]["id"].as_str().unwrap().to_string();
    // Forge what a process killed mid-burn leaves behind: status still
    // `running`, totals never written, a pid that is gone.
    let rec_path = sb.home().join("runs").join(&id).join("run.json");
    let mut rec: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&rec_path).unwrap()).unwrap();
    rec["status"] = serde_json::json!("running");
    rec["burns"] = serde_json::json!(0);
    rec["finished"] = serde_json::Value::Null;
    rec["pid"] = serde_json::json!(2_147_483_647u32);
    fs::write(&rec_path, serde_json::to_string(&rec).unwrap()).unwrap();

    let runs = sb.runs();
    assert_eq!(runs[0]["status"], "interrupted", "{runs:?}");
    assert_eq!(runs[0]["burns"], 2, "counted from the journal: {runs:?}");

    let doctor: serde_json::Value =
        serde_json::from_slice(&sb.cig(&["--json", "doctor"]).stdout).unwrap();
    assert_eq!(doctor["interrupted_runs"][0], id, "{doctor}");
    let text = String::from_utf8_lossy(&sb.cig(&["doctor"]).stdout).to_string();
    assert!(
        text.contains("interrupted mid-burn") && text.contains("E706"),
        "{text}"
    );

    sb.run_ok(&["unburn", &id]);
    assert_eq!(sb.read("a.txt"), "one");
    assert!(!sb.exists("b.txt"));
    assert_eq!(sb.runs()[0]["status"], "unburned");
}

#[test]
fn unburn_refuses_a_second_time_without_force() {
    let sb = Sandbox::new();
    sb.write("a.txt", "one");
    sb.write("s.cig", "burn {\n  fs.write_text(\"a.txt\", \"two\")\n}\n");
    sb.run_ok(&["run", "s.cig"]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    sb.run_ok(&["unburn", &id]);
    assert_eq!(sb.read("a.txt"), "one");
    sb.write("a.txt", "three");
    let err = sb.run_err(&["unburn", &id], 1);
    assert!(
        err.contains("error[E705 burn]:") && err.contains("already rolled back"),
        "{err}"
    );
    assert_eq!(
        sb.read("a.txt"),
        "three",
        "a refused unburn touches nothing"
    );
    // --dry-run is always allowed.
    sb.run_ok(&["unburn", &id, "--dry-run"]);
    assert_eq!(sb.read("a.txt"), "three");
    sb.run_ok(&["unburn", &id, "--force"]);
    assert_eq!(sb.read("a.txt"), "one");
}

#[test]
fn unburn_checks_every_snapshot_before_touching_anything() {
    let sb = Sandbox::new();
    sb.write("m1.txt", "old1");
    sb.write("m2.txt", "old2");
    sb.write(
        "s.cig",
        "burn {\n  fs.write_text(\"m1.txt\", \"new1\")\n  fs.write_text(\"m2.txt\", \"new2\")\n}\n",
    );
    sb.run_ok(&["run", "s.cig"]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    let snaps = sb.home().join("runs").join(&id).join("snapshots");
    let mut removed = 0;
    for e in fs::read_dir(&snaps).unwrap().flatten() {
        if e.file_name().to_string_lossy().contains("m2.txt") {
            fs::remove_file(e.path()).unwrap();
            removed += 1;
        }
    }
    assert_eq!(removed, 1, "one snapshot for m2.txt");
    let err = sb.run_err(&["unburn", &id], 1);
    assert!(
        err.contains("error[E703 burn]:") && err.contains("snapshot missing for"),
        "{err}"
    );
    assert_eq!(
        sb.read("m1.txt"),
        "new1",
        "nothing restored before the check passes"
    );
    assert_eq!(sb.read("m2.txt"), "new2");
    let plan = String::from_utf8_lossy(&sb.cig(&["unburn", &id, "--dry-run"]).stdout).to_string();
    assert!(plan.contains("problem:") && plan.contains("E703"), "{plan}");
    // --force restores what it can and reports the rest.
    let out = sb.cig(&["unburn", &id, "--force"]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        text.contains("restored") && text.contains("failed"),
        "{text}"
    );
    assert_eq!(sb.read("m1.txt"), "old1");
    assert_eq!(sb.read("m2.txt"), "new2");
}

#[test]
fn i64_min_is_a_literal_and_the_overflow_next_to_it_is_a_diagnostic() {
    let sb = Sandbox::new();
    let out = sb.run_ok(&["eval", "--", "-9223372036854775808"]);
    assert_eq!(out.trim(), "-9223372036854775808");
    let out = sb.run_ok(&[
        "eval",
        "--",
        "-9223372036854775808 == -9223372036854775807 - 1",
    ]);
    assert_eq!(out.trim(), "true");
    let err = sb.run_err(&["eval", "--", "9223372036854775808"], 2);
    assert!(
        err.contains("E104") && err.contains("does not fit"),
        "{err}"
    );
    let err = sb.run_err(&["eval", "--", "9223372036854775809"], 2);
    assert!(err.contains("E104"), "{err}");
    let err = sb.run_err(&["eval", "--", "-9223372036854775808 - 1"], 1);
    assert!(err.contains("overflow"), "{err}");
    let err = sb.run_err(&["eval", "--", "0 - -9223372036854775808"], 1);
    assert!(err.contains("overflow"), "{err}");
}

#[test]
fn chain_failures_point_at_the_step_not_the_light_call() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        concat!(
            "roll boom = pack(x) {\n",
            "  cough \"kaboom\"\n",
            "}\n",
            "chain deploy {\n",
            "  pack() => 1\n",
            "  boom\n",
            "}\n",
            "\n",
            "light(deploy)\n"
        ),
    );
    // From inside the script: the step's line, plus where the cough was raised.
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(
        err.contains(
            "error[E602 cough]: chain `deploy` failed at step 2 (boom): kaboom (raised at line 2)"
        ),
        "{err}"
    );
    assert!(
        err.contains("s.cig:6:"),
        "points at the step, not line 9: {err}"
    );
    assert!(!err.contains(":9:"), "{err}");
    // From the CLI: there is no light() call at all, and still no 0:0.
    let err = sb.run_err(&["light", "s.cig", "deploy"], 1);
    assert!(err.contains("failed at step 2 (boom)"), "{err}");
    assert!(err.contains("s.cig:6:"), "{err}");
    assert!(!err.contains(":0:0"), "{err}");
}

#[test]
fn allocation_ceiling_is_a_diagnostic_not_an_abort() {
    let sb = Sandbox::new();
    let err = sb.run_err(&["eval", "\"x\".repeat(9223372036854775807)"], 1);
    assert!(
        err.contains("error[E514 runtime]:") && err.contains("over the ceiling"),
        "{err}"
    );
    let err = sb.run_err(&["eval", "\"ab\" * 9223372036854775807"], 1);
    assert!(err.contains("E514"), "{err}");
    let err = sb.run_err(&["eval", "\"x\".pad_left(9223372036854775807)"], 1);
    assert!(err.contains("E514"), "{err}");
    // The ceiling is configurable, and a value under it is built normally.
    let out = sb.cig_env(
        &["eval", "\"x\".repeat(2000).len()"],
        &[("CIG_MAX_ALLOC", "1024")],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("E514"));
    let out = sb.cig_env(
        &["eval", "\"x\".repeat(10).len()"],
        &[("CIG_MAX_ALLOC", "1024")],
    );
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "10");
    // Reading a file over the ceiling is refused before it is read.
    sb.write("big.txt", &"y".repeat(4096));
    let out = sb.cig_env(
        &["eval", "fs.read_text(\"big.txt\").len()"],
        &[("CIG_MAX_ALLOC", "1024")],
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("E514"));
    sb.run_ok(&["explain", "E514"]);
}

#[cfg(unix)]
#[test]
fn symlinks_roll_back_as_symlinks() {
    use std::os::unix::fs::symlink;
    let sb = Sandbox::new();
    sb.write("target.txt", "t");
    symlink("target.txt", sb.path().join("live.lnk")).unwrap();
    symlink("nowhere/at/all", sb.path().join("dangling.lnk")).unwrap();
    fs::create_dir_all(sb.path().join("tree")).unwrap();
    symlink("../target.txt", sb.path().join("tree/inner.lnk")).unwrap();
    sb.write(
        "s.cig",
        concat!(
            "burn {\n",
            "  fs.rm(\"live.lnk\")\n",
            "  fs.rm(\"dangling.lnk\")\n",
            "  fs.rm(\"tree\")\n",
            "  fs.write_text(\"target.txt\", \"changed\")\n",
            "}\n",
            "cough \"abort\"\n"
        ),
    );
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(
        err.contains("unburn: rolling back") && !err.contains("failed"),
        "{err}"
    );
    let is_link = |p: &str| {
        fs::symlink_metadata(sb.path().join(p))
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    };
    assert!(
        is_link("live.lnk"),
        "live.lnk came back as a real file or not at all"
    );
    assert_eq!(
        fs::read_link(sb.path().join("live.lnk")).unwrap(),
        Path::new("target.txt")
    );
    assert!(is_link("dangling.lnk"));
    assert_eq!(
        fs::read_link(sb.path().join("dangling.lnk")).unwrap(),
        Path::new("nowhere/at/all")
    );
    assert!(
        is_link("tree/inner.lnk"),
        "links inside a restored directory stay links"
    );
    assert_eq!(sb.read("target.txt"), "t");

    // Writing through a link changes the target; rollback restores the
    // target's bytes and leaves the link a link.
    sb.write(
        "s2.cig",
        "burn {\n  fs.write_text(\"live.lnk\", \"through\")\n}\ncough \"abort\"\n",
    );
    sb.run_err(&["run", "s2.cig"], 1);
    assert!(is_link("live.lnk"));
    assert_eq!(sb.read("target.txt"), "t");
    // ... and a write through a dangling link that would create its target
    // removes that target again.
    symlink("ghost.txt", sb.path().join("ghost.lnk")).unwrap();
    sb.write(
        "s3.cig",
        "burn {\n  fs.write_text(\"ghost.lnk\", \"boo\")\n}\ncough \"abort\"\n",
    );
    sb.run_err(&["run", "s3.cig"], 1);
    assert!(is_link("ghost.lnk"));
    assert!(!sb.exists("ghost.txt"));
    // fs.exists sees a dangling link, so `if fs.exists(p) { fs.rm(p) }` works.
    assert_eq!(
        sb.run_ok(&["eval", "fs.exists(\"dangling.lnk\")"]).trim(),
        "true"
    );
}

// ----- the Hammer update: the error registry -------------------------------

#[test]
fn explain_pages_match_golden_text() {
    let sb = Sandbox::new();
    let nodes: Vec<serde_json::Value> =
        serde_json::from_str(&sb.run_ok(&["--json", "explain"])).unwrap();
    assert!(nodes.len() > 50);
    let mut text = sb.run_ok(&["explain"]);
    for n in &nodes {
        let code = n["code"].as_str().unwrap();
        text.push_str(&format!("\n======== {code} ========\n"));
        text.push_str(&sb.run_ok(&["explain", code]));
    }
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/explain.txt");
    if std::env::var_os("CIG_UPDATE_GOLDEN").is_some() {
        fs::create_dir_all(golden.parent().unwrap()).unwrap();
        fs::write(&golden, &text).unwrap();
    }
    let expected = fs::read_to_string(&golden)
        .unwrap_or_else(|_| panic!("missing {}; run with CIG_UPDATE_GOLDEN=1", golden.display()));
    assert_eq!(
        text, expected,
        "explain wording changed; if that is intended, rerun with CIG_UPDATE_GOLDEN=1"
    );
}

#[test]
fn registry_nodes_and_schema_agree_with_json_diagnostics() {
    let sb = Sandbox::new();
    let schema: serde_json::Value =
        serde_json::from_str(&sb.run_ok(&["explain", "--schema"])).unwrap();
    let codes: Vec<&str> = schema["properties"]["code"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let kinds: Vec<&str> = schema["properties"]["kind"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let props: Vec<&str> = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let nodes: Vec<serde_json::Value> =
        serde_json::from_str(&sb.run_ok(&["--json", "explain"])).unwrap();
    assert_eq!(codes.len(), nodes.len());
    for n in &nodes {
        assert!(codes.contains(&n["code"].as_str().unwrap()));
        assert!(kinds.contains(&n["kind"].as_str().unwrap()), "{n}");
        assert!(
            n["family"].is_string() && n["no"]["hear"].is_string() && n["no"]["manual"].is_string(),
            "{n}"
        );
        assert!(!n["causes"].as_array().unwrap().is_empty(), "{n}");
    }
    let one: serde_json::Value =
        serde_json::from_str(&sb.run_ok(&["--json", "explain", "E508"])).unwrap();
    assert_eq!(one["no"]["id"], "depends");
    assert_eq!(one["causes"][0]["probe"], "path-exists");
    // A real diagnostic validates against the schema, structurally.
    sb.write("s.cig", "roll a = 1\nexhale b\nstick c = 1\nc = 2\n");
    let rep: serde_json::Value =
        serde_json::from_slice(&sb.cig(&["--json", "check", "s.cig"]).stdout).unwrap();
    let diags = rep["errors"].as_array().unwrap();
    assert_eq!(diags.len(), 2);
    for d in diags {
        for k in d.as_object().unwrap().keys() {
            assert!(
                props.contains(&k.as_str()),
                "field `{k}` is not in the schema: {d}"
            );
        }
        assert!(codes.contains(&d["code"].as_str().unwrap()), "{d}");
        assert!(kinds.contains(&d["kind"].as_str().unwrap()), "{d}");
        assert!(d["message"].is_string());
        assert!(d["line"].as_u64().unwrap() >= 1 && d["col"].as_u64().unwrap() >= 1);
    }
}

#[test]
fn check_can_deny_warnings_for_ci() {
    let sb = Sandbox::new();
    sb.write("w.cig", "pull f() {\n  fs.rm(\"x\")\n}\n");
    sb.run_ok(&["check", "w.cig"]);
    let err = sb.run_err(&["check", "w.cig", "--deny-warnings"], 2);
    assert!(
        err.contains("warning[E303 check]") && err.contains("--deny-warnings"),
        "{err}"
    );
    let out = sb.cig(&["--json", "check", "w.cig", "--deny-warnings"]);
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["warnings_denied"], true);
    assert_eq!(v["warnings"][0]["code"], "E303");
}

#[test]
fn usage_layer_errors_carry_their_codes() {
    let sb = Sandbox::new();
    let err = sb.run_err(&["run", "nope.cig"], 3);
    assert!(
        err.contains("error[E801 usage]: cannot read nope.cig") && err.contains("cig explain E801"),
        "{err}"
    );
    let err = sb.run_err(&["runs", "zzz"], 3);
    assert!(
        err.contains("error[E803 usage]: no run matches `zzz`"),
        "{err}"
    );
    let err = sb.run_err(&["unburn", "zzz"], 3);
    assert!(err.contains("error[E803 usage]"), "{err}");
    // No transport at all is E805, distinct from a transport that failed (E806).
    let out = sb.cig_env(&["update", "--check"], &[("PATH", "")]);
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        err.contains("error[E805 usage]") && err.contains("neither `gh` nor `curl`"),
        "{err}"
    );
}
