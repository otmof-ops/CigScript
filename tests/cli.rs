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

    let out = sb.cig(&["doctor", &id]);
    let text = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        text.contains("error[E706 burn]")
            && text.contains("= doctor: I think the process was killed"),
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

// ----- the Hammer update: doctor on the emit hook -----------------------------

/// Normalise the parts of doctor's output that legitimately vary.
fn normalise(text: &str, sandbox: &Path) -> String {
    let ms = regex::Regex::new(r"\(\d+ ms\)").unwrap();
    let ids = regex::Regex::new(r"\b\d{8}T\d{6}-[0-9a-f]{6}\b").unwrap();
    let t = text.replace(sandbox.to_str().unwrap(), "<sb>");
    let t = ms.replace_all(&t, "(N ms)").to_string();
    ids.replace_all(&t, "<run-id>").to_string()
}

/// (hallway phrase, manual name) for every kind of no, from the binary.
fn kinds_of_no() -> Vec<(String, String)> {
    let sb = Sandbox::new();
    let nodes: Vec<serde_json::Value> =
        serde_json::from_str(&sb.run_ok(&["--json", "explain"])).unwrap();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |v: &serde_json::Value| {
        let pair = (
            v["hear"].as_str().unwrap().to_string(),
            v["manual"].as_str().unwrap().to_string(),
        );
        if !out.contains(&pair) {
            out.push(pair);
        }
    };
    for n in &nodes {
        push(&n["no"]);
        for c in n["causes"].as_array().unwrap() {
            if let Some(id) = c["no"].as_str() {
                if let Some(m) = nodes.iter().find(|x| x["no"]["id"] == id) {
                    push(&m["no"]);
                }
            }
        }
    }
    out
}

/// What plain mode must equal: the themed text without the banner line and
/// with every hallway phrase replaced by the manual's name.
fn subtract_theme(themed: &str, nos: &[(String, String)]) -> String {
    let mut out = String::new();
    for line in themed.lines() {
        if line == "Don't see any cigarettes." {
            continue;
        }
        let mut l = line.to_string();
        for (hear, manual) in nos {
            l = l.replace(&format!(" ({hear})"), &format!(" ({manual})"));
        }
        out.push_str(&l);
        out.push('\n');
    }
    out
}

fn corpus_env(sb: &Sandbox) -> Vec<(String, String)> {
    vec![
        ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ("HOME".to_string(), sb.path().to_str().unwrap().to_string()),
        // A small budget so the runaway-loop case ends in milliseconds.
        ("CIG_MAX_STEPS".to_string(), "100000".to_string()),
    ]
}

fn run_corpus(sb: &Sandbox, name: &str, extra: &[(&str, &str)]) -> Output {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(name);
    fs::copy(&src, sb.path().join(name)).unwrap();
    let base = corpus_env(sb);
    let mut envs: Vec<(&str, &str)> = base.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    envs.extend_from_slice(extra);
    let mut args = corpus_args(&src);
    args.push(name.to_string());
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    sb.cig_env(&argv, &envs)
}

/// A first line `# cig: run --dry-run` chooses the command; `run` otherwise.
fn corpus_args(script: &Path) -> Vec<String> {
    fs::read_to_string(script)
        .unwrap()
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("# cig:"))
        .map(|rest| rest.split_whitespace().map(str::to_string).collect())
        .unwrap_or_else(|| vec!["run".to_string()])
}

#[test]
fn corpus_diagnoses_match_golden_output() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut scripts: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "cig"))
        .collect();
    scripts.sort();
    assert!(scripts.len() >= 8, "corpus has {} scripts", scripts.len());
    let update = std::env::var_os("CIG_UPDATE_GOLDEN").is_some();
    let nos = kinds_of_no();
    let mut failures = Vec::new();
    for path in scripts {
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        let sb = Sandbox::new();
        let out = run_corpus(&sb, &name, &[]);
        let text = normalise(&String::from_utf8_lossy(&out.stderr), sb.path());
        let expect = path.with_extension("expect");
        if update {
            fs::write(&expect, &text).unwrap();
        }
        let want = fs::read_to_string(&expect).unwrap_or_else(|_| {
            panic!("missing {}; run with CIG_UPDATE_GOLDEN=1", expect.display())
        });
        if text != want {
            failures.push(format!("{name}:\n--- expected\n{want}\n--- got\n{text}"));
        }
        // Plain mode: golden too, and a strict subtraction of the themed text.
        let base = corpus_env(&sb);
        let mut envs: Vec<(&str, &str)> =
            base.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        envs.push(("CIG_PLAIN", "1"));
        let mut plain_args = corpus_args(&path);
        plain_args.push(name.clone());
        let plain_argv: Vec<&str> = plain_args.iter().map(String::as_str).collect();
        let plain_out = sb.cig_env(&plain_argv, &envs);
        let plain = normalise(&String::from_utf8_lossy(&plain_out.stderr), sb.path());
        let plain_expect = path.with_extension("plain.expect");
        if update {
            fs::write(&plain_expect, &plain).unwrap();
        }
        let plain_want = fs::read_to_string(&plain_expect).unwrap_or_else(|_| {
            panic!(
                "missing {}; run with CIG_UPDATE_GOLDEN=1",
                plain_expect.display()
            )
        });
        if plain != plain_want {
            failures.push(format!(
                "{name} (plain):\n--- expected\n{plain_want}\n--- got\n{plain}"
            ));
        }
        let subtracted = subtract_theme(&text, &nos);
        if plain != subtracted {
            failures.push(format!("{name}: plain mode is not a strict subtraction of themed mode\n--- themed minus catchphrases\n{subtracted}\n--- plain\n{plain}"));
        }
    }
    assert!(
        failures.is_empty(),
        "doctor output changed; if intended, rerun with CIG_UPDATE_GOLDEN=1\n{}",
        failures.join("\n")
    );
}

#[test]
fn no_doctor_is_byte_identical_to_the_bare_diagnostic() {
    let sb = Sandbox::new();
    let with = normalise(
        &String::from_utf8_lossy(&run_corpus(&sb, "e508-missing-file.cig", &[]).stderr),
        sb.path(),
    );
    let base = corpus_env(&sb);
    let mut envs: Vec<(&str, &str)> = base.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let flag = normalise(
        &String::from_utf8_lossy(
            &sb.cig_env(&["--no-doctor", "run", "e508-missing-file.cig"], &envs)
                .stderr,
        ),
        sb.path(),
    );
    envs.push(("CIG_DOCTOR", "0"));
    let env_off = normalise(
        &String::from_utf8_lossy(&sb.cig_env(&["run", "e508-missing-file.cig"], &envs).stderr),
        sb.path(),
    );
    assert!(with.contains("= doctor: I think"), "{with}");
    assert!(!flag.contains("doctor"), "{flag}");
    assert_eq!(flag, env_off);
    let stripped: String = with
        .lines()
        .filter(|l| {
            !(l.starts_with("  = doctor:")
                || l.starts_with("  = fix:")
                || l.starts_with("  = if not:")
                || l.starts_with("  = maybe:")
                || l.starts_with("  =  "))
        })
        .map(|l| format!("{l}\n"))
        .collect();
    assert_eq!(
        flag, stripped,
        "--no-doctor must only remove doctor's lines"
    );
}

#[test]
fn json_diagnostics_carry_the_diagnosis_within_budget() {
    let sb = Sandbox::new();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/e551-not-on-path.cig");
    fs::copy(&src, sb.path().join("p.cig")).unwrap();
    let out = sb.cig_env(&["--json", "run", "p.cig"], &[("PATH", "/usr/bin:/bin")]);
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap()
        .to_string();
    let v: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(v["code"], "E551");
    assert_eq!(v["subject"], "definitely-not-a-program-xyz");
    let dx = &v["diagnosis"];
    assert_eq!(dx["confirmed"], true, "{dx}");
    assert_eq!(dx["no"], "cant-see-any");
    assert_eq!(dx["probes"][0]["probe"], "on-path");
    assert_eq!(dx["probes"][0]["outcome"], "confirmed");
    for key in [
        "code",
        "confirmed",
        "verdict",
        "why",
        "fix",
        "if_not",
        "probes",
        "ms",
    ] {
        assert!(dx.get(key).is_some(), "diagnosis lacks `{key}`: {dx}");
    }
    assert!(dx["ms"].as_u64().unwrap() < 500, "{dx}");
    // The check command's JSON carries it per error too.
    sb.write("t.cig", "roll count = 1\nexhale cuont\n");
    let rep: serde_json::Value =
        serde_json::from_slice(&sb.cig(&["--json", "check", "t.cig"]).stdout).unwrap();
    assert_eq!(rep["errors"][0]["diagnosis"]["confirmed"], true, "{rep}");
    assert!(rep["errors"][0]["diagnosis"]["why"][0]
        .as_str()
        .unwrap()
        .contains("`count`"));
}

fn tree_fingerprint(root: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(root).sort_by_file_name() {
        let e = e.unwrap();
        let m = e.metadata().unwrap();
        out.push((e.path().to_path_buf(), m.len(), m.modified().unwrap()));
    }
    out
}

#[test]
fn rediagnosis_is_read_only_and_reproduces_the_verdict() {
    let sb = Sandbox::new();
    run_corpus(&sb, "e508-missing-file.cig", &[]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    let before = tree_fingerprint(sb.path());
    let out = sb.cig(&["doctor", &id]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        err.contains("error[E508 runtime]")
            && err.contains("= doctor: I think the path does not exist"),
        "{err}"
    );
    assert_eq!(
        before,
        tree_fingerprint(sb.path()),
        "cig doctor <run> wrote something"
    );
    // With the whole state directory read-only it still answers.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let home = sb.home();
        if fs::metadata("/proc/self/status").is_ok()
            && fs::read_to_string("/proc/self/status")
                .unwrap()
                .lines()
                .any(|l| l.starts_with("Uid:\t0\t"))
        {
            return; // root ignores mode bits
        }
        for e in walkdir::WalkDir::new(&home).contents_first(false) {
            let e = e.unwrap();
            if e.file_type().is_dir() {
                fs::set_permissions(e.path(), fs::Permissions::from_mode(0o500)).unwrap();
            }
        }
        let out = sb.cig(&["doctor", &id]);
        for e in walkdir::WalkDir::new(&home) {
            let e = e.unwrap();
            if e.file_type().is_dir() {
                fs::set_permissions(e.path(), fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stderr).contains("= doctor: I think"));
    }
    // JSON form.
    let v: serde_json::Value =
        serde_json::from_slice(&sb.cig(&["--json", "doctor", &id]).stdout).unwrap();
    assert_eq!(v["diagnosis"]["confirmed"], true, "{v}");
    assert_eq!(v["diagnostic"]["code"], "E508");
}

#[test]
fn plain_mode_drops_the_catchphrases_and_keeps_the_facts() {
    let sb = Sandbox::new();
    let themed = normalise(
        &String::from_utf8_lossy(&run_corpus(&sb, "e508-missing-file.cig", &[]).stderr),
        sb.path(),
    );
    let base = corpus_env(&sb);
    let envs: Vec<(&str, &str)> = base.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let plain = normalise(
        &String::from_utf8_lossy(
            &sb.cig_env(&["--plain", "run", "e508-missing-file.cig"], &envs)
                .stderr,
        ),
        sb.path(),
    );
    let mut env_plain = envs.clone();
    env_plain.push(("CIG_PLAIN", "1"));
    let via_env = normalise(
        &String::from_utf8_lossy(
            &sb.cig_env(&["run", "e508-missing-file.cig"], &env_plain)
                .stderr,
        ),
        sb.path(),
    );
    assert_eq!(plain, via_env);
    assert!(
        themed.starts_with("Don't see any cigarettes.\n"),
        "{themed}"
    );
    assert!(
        !plain.contains("cigarettes") && !plain.contains("ciggies"),
        "{plain}"
    );
    assert!(
        plain.contains("(wrong address"),
        "the manual's name replaces the hallway phrase: {plain}"
    );
    // Everything else is identical: plain is a subtraction.
    let themed_facts: Vec<&str> = themed
        .lines()
        .skip(1)
        .map(|l| l.split(" (can't see any").next().unwrap())
        .collect();
    let plain_facts: Vec<&str> = plain
        .lines()
        .map(|l| l.split(" (wrong address").next().unwrap())
        .collect();
    assert_eq!(themed_facts, plain_facts);
}

#[test]
fn report_bundles_a_run_redacted() {
    let sb = Sandbox::new();
    run_corpus(&sb, "e551-not-on-path.cig", &[]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    let base = corpus_env(&sb);
    let envs: Vec<(&str, &str)> = base.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let out = sb.cig_env(&["report", &id], &envs);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(
        printed.starts_with("~/home/reports/"),
        "the path is printed with ~ for HOME: {printed}"
    );
    let file = sb.home().join("reports").join(format!("{id}.json"));
    let bundle: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(bundle["kind"], "cigscript-report");
    assert_eq!(bundle["run"]["id"], id);
    assert_eq!(
        bundle["run"]["cwd"], "~",
        "cwd under HOME is redacted to ~: {}",
        bundle["run"]
    );
    assert_eq!(bundle["diagnostic"]["code"], "E551");
    assert_eq!(
        bundle["diagnosis"]["confirmed"], true,
        "{}",
        bundle["diagnosis"]
    );
    assert_eq!(bundle["journal"].as_array().unwrap().len(), 1);
    assert_eq!(bundle["journal"][0]["reversible"], false);
    assert!(bundle["note"].as_str().unwrap().contains("Redacted"));
    let text = fs::read_to_string(&file).unwrap();
    assert!(
        !text.contains(sb.path().to_str().unwrap()),
        "an absolute sandbox path leaked into the bundle"
    );
    let out = sb.cig_env(&["--json", "report", &id], &envs);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["run"]["id"], id);
    let err = sb.run_err(&["report", "zzz"], 3);
    assert!(err.contains("error[E803 usage]"), "{err}");
}

// ----- the Hammer update: the step budget and finally -------------------------

#[test]
fn while_true_ends_with_your_loop_never_ends_and_a_line() {
    let sb = Sandbox::new();
    sb.write("loop.cig", "roll i = 0\nwhile true {\n  i += 1\n}\n");
    let err = sb.run_err(&["run", "--max-steps", "5000", "loop.cig"], 1);
    assert!(
        err.contains("error[E515 runtime]: your loop never ends: 5000 steps and still going"),
        "{err}"
    );
    assert!(err.contains("loop.cig:2:"), "the while line: {err}");
    assert!(
        err.contains("= doctor: I think a while loop whose condition never becomes false"),
        "{err}"
    );
    assert!(err.contains("the condition on line 2 is `true`"), "{err}");
    // The environment sets it too; 0 disables it.
    let out = sb.cig_env(&["run", "loop.cig"], &[("CIG_MAX_STEPS", "3000")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("3000 steps"));
    sb.write(
        "bounded.cig",
        "roll n = 0\nfor i in 1..20001 {\n  n += 1\n}\nexhale n\n",
    );
    let err = sb.run_err(&["run", "--max-steps", "100", "bounded.cig"], 1);
    assert!(err.contains("E515"), "{err}");
    assert_eq!(
        sb.run_ok(&["run", "--max-steps", "0", "bounded.cig"])
            .trim(),
        "20000"
    );
    assert_eq!(
        sb.run_ok(&["run", "bounded.cig"]).trim(),
        "20000",
        "the default budget is generous"
    );
    // Outside a loop the message does not blame a loop.
    sb.write(
        "deep.cig",
        "pull f(n) {\n  if n == 0 { snuff 0 }\n  snuff f(n - 1)\n}\nexhale f(3000)\n",
    );
    let err = sb.run_err(&["run", "--max-steps", "1000", "deep.cig"], 1);
    assert!(
        err.contains("the script needs more than 1000 steps"),
        "{err}"
    );
    // A run that fails on the budget still rolls back what it burned.
    sb.write("k.txt", "keep");
    sb.write(
        "burnloop.cig",
        "burn {\n  fs.write_text(\"k.txt\", \"changed\")\n  while true {}\n}\n",
    );
    let err = sb.run_err(&["run", "--max-steps", "1000", "burnloop.cig"], 1);
    assert!(err.contains("E515") && err.contains("restored"), "{err}");
    assert_eq!(sb.read("k.txt"), "keep");
}

#[test]
fn finally_runs_on_exit_and_a_missing_ashtray_is_a_syntax_error() {
    let sb = Sandbox::new();
    sb.write(
        "x.cig",
        "try {\n  exit(4)\n} finally {\n  exhale \"cleanup\"\n}\nexhale \"not reached\"\n",
    );
    let out = sb.cig(&["run", "x.cig"]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "cleanup\n");
    sb.write("bad.cig", "try {\n  exhale 1\n}\nexhale 2\n");
    let err = sb.run_err(&["run", "bad.cig"], 2);
    assert!(
        err.contains("expected `ashtray` or `finally` after the `try` block"),
        "{err}"
    );
    sb.write("stray.cig", "finally {\n}\n");
    let err = sb.run_err(&["run", "stray.cig"], 2);
    assert!(err.contains("`finally` without a preceding `try`"), "{err}");
    // finally inside a burn keeps the burn accounting straight.
    sb.write("b.cig", "burn {\n  try {\n    fs.write_text(\"a.txt\", \"1\")\n  } finally {\n    fs.write_text(\"b.txt\", \"2\")\n  }\n}\nfs.write_text(\"c.txt\", \"3\")\n");
    let err = sb.run_err(&["run", "b.cig"], 2);
    assert!(
        err.contains("E303"),
        "the write after the burn is still refused: {err}"
    );
}

// ----- the Hammer update: the cross-run check on unburn ------------------------

#[test]
fn unburn_refuses_when_a_file_changed_since_the_run_unless_forced() {
    let sb = Sandbox::new();
    sb.write("f.txt", "one");
    sb.write(
        "s.cig",
        "burn {\n  fs.write_text(\"f.txt\", \"two\")\n  fs.write_text(\"g.txt\", \"new\")\n}\n",
    );
    sb.run_ok(&["run", "s.cig"]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    assert!(sb
        .home()
        .join("runs")
        .join(&id)
        .join("after.json")
        .is_file());
    // Untouched since: the plan says so, and the rollback is allowed.
    let plan = sb.run_ok(&["unburn", &id, "--dry-run"]);
    assert!(
        plan.contains("unchanged since") && !plan.contains("changed since:"),
        "{plan}"
    );
    // Someone edits f.txt by hand.
    sb.write("f.txt", "three");
    let plan = sb.run_ok(&["unburn", &id, "--dry-run"]);
    assert!(
        plan.contains("changed since: ") && plan.contains("f.txt") && plan.contains("E704"),
        "{plan}"
    );
    let err = sb.run_err(&["unburn", &id], 1);
    assert!(
        err.contains("error[E704 burn]:") && err.contains("f.txt") && err.contains("now a file"),
        "{err}"
    );
    assert_eq!(
        sb.read("f.txt"),
        "three",
        "a refused unburn touches nothing"
    );
    assert!(sb.exists("g.txt"));
    sb.run_ok(&["unburn", &id, "--force"]);
    assert_eq!(sb.read("f.txt"), "one");
    assert!(!sb.exists("g.txt"));
}

#[test]
fn unburn_names_the_later_run_that_changed_the_file() {
    let sb = Sandbox::new();
    sb.write("f.txt", "one");
    sb.write("a.cig", "burn {\n  fs.write_text(\"f.txt\", \"two\")\n}\n");
    sb.write(
        "b.cig",
        "burn {\n  fs.write_text(\"f.txt\", \"three\")\n}\n",
    );
    sb.run_ok(&["run", "a.cig"]);
    let a = sb.runs()[0]["id"].as_str().unwrap().to_string();
    std::thread::sleep(std::time::Duration::from_millis(1100)); // ids are second-resolution
    sb.run_ok(&["run", "b.cig"]);
    let b = sb.runs()[0]["id"].as_str().unwrap().to_string();
    assert_ne!(a, b);
    let err = sb.run_err(&["unburn", &a], 1);
    assert!(
        err.contains("E704") && err.contains(&format!("changed by run {b}")),
        "{err}"
    );
    let plan = sb.run_ok(&["unburn", &a, "--dry-run"]);
    assert!(plan.contains(&format!("by run {b}")), "{plan}");
    // Newest first across runs: unburn b, then a.
    sb.run_ok(&["unburn", &b]);
    assert_eq!(sb.read("f.txt"), "two");
    sb.run_ok(&["unburn", &a]);
    assert_eq!(sb.read("f.txt"), "one");
}

#[test]
fn runs_without_after_state_are_still_unburnable_and_say_so() {
    let sb = Sandbox::new();
    sb.write("f.txt", "one");
    sb.write("s.cig", "burn {\n  fs.write_text(\"f.txt\", \"two\")\n}\n");
    sb.run_ok(&["run", "s.cig"]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    fs::remove_file(sb.home().join("runs").join(&id).join("after.json")).unwrap();
    sb.write("f.txt", "three");
    let plan = sb.run_ok(&["unburn", &id, "--dry-run"]);
    assert!(
        plan.contains("changed since: unknown") && plan.contains("predates"),
        "{plan}"
    );
    sb.run_ok(&["unburn", &id]);
    assert_eq!(sb.read("f.txt"), "one");
}

// ----- the Hammer update: the ghost filesystem --------------------------------

#[test]
fn chain_dry_run_reads_ghost_write() {
    let sb = Sandbox::new();
    sb.write(
        "tasks.cig",
        concat!(
            "stick fetch = pack() { burn { fs.write_text(\"build/src.txt\", \"hello\") } }\n",
            "stick build = pack() { burn { fs.write_text(\"build/out.txt\", fs.read_text(\"build/src.txt\").upper()) } }\n",
            "stick report = pack() { exhale fs.read_text(\"build/out.txt\"), fs.list(\"build\").len() }\n",
            "chain release { fetch, build, report }\n"
        ),
    );
    let out = sb.cig(&["light", "--dry-run", "tasks.cig", "release"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{stderr}");
    assert!(
        stdout.contains("HELLO 2"),
        "step 3 read what step 2 pretended to write: {stdout}"
    );
    assert!(stderr.contains("dry-run plan: 2 ops"), "{stderr}");
    assert!(!sb.exists("build"), "the disk was touched by a dry-run");
    let out = sb.cig(&["light", "tasks.cig", "release"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(sb.read("build/out.txt"), "HELLO");
}

#[test]
fn dry_run_reads_see_the_ghost_filesystem() {
    let sb = Sandbox::new();
    sb.write("real.txt", "disk");
    sb.write("log.txt", "one\n");
    fs::create_dir_all(sb.path().join("dir")).unwrap();
    sb.write("dir/a.txt", "a");
    sb.write(
        "s.cig",
        concat!(
            "burn {\n",
            "  fs.write_text(\"new/deep/file.txt\", \"made\")\n",
            "  exhale fs.read_text(\"new/deep/file.txt\"), fs.exists(\"new/deep\"), fs.is_dir(\"new\"), fs.size(\"new/deep/file.txt\")\n",
            "  fs.append_text(\"log.txt\", \"two\\n\")\n",
            "  exhale fs.read_lines(\"log.txt\").len()\n",
            "  fs.rm(\"real.txt\")\n",
            "  exhale fs.exists(\"real.txt\"), fs.is_file(\"real.txt\")\n",
            "  fs.mv(\"dir\", \"moved\")\n",
            "  exhale fs.exists(\"dir\"), fs.read_text(\"moved/a.txt\"), fs.list(\"moved\")\n",
            "  fs.cp(\"moved/a.txt\", \"copy.txt\")\n",
            "  json.save(\"data.json\", {n: 1})\n",
            "  exhale json.load(\"data.json\").n, hash.sha256_file(\"copy.txt\") == hash.sha256(\"a\")\n",
            "  exhale fs.glob(\"*.txt\")\n",
            "  exhale fs.mkdir(\"new\")\n",
            "}\n"
        ),
    );
    let out = sb.cig(&["run", "--dry-run", "s.cig"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "made true true 4");
    assert_eq!(lines[1], "2");
    assert_eq!(lines[2], "false false");
    assert_eq!(lines[3], "false a [\"moved/a.txt\"]");
    assert_eq!(lines[4], "1 true");
    assert_eq!(
        lines[5], "[\"copy.txt\", \"log.txt\"]",
        "removed real.txt hidden, ghost copy.txt shown"
    );
    assert_eq!(lines[6], "false", "the ghost knows `new` exists");
    assert_eq!(sb.read("real.txt"), "disk");
    assert_eq!(sb.read("log.txt"), "one\n");
    assert!(
        sb.exists("dir/a.txt")
            && !sb.exists("moved")
            && !sb.exists("new")
            && !sb.exists("copy.txt")
    );
}

#[test]
fn dry_run_reports_reads_of_ghost_removed_paths_as_e520_and_missing_sources_as_e508() {
    let sb = Sandbox::new();
    sb.write("a.txt", "a");
    sb.write(
        "s.cig",
        "burn {\n  fs.rm(\"a.txt\")\n  exhale fs.read_text(\"a.txt\")\n}\n",
    );
    let err = sb.run_err(&["run", "--dry-run", "s.cig"], 1);
    assert!(err.contains("error[E520 runtime]: fs.read_text: a.txt: removed earlier in this dry-run by op 1 (delete a.txt)"), "{err}");
    assert!(
        err.contains(
            "= doctor: I think an earlier step's simulated delete or move took the path away"
        ),
        "{err}"
    );
    assert_eq!(sb.read("a.txt"), "a");
    sb.write("t.cig", "burn {\n  fs.cp(\"nope.txt\", \"x.txt\")\n}\n");
    let err = sb.run_err(&["run", "--dry-run", "t.cig"], 1);
    assert!(
        err.contains("error[E508 runtime]: fs.cp: nope.txt: no such file or directory"),
        "{err}"
    );
    sb.write(
        "u.cig",
        "burn {\n  fs.rm(\"a.txt\")\n  fs.rm(\"a.txt\")\n}\n",
    );
    let err = sb.run_err(&["run", "--dry-run", "u.cig"], 1);
    assert!(
        err.contains("E520") && err.contains("fs.rm: a.txt"),
        "{err}"
    );
}

#[test]
fn unlit_burns_stay_out_of_the_ghost_and_off_the_disk() {
    let sb = Sandbox::new();
    sb.write("s.cig", "burn unlit {\n  fs.write_text(\"u.txt\", \"x\")\n}\nexhale fs.exists(\"u.txt\")\nburn {\n  fs.write_text(\"w.txt\", \"y\")\n}\nexhale fs.exists(\"w.txt\")\n");
    assert_eq!(sb.run_ok(&["run", "s.cig"]).trim(), "false\ntrue");
    assert!(!sb.exists("u.txt") && sb.exists("w.txt"));
    fs::remove_file(sb.path().join("w.txt")).unwrap();
    assert_eq!(
        sb.run_ok(&["run", "--dry-run", "s.cig"]).trim(),
        "false\ntrue",
        "in a dry-run the unlit write stays out, the burn's write is a ghost"
    );
    assert!(!sb.exists("u.txt") && !sb.exists("w.txt"));
}

#[test]
fn a_file_added_to_a_run_created_directory_is_not_lost_by_unburn() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "burn {\n  fs.mkdir(\"made\")\n  fs.write_text(\"made/a.txt\", \"a\")\n}\n",
    );
    sb.run_ok(&["run", "s.cig"]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    // Untouched since: the directory's fingerprint matches.
    let plan = sb.run_ok(&["unburn", &id, "--dry-run"]);
    assert!(
        plan.contains("unchanged since") && !plan.contains("changed since:"),
        "{plan}"
    );
    // Someone keeps their own file in the directory the run created.
    sb.write("made/keep.txt", "mine");
    let plan = sb.run_ok(&["unburn", &id, "--dry-run"]);
    assert!(
        plan.contains("changed since: ") && plan.contains("made") && plan.contains("E704"),
        "{plan}"
    );
    let err = sb.run_err(&["unburn", &id], 1);
    assert!(
        err.contains("error[E704 burn]:") && err.contains("now a directory with 2 entries"),
        "{err}"
    );
    assert_eq!(
        sb.read("made/keep.txt"),
        "mine",
        "a refused unburn touches nothing"
    );
    // --force is the knowing choice.
    sb.run_ok(&["unburn", &id, "--force"]);
    assert!(!sb.exists("made"));
}

#[test]
fn a_journal_cut_short_by_a_crash_still_unburns_what_came_before_it() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "burn {\n  fs.write_text(\"one.txt\", \"1\")\n  fs.write_text(\"two.txt\", \"2\")\n}\n",
    );
    sb.run_ok(&["run", "s.cig"]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    let journal = sb.home().join("runs").join(&id).join("journal.jsonl");
    // A crash while appending leaves half a line.
    let mut text = fs::read_to_string(&journal).unwrap();
    text.push_str("{\"seq\": 3, \"op\": {\"kind\": \"wri");
    fs::write(&journal, text).unwrap();
    let out = sb.cig(&["unburn", &id]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(
        err.contains("the journal's last line was cut short")
            && err.contains("2 entries before it loaded"),
        "{err}"
    );
    assert!(!sb.exists("one.txt") && !sb.exists("two.txt"));
}

#[test]
fn fifty_thousand_nested_brackets_are_a_diagnostic_not_a_stack_overflow() {
    let sb = Sandbox::new();
    let deep = format!("stick x = {}{}\n", "[".repeat(50_000), "]".repeat(50_000));
    sb.write("deep.cig", &deep);
    let err = sb.run_err(&["run", "deep.cig"], 2);
    assert!(
        err.contains("error[E208 syntax]: nesting deeper than 5000 levels"),
        "{err}"
    );
    // The 100 kB line is windowed around the caret, not printed whole.
    assert!(
        err.len() < 2_000,
        "the source line was printed whole: {} bytes",
        err.len()
    );
    assert!(err.contains("…[[[["), "{err}");
    let deep = format!("stick y = {}1{}\n", "(".repeat(50_000), ")".repeat(50_000));
    sb.write("parens.cig", &deep);
    let err = sb.run_err(&["run", "parens.cig"], 2);
    assert!(err.contains("E208"), "{err}");
    let deep = format!("{}{}", "if true {\n".repeat(20_000), "}\n".repeat(20_000));
    sb.write("blocks.cig", &deep);
    let err = sb.run_err(&["run", "blocks.cig"], 2);
    assert!(err.contains("E208"), "{err}");
}

#[test]
fn a_utf8_byte_order_mark_is_skipped() {
    let sb = Sandbox::new();
    sb.write("bom.cig", "\u{feff}exhale \"bom ok\"\n");
    let out = sb.run_ok(&["run", "bom.cig"]);
    assert_eq!(out.trim(), "bom ok");
}

#[test]
fn a_list_that_contains_itself_prints_without_overflowing() {
    let sb = Sandbox::new();
    sb.write(
        "cyc.cig",
        "roll l = [1]\nl.push(l)\nexhale l\nroll m = {}\nm.me = m\nexhale m\n",
    );
    let out = sb.run_ok(&["run", "cyc.cig"]);
    assert!(out.contains("…"), "{out}");
    assert!(out.len() < 20_000, "{} bytes", out.len());
}

// ----- hardening, second round --------------------------------------------

#[test]
fn a_copy_or_move_onto_itself_or_into_itself_is_refused() {
    let sb = Sandbox::new();
    sb.write("a.txt", "hello");
    sb.write("s.cig", "burn {\n  fs.cp(\"a.txt\", \"a.txt\")\n}\n");
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(
        err.contains("error[E508 runtime]: fs.cp: a.txt and a.txt are the same file"),
        "{err}"
    );
    assert_eq!(sb.read("a.txt"), "hello", "the copy truncated the file");
    sb.write("s.cig", "burn {\n  fs.mv(\"a.txt\", \"./a.txt\")\n}\n");
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(err.contains("are the same file"), "{err}");
    assert_eq!(sb.read("a.txt"), "hello");
    sb.write("d/f.txt", "keep");
    sb.write("s.cig", "burn {\n  fs.cp(\"d\", \"d/sub\")\n}\n");
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(
        err.contains("fs.cp: d/sub is inside d, the directory being copied"),
        "{err}"
    );
    assert!(!sb.exists("d/sub"));
}

#[test]
fn a_move_the_os_refused_is_marked_undone_and_never_moved_back() {
    let sb = Sandbox::new();
    // Caught inside the run: the journal must not say the move happened.
    sb.write("d/f.txt", "keep");
    sb.write(
        "s.cig",
        "burn {\n  fs.write_text(\"d/g.txt\", \"g\")\n  try {\n    fs.mv(\"d\", \"d/sub\")\n  } ashtray e {\n    exhale \"caught\"\n  }\n}\n",
    );
    let out = sb.run_ok(&["run", "s.cig"]);
    assert_eq!(out.trim(), "caught");
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    assert_eq!(sb.runs()[0]["burns"], 1, "the refused move is not a burn");
    sb.run_ok(&["unburn", &id]);
    assert_eq!(sb.read("d/f.txt"), "keep");
    assert!(!sb.exists("d/g.txt"));
    // Uncaught, onto a non-empty directory: rollback leaves the source alone.
    sb.write("important.txt", "important");
    sb.write("blocker/other.txt", "other");
    sb.write(
        "s.cig",
        "burn {\n  fs.mv(\"important.txt\", \"blocker\")\n}\n",
    );
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(
        err.contains("error[E508 runtime]: fs.mv: important.txt"),
        "{err}"
    );
    assert_eq!(sb.read("important.txt"), "important");
    assert_eq!(sb.read("blocker/other.txt"), "other");
}

#[test]
fn a_cross_device_move_rolls_back_by_copying() {
    use std::os::unix::fs::MetadataExt;
    let sb = Sandbox::new();
    let shm = Path::new("/dev/shm");
    let Ok(other) = tempfile::tempdir_in(shm) else {
        return; // no second filesystem to move across on this machine
    };
    if fs::metadata(other.path()).map(|m| m.dev()).ok()
        == fs::metadata(sb.path()).map(|m| m.dev()).ok()
    {
        return;
    }
    let src = other.path().join("src.txt");
    fs::write(&src, "across").unwrap();
    sb.write(
        "s.cig",
        &format!(
            "burn {{\n  fs.mv(\"{}\", \"landed.txt\")\n  cough \"x\"\n}}\n",
            src.display()
        ),
    );
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(err.contains("moved back"), "{err}");
    assert_eq!(fs::read_to_string(&src).unwrap(), "across");
    assert!(!sb.exists("landed.txt"));
}

#[test]
fn a_hardlinked_file_is_restored_in_place() {
    let sb = Sandbox::new();
    sb.write("orig", "before");
    fs::hard_link(sb.path().join("orig"), sb.path().join("other")).unwrap();
    sb.write(
        "s.cig",
        "burn {\n  fs.write_text(\"orig\", \"after\")\n  cough \"x\"\n}\n",
    );
    sb.run_err(&["run", "s.cig"], 1);
    assert_eq!(sb.read("orig"), "before");
    assert_eq!(
        sb.read("other"),
        "before",
        "the other name kept the run's content"
    );
}

#[test]
fn setuid_and_sticky_bits_survive_rollback() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    sb.write("suid", "s");
    fs::set_permissions(sb.path().join("suid"), fs::Permissions::from_mode(0o4755)).unwrap();
    fs::create_dir(sb.path().join("sticky")).unwrap();
    fs::set_permissions(sb.path().join("sticky"), fs::Permissions::from_mode(0o1777)).unwrap();
    sb.write(
        "s.cig",
        "burn {\n  fs.rm(\"suid\")\n  fs.rm(\"sticky\")\n  cough \"x\"\n}\n",
    );
    sb.run_err(&["run", "s.cig"], 1);
    let mode = |name: &str| {
        fs::metadata(sb.path().join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    };
    assert_eq!(mode("suid"), 0o4755);
    assert_eq!(mode("sticky"), 0o1777);
}

#[test]
fn a_named_pipe_is_refused_not_hung() {
    use wait_timeout::ChildExt;
    let sb = Sandbox::new();
    assert!(Command::new("mkfifo")
        .arg(sb.path().join("fifo"))
        .status()
        .expect("mkfifo")
        .success());
    sb.write("s.cig", "burn {\n  fs.rm(\"fifo\")\n}\n");
    let mut child = Command::new(env!("CARGO_BIN_EXE_cig"))
        .args(["run", "s.cig"])
        .current_dir(sb.path())
        .env("CIGSCRIPT_HOME", sb.home())
        .env("NO_COLOR", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn cig");
    let status = child
        .wait_timeout(std::time::Duration::from_secs(20))
        .expect("wait");
    let Some(status) = status else {
        let _ = child.kill();
        panic!("cig hung on the named pipe");
    };
    assert_eq!(status.code(), Some(1));
    let mut err = String::new();
    std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut err).unwrap();
    assert!(
        err.contains("error[E702 burn]:") && err.contains("cannot snapshot it"),
        "{err}"
    );
    assert!(sb.path().join("fifo").exists());
}

#[test]
fn rollback_failures_are_recorded_on_the_run() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "pack { \".\" }\nburn {\n  fs.write_text(\"free.txt\", \"1\")\n  fs.mkdir(\"locked\")\n  fs.write_text(\"locked/secret.txt\", \"hidden\")\n  proc.run(\"chmod\", [\"000\", \"locked\"])\n  cough \"boom\"\n}\n",
    );
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(
        err.contains("failed write locked/secret.txt") && err.contains("restored write free.txt"),
        "{err}"
    );
    let rec = &sb.runs()[0];
    assert_eq!(rec["status"], "rollback_incomplete");
    assert_eq!(rec["rolled_back"], true);
    assert_eq!(rec["rollback_failed"].as_array().map(Vec::len), Some(2));
    let id = rec["id"].as_str().unwrap().to_string();
    let shown = sb.run_ok(&["runs", &id]);
    assert!(
        shown.contains("status    rollback_incomplete") && shown.contains("2 restores failed"),
        "{shown}"
    );
    let err = sb.run_err(&["unburn", &id], 1);
    assert!(
        err.contains("error[E705 burn]:") && err.contains("2 restores failed the first time"),
        "{err}"
    );
    fs::set_permissions(sb.path().join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    sb.run_ok(&["unburn", &id, "--force"]);
    assert!(!sb.exists("locked") && !sb.exists("free.txt"));
}

#[test]
fn a_read_only_run_directory_refuses_unburn_until_forced() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    sb.write("s.cig", "burn {\n  fs.write_text(\"ro.txt\", \"1\")\n}\n");
    sb.run_ok(&["run", "s.cig"]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    let dir = sb.home().join("runs").join(&id);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
    let err = sb.run_err(&["unburn", &id], 3);
    assert!(
        err.contains("error[E802 usage]: cannot record the rollback"),
        "{err}"
    );
    assert!(sb.exists("ro.txt"), "a refused unburn touches nothing");
    let out = sb.cig(&["unburn", &id, "--force"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(
        err.contains("--force given") && err.contains("could not record"),
        "{err}"
    );
    assert!(!sb.exists("ro.txt"));
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_damaged_or_renamed_run_record_still_lists_and_unburns() {
    let sb = Sandbox::new();
    sb.write("s.cig", "burn {\n  fs.write_text(\"dm.txt\", \"1\")\n}\n");
    sb.run_ok(&["run", "s.cig"]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    fs::write(sb.home().join("runs").join(&id).join("run.json"), "garbage").unwrap();
    let rec = sb
        .runs()
        .into_iter()
        .find(|r| r["id"] == id.as_str())
        .expect("listed");
    assert_eq!(rec["status"], "damaged");
    assert_eq!(rec["burns"], 1, "counted from the journal");
    let shown = sb.run_ok(&["runs", &id]);
    assert!(shown.contains("damaged"), "{shown}");
    sb.run_ok(&["unburn", &id]);
    assert!(!sb.exists("dm.txt"));
    // A renamed directory: the record remembers where it was read from.
    sb.run_ok(&["run", "s.cig"]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    let renamed = sb.home().join("runs").join("RENAMED-1");
    fs::rename(sb.home().join("runs").join(&id), &renamed).unwrap();
    let shown = sb.run_ok(&["runs", &id]);
    assert!(shown.contains("RENAMED-1"), "{shown}");
    sb.run_ok(&["unburn", &id]);
    assert!(!sb.exists("dm.txt"));
}

#[test]
fn a_dead_run_whose_pid_was_reused_is_interrupted_not_running() {
    let sb = Sandbox::new();
    sb.write("s.cig", "burn {\n  fs.write_text(\"p.txt\", \"1\")\n}\n");
    sb.run_ok(&["run", "s.cig"]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    let path = sb.home().join("runs").join(&id).join("run.json");
    let mut rec: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    rec["status"] = serde_json::Value::String("running".to_string());
    rec["pid"] = serde_json::Value::from(1);
    fs::write(&path, rec.to_string()).unwrap();
    assert_eq!(sb.runs()[0]["status"], "interrupted");
}

#[test]
fn a_burn_block_that_fails_half_way_leaves_its_hops_honestly_irreversible() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "burn (s) {\n  proc.run(\"sh\", [\"-c\", \"echo before > before.txt\"])\n  cough \"abort\"\n} unburn {\n  exhale \"should never run\"\n}\n",
    );
    let out = sb.cig(&["run", "s.cig"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1));
    assert!(err.contains("cannot undo run sh -c"), "{err}");
    assert!(!err.contains("compensated"), "{err}");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("should never run"));
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    let shown = sb.run_ok(&["runs", &id]);
    assert!(shown.contains("irreversible  run sh -c"), "{shown}");
    // A block that completes stamps its hop, and a later cough runs the block.
    sb.write(
        "s.cig",
        "burn (s) {\n  proc.run(\"sh\", [\"-c\", \"echo done > done.txt\"])\n  s.n = 1\n} unburn {\n  exhale \"undoing\", s.n\n}\nburn {\n  cough \"later\"\n}\n",
    );
    let out = sb.cig(&["run", "s.cig"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("compensated run sh -c"), "{err}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("undoing 1"),
        "{err}"
    );
}

#[test]
fn a_compensation_that_reads_args_runs_on_a_deferred_unburn() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "burn (s) {\n  fs.write_text(\"marker.txt\", \"x\")\n} unburn {\n  exhale \"args:\", args.len(), args[0]\n}\n",
    );
    sb.run_ok(&["run", "s.cig", "--", "a", "b", "c"]);
    let id = sb.runs()[0]["id"].as_str().unwrap().to_string();
    let out = sb.cig(&["unburn", &id]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("args: 3 a"),
        "{err}"
    );
    assert!(!sb.exists("marker.txt"));
}

#[test]
fn the_ghost_resolves_symlinked_directories_and_lists_created_ancestors() {
    use std::os::unix::fs::symlink;
    let sb = Sandbox::new();
    fs::create_dir(sb.path().join("real")).unwrap();
    symlink("real", sb.path().join("link")).unwrap();
    sb.write(
        "s.cig",
        "burn {\n  fs.write_text(\"link/x.txt\", \"via-link\")\n  exhale fs.read_text(\"real/x.txt\")\n  fs.write_text(\"a/b/c/file.txt\", \"deep\")\n  exhale fs.list(\"a\"), fs.list(\"a/b\")\n}\n",
    );
    let out = sb.run_ok(&["run", "--dry-run", "s.cig"]);
    assert_eq!(out.trim(), "via-link\n[\"a/b\"] [\"a/b/c\"]", "{out}");
    assert!(!sb.exists("real/x.txt") && !sb.exists("a"));
}

#[test]
fn an_empty_pack_root_is_refused() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "pack { \"\" }\nburn {\n  fs.write_text(\"z.txt\", \"z\")\n}\n",
    );
    let err = sb.run_err(&["run", "s.cig"], 2);
    assert!(
        err.contains("error[E755 check]: a pack root is empty"),
        "{err}"
    );
    sb.write(
        "s.cig",
        "pack { \"\" + \"\" }\nburn {\n  fs.write_text(\"z.txt\", \"z\")\n}\n",
    );
    let err = sb.run_err(&["run", "s.cig"], 1);
    assert!(
        err.contains("error[E755 burn]: a pack root is empty"),
        "{err}"
    );
    assert!(!sb.exists("z.txt"));
}

#[test]
fn i64_min_divided_by_minus_one_is_a_diagnostic() {
    let sb = Sandbox::new();
    let err = sb.run_err(&["eval", "--", "-9223372036854775808 / -1"], 1);
    assert!(
        err.contains("error[E503 runtime]: integer overflow in `/`"),
        "{err}"
    );
    assert!(!err.contains("E901"), "{err}");
    let out = sb.run_ok(&["eval", "--", "-9223372036854775808 % -1"]);
    assert_eq!(out.trim(), "0");
}

#[test]
fn a_value_that_contains_itself_is_refused_by_json_and_compared_without_looping() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "roll l = [1]\nl.push(l)\nroll m = [1]\nm.push(m)\nexhale l == m, l == [1, l], l == [2, l]\nroll a = {}\na.me = a\nroll b = {}\nb.me = b\nexhale a == b\nexhale json.stringify(l)\n",
    );
    let out = sb.cig(&["run", "s.cig"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "true true false\ntrue"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("error[E500 runtime]: json: the value contains itself"),
        "{err}"
    );
}

#[test]
fn floats_stay_floats_past_sixteen_digits_and_int_refuses_what_does_not_fit() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "exhale 1e16, 1.5e20, 9999999999999998.0, 0.1, 2.0, type_of(1e16)\nexhale int(3.9), int(\"42\"), int(\"1e3\")\n",
    );
    let out = sb.run_ok(&["run", "s.cig"]);
    assert_eq!(
        out.trim(),
        "1e16 1.5e20 9999999999999998.0 0.1 2.0 float\n3 42 1000"
    );
    let err = sb.run_err(&["eval", "--", "int(1e300)"], 1);
    assert!(
        err.contains("error[E503 runtime]: int: 1e300 does not fit in an int"),
        "{err}"
    );
    let err = sb.run_err(&["eval", "--", "int(\"9223372036854775808\")"], 1);
    assert!(err.contains("does not fit in an int"), "{err}");
    let err = sb.run_err(&["eval", "--", "int(-1e300)"], 1);
    assert!(err.contains("E503"), "{err}");
}

#[test]
fn a_redeclaration_at_run_time_is_a_check_error_and_args_is_shadowable() {
    let sb = Sandbox::new();
    sb.write("s.cig", "roll y = 1\nroll y = 2\n");
    let err = sb.run_err(&["run", "--no-check", "s.cig"], 2);
    assert!(err.contains("error[E306 check]:"), "{err}");
    sb.write("s.cig", "roll args = [\"mine\"]\nexhale args\n");
    let out = sb.run_ok(&["check", "s.cig"]);
    assert!(out.contains("ok") || out.is_empty(), "{out}");
    let out = sb.run_ok(&["run", "s.cig"]);
    assert_eq!(out.trim(), "[\"mine\"]");
    sb.write("s.cig", "exhale args\n");
    let out = sb.run_ok(&["run", "s.cig", "--", "x"]);
    assert_eq!(out.trim(), "[\"x\"]");
}

#[test]
fn a_string_that_runs_past_its_line_is_e101() {
    let sb = Sandbox::new();
    sb.write("s.cig", "roll s = \"a\nb\"\nexhale s\n");
    let err = sb.run_err(&["run", "s.cig"], 2);
    assert!(
        err.contains(
            "error[E101 lex]: unterminated string literal: the line ends before the closing quote"
        ) && err.contains("--> s.cig:1:10"),
        "{err}"
    );
    sb.write("s.cig", "roll s = \"a\\nb\"\nexhale s.len()\n");
    assert_eq!(sb.run_ok(&["run", "s.cig"]).trim(), "3");
}

#[test]
fn chained_operators_are_capped_and_a_bad_step_budget_is_a_usage_error() {
    let sb = Sandbox::new();
    let chain = format!("exhale 1{}\n", " + 1".repeat(12_000));
    sb.write("chain.cig", &chain);
    let err = sb.run_err(&["run", "chain.cig"], 2);
    assert!(
        err.contains("error[E208 syntax]: more than 10000 operators chained in one expression"),
        "{err}"
    );
    let chain = format!("exhale 1{}\n", " + 1".repeat(8_000));
    sb.write("chain.cig", &chain);
    assert_eq!(sb.run_ok(&["run", "chain.cig"]).trim(), "8001");
    sb.write("s.cig", "exhale 1\n");
    let out = sb.cig_env(&["run", "s.cig"], &[("CIG_MAX_STEPS", "abc")]);
    assert_eq!(out.status.code(), Some(3));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("error[E800 usage]: CIG_MAX_STEPS=`abc` is not a step count"),
        "{err}"
    );
    let out = sb.cig_env(&["run", "s.cig"], &[("CIG_MAX_STEPS", "5")]);
    assert!(out.status.success());
}

#[test]
fn a_device_as_the_script_is_refused_and_a_bad_invocation_exits_3() {
    let sb = Sandbox::new();
    if Path::new("/dev/zero").exists() {
        let err = sb.run_err(&["run", "/dev/zero"], 3);
        assert!(
            err.contains("error[E801 usage]: cannot read /dev/zero: larger than 64 MiB"),
            "{err}"
        );
    }
    let err = sb.run_err(&["run", "does-not-exist.cig"], 3);
    assert!(err.contains("error[E801 usage]:"), "{err}");
    assert_eq!(sb.cig(&["frobnicate"]).status.code(), Some(3));
    assert_eq!(sb.cig(&["run"]).status.code(), Some(3));
    assert_eq!(
        sb.cig(&["run", "--max-steps", "abc", "x.cig"])
            .status
            .code(),
        Some(3)
    );
    assert_eq!(sb.cig(&["--version"]).status.code(), Some(0));
    assert_eq!(sb.cig(&["--help"]).status.code(), Some(0));
}

#[test]
fn crash_send_honours_never() {
    let sb = Sandbox::new();
    sb.write("s.cig", "exhale 1\n");
    let out = sb.cig_env(&["run", "s.cig"], &[("CIG_INTERNAL_PANIC", "1")]);
    assert_eq!(out.status.code(), Some(70));
    let listed = sb.run_ok(&["--json", "crash", "list"]);
    let v: Vec<serde_json::Value> = serde_json::from_str(&listed).unwrap();
    let id = v[0]["id"].as_str().unwrap().to_string();
    sb.run_ok(&["config", "crash_reports", "never"]);
    let err = sb.run_err(&["crash", "send", &id], 3);
    assert!(
        err.contains("error[E800 usage]: crash_reports is \"never\""),
        "{err}"
    );
}

#[test]
fn env_changes_are_labelled_irreversible_in_the_plan() {
    let sb = Sandbox::new();
    sb.write(
        "s.cig",
        "burn {\n  env.set(\"CIG_PROBE\", \"1\")\n  env.unset(\"CIG_PROBE\")\n}\n",
    );
    let out = sb.cig(&["run", "--dry-run", "s.cig"]);
    let plan = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{plan}");
    assert!(
        plan.contains("irreversible  set env CIG_PROBE")
            && plan.contains("irreversible  unset env CIG_PROBE"),
        "{plan}"
    );
    assert!(!plan.contains("reversible    set env"), "{plan}");
    let out = sb.cig(&["run", "s.cig"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(err.contains("(2 irreversible)"), "{err}");
}
