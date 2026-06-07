//! Integration tests for `klasp-plugin-agentic-flow`.
//!
//! Each test drives the compiled binary directly via
//! `CARGO_BIN_EXE_klasp-plugin-agentic-flow`. A real temp git repo is built so
//! `git diff` works for the freshness check; synthetic flow.yaml + state.json +
//! receipts are written into it; the `PluginGateInput` JSON is built by hand
//! with `schema_version = 2`.
//!
//! Tests are gated on `#[cfg(unix)]` to match the pre-commit reference plugin's
//! harness assumptions (git + sh fixtures). On Windows the plugin still compiles
//! but these specific integration tests are skipped.
//!
//! Test list:
//!  1. describe_emits_protocol_v0
//!  2. gate_all_required_fresh_returns_pass        (LOAD-BEARING positive)
//!  3. gate_missing_required_receipt_returns_fail_with_resume (LOAD-BEARING negative)
//!  4. gate_stale_receipt_returns_fail
//!  5. gate_unconfirmed_user_confirm_step_fails
//!  6. gate_legit_skipped_step_passes
//!  7. gate_unknown_manifest_step_warns_not_fails
//!  8. gate_missing_receipts_dir_returns_warn
//!  9. gate_malformed_receipt_json_returns_warn
//! 10. gate_malformed_stdin_returns_warn_and_exits_zero
//! 11. gate_empty_stdin_returns_warn_and_exits_zero
//! 12. gate_unknown_protocol_version_warns_not_fails
//! 13. gate_git_diff_unavailable_returns_warn
//! 14. diff_hash_parity (uses the same git recipe as the plugin)

#![cfg(unix)]

use std::io::Write as IoWrite;
use std::path::Path;
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// Absolute path to the compiled plugin binary.
fn plugin_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp-plugin-agentic-flow")
}

const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin";

/// The full agentic-flow flow.yaml manifest (12 steps in canonical order).
fn full_manifest() -> &'static str {
    "version: 1
steps:
  - id: ideate
    enabled: true
    gating: user-confirm
  - id: graphify-onboard
    enabled: true
    gating: auto
  - id: feature-dev
    enabled: true
    gating: user-confirm
  - id: log-issue
    enabled: true
    gating: user-confirm
  - id: dispatch-impl
    enabled: true
    gating: user-confirm
  - id: simplify
    enabled: true
    gating: auto
  - id: code-review
    enabled: true
    gating: auto
  - id: review-handoff
    enabled: true
    gating: auto
  - id: quality-gates
    enabled: true
    gating: user-confirm
  - id: triage-followups
    enabled: true
    gating: auto
  - id: merge
    enabled: true
    gating: auto
  - id: schedule-routines
    enabled: true
    gating: user-confirm
"
}

/// Run a git command in `dir`, asserting success.
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Initialize a git repo with a base commit on `main`, then a feature branch
/// with one extra commit, so `git diff origin/main...HEAD` is non-empty.
///
/// Returns (repo_dir, base_ref). We use a local ref `main` as the base and tag
/// it as `origin/main` via a same-name local branch so the plugin's
/// `<base_ref>...HEAD` resolves.
fn init_git_repo(tmp: &TempDir) -> (std::path::PathBuf, String) {
    let dir = tmp.path().to_path_buf();
    git(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("base.txt"), "base\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "base"]);

    // Create a local ref that stands in for origin/main at the base commit.
    git(&dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);

    // Feature branch with one extra commit → non-empty diff vs base.
    git(&dir, &["checkout", "-q", "-b", "feature/thing"]);
    std::fs::write(dir.join("feature.txt"), "feature work\n").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "feature"]);

    (dir, "origin/main".to_string())
}

/// Compute the canonical diff hash the SAME way the plugin does — the parity
/// guard. `kind` is "commit" or "push". For push: three-dot only. For commit:
/// three-dot ++ staged.
fn expected_diff_hash(repo: &Path, base_ref: &str, kind: &str) -> String {
    let three_dot = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "diff",
            "--no-color",
            "--no-ext-diff",
            &format!("{base_ref}...HEAD"),
        ])
        .output()
        .expect("git diff three-dot")
        .stdout;
    let mut hasher = Sha256::new();
    hasher.update(&three_dot);
    if kind == "commit" {
        let cached = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["diff", "--no-color", "--no-ext-diff", "--cached"])
            .output()
            .expect("git diff cached")
            .stdout;
        hasher.update(&cached);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        hex.push_str(&format!("{b:02x}"));
    }
    format!("sha256:{hex}")
}

/// Write `.agentic-flow/receipts/<nn_step>.json`.
fn write_receipt(receipts_dir: &Path, nn_step: &str, json: &str) {
    std::fs::create_dir_all(receipts_dir).unwrap();
    std::fs::write(receipts_dir.join(format!("{nn_step}.json")), json).unwrap();
}

/// Write `.agentic-flow/state.json`.
fn write_state(repo: &Path, json: &str) {
    let af = repo.join(".agentic-flow");
    std::fs::create_dir_all(&af).unwrap();
    std::fs::write(af.join("state.json"), json).unwrap();
}

/// Write a flow.yaml manifest to an explicit path and return it.
fn write_manifest(tmp_dir: &Path, yaml: &str) -> std::path::PathBuf {
    let p = tmp_dir.join("flow.yaml");
    std::fs::write(&p, yaml).unwrap();
    p
}

/// A "completed + fresh" receipt for a step in a push-trigger repo.
fn fresh_completed_receipt(
    nn_step: &str,
    gating: &str,
    branch: &str,
    base_ref: &str,
    diff_hash: &str,
    user_confirm: bool,
) -> String {
    let uc = if user_confirm {
        r#""user_confirmed": true, "confirmation_id": "c-abc123","#
    } else {
        r#""user_confirmed": false,"#
    };
    format!(
        r#"{{
  "step": "{nn_step}",
  "status": "completed",
  "gating": "{gating}",
  "branch": "{branch}",
  "base_ref": "{base_ref}",
  "head": "deadbeef",
  "diff_hash": "{diff_hash}",
  "verdict": "pass",
  {uc}
  "started_at": "2026-05-08T10:00:00Z",
  "completed_at": "2026-05-08T10:02:00Z",
  "manifest_version": 1
}}"#
    )
}

/// Build a `PluginGateInput` JSON with the given settings object (inlined).
fn gate_input(
    protocol_version: u32,
    kind: &str,
    repo_root: &str,
    base_ref: &str,
    settings_json: &str,
) -> String {
    format!(
        r#"{{
  "protocol_version": {protocol_version},
  "schema_version": 2,
  "trigger": {{ "kind": "{kind}", "files": [] }},
  "config": {{ "type": "agentic-flow", "args": [], "settings": {settings_json} }},
  "repo_root": "{repo_root}",
  "base_ref": "{base_ref}"
}}"#
    )
}

/// Build a settings JSON pinning the manifest path; state + receipts use
/// repo-relative defaults.
fn settings_with_manifest(manifest_path: &Path) -> String {
    format!(r#"{{ "manifest": "{}" }}"#, manifest_path.display())
}

/// Invoke the plugin with args, optional stdin, optional PATH override.
fn run_plugin(args: &[&str], stdin: Option<&str>, path: Option<&str>) -> (i32, String, String) {
    let mut cmd = Command::new(plugin_bin());
    for a in args {
        cmd.arg(a);
    }
    // Ensure git is reachable for the plugin's diff hashing.
    cmd.env("PATH", path.unwrap_or(SYSTEM_PATH));
    // Provide a HOME so `~` expansion is deterministic (not used by these tests
    // since we pin the manifest, but keeps the env predictable).
    cmd.env("HOME", "/tmp");
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("spawn plugin");
    if let Some(input) = stdin {
        child
            .stdin
            .as_mut()
            .expect("piped stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Parse plugin stdout as JSON, panicking with context on failure.
fn parse_out(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("output not valid JSON: {e}\nout={stdout}"))
}

/// Write the four push-required receipts as completed+fresh.
fn write_push_required_fresh(receipts_dir: &Path, branch: &str, base: &str, hash: &str) {
    write_receipt(
        receipts_dir,
        "06-simplify",
        &fresh_completed_receipt("06-simplify", "auto", branch, base, hash, false),
    );
    write_receipt(
        receipts_dir,
        "07-code-review",
        &fresh_completed_receipt("07-code-review", "auto", branch, base, hash, false),
    );
    write_receipt(
        receipts_dir,
        "08-review-handoff",
        &fresh_completed_receipt("08-review-handoff", "auto", branch, base, hash, false),
    );
    // quality-gates is user-confirm in the manifest → needs user_confirmed=true.
    write_receipt(
        receipts_dir,
        "09-quality-gates",
        &fresh_completed_receipt("09-quality-gates", "user-confirm", branch, base, hash, true),
    );
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// 1. `--describe` → protocol_version=0, name, config_types, verdict_v0.
#[test]
fn describe_emits_protocol_v0() {
    let (code, stdout, stderr) = run_plugin(&["--describe"], None, None);
    assert_eq!(code, 0, "exit must be 0; stderr: {stderr}");
    let v = parse_out(&stdout);
    assert_eq!(v["protocol_version"].as_u64(), Some(0));
    assert!(v["name"]
        .as_str()
        .unwrap_or("")
        .starts_with("klasp-plugin-agentic-flow"));
    assert!(v["config_types"]
        .as_array()
        .expect("config_types array")
        .iter()
        .any(|t| t.as_str() == Some("agentic-flow")));
    assert_eq!(v["supports"]["verdict_v0"].as_bool(), Some(true));
}

/// 2. LOAD-BEARING positive: all push-required receipts completed + fresh → pass.
#[test]
fn gate_all_required_fresh_returns_pass() {
    let tmp = TempDir::new().unwrap();
    let (repo, base) = init_git_repo(&tmp);
    let manifest = write_manifest(repo.as_path(), full_manifest());
    let hash = expected_diff_hash(&repo, &base, "push");

    write_state(
        &repo,
        r#"{ "version": 1, "current_step": "quality-gates", "skipped": [], "history": [] }"#,
    );
    let receipts_dir = repo.join(".agentic-flow").join("receipts");
    write_push_required_fresh(&receipts_dir, "feature/thing", &base, &hash);

    let settings = settings_with_manifest(&manifest);
    let input = gate_input(0, "push", &repo.to_string_lossy(), &base, &settings);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), None);
    assert_eq!(code, 0, "exit 0; stderr: {stderr}");
    let v = parse_out(&stdout);
    assert_eq!(
        v["verdict"].as_str(),
        Some("pass"),
        "expected pass; got: {v}"
    );
    assert_eq!(v["findings"].as_array().map(|a| a.len()), Some(0));
}

/// 3. LOAD-BEARING negative: omit 07-code-review → fail + missing-step + resume hint.
#[test]
fn gate_missing_required_receipt_returns_fail_with_resume() {
    let tmp = TempDir::new().unwrap();
    let (repo, base) = init_git_repo(&tmp);
    let manifest = write_manifest(repo.as_path(), full_manifest());
    let hash = expected_diff_hash(&repo, &base, "push");

    write_state(
        &repo,
        r#"{ "version": 1, "current_step": "quality-gates", "skipped": [], "history": [] }"#,
    );
    let receipts_dir = repo.join(".agentic-flow").join("receipts");
    // Write everything EXCEPT 07-code-review.
    write_receipt(
        &receipts_dir,
        "06-simplify",
        &fresh_completed_receipt("06-simplify", "auto", "feature/thing", &base, &hash, false),
    );
    write_receipt(
        &receipts_dir,
        "08-review-handoff",
        &fresh_completed_receipt("08-review-handoff", "auto", "feature/thing", &base, &hash, false),
    );
    write_receipt(
        &receipts_dir,
        "09-quality-gates",
        &fresh_completed_receipt(
            "09-quality-gates",
            "user-confirm",
            "feature/thing",
            &base,
            &hash,
            true,
        ),
    );

    let settings = settings_with_manifest(&manifest);
    let input = gate_input(0, "push", &repo.to_string_lossy(), &base, &settings);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), None);
    assert_eq!(code, 0, "exit 0 even on fail; stderr: {stderr}");
    let v = parse_out(&stdout);
    assert_eq!(v["verdict"].as_str(), Some("fail"), "got: {v}");
    let findings = v["findings"].as_array().expect("findings array");
    let missing = findings
        .iter()
        .find(|f| f["rule"].as_str() == Some("agentic-flow/missing-step"))
        .expect("must have a missing-step finding");
    assert!(
        missing["file"]
            .as_str()
            .unwrap_or("")
            .contains("07-code-review"),
        "missing finding file must reference 07-code-review; got: {missing}"
    );
    // The resume hint leads the earliest error message.
    let any_resume = findings
        .iter()
        .any(|f| f["message"].as_str().unwrap_or("").contains("resume --from"));
    assert!(any_resume, "a finding must carry the resume hint: {findings:?}");
}

/// 4. Stale receipt (wrong diff_hash) → fail + stale-step, resume target = 06.
#[test]
fn gate_stale_receipt_returns_fail() {
    let tmp = TempDir::new().unwrap();
    let (repo, base) = init_git_repo(&tmp);
    let manifest = write_manifest(repo.as_path(), full_manifest());
    let hash = expected_diff_hash(&repo, &base, "push");

    write_state(
        &repo,
        r#"{ "version": 1, "current_step": "quality-gates", "skipped": [], "history": [] }"#,
    );
    let receipts_dir = repo.join(".agentic-flow").join("receipts");
    // 06-simplify gets a WRONG hash → stale.
    write_receipt(
        &receipts_dir,
        "06-simplify",
        &fresh_completed_receipt(
            "06-simplify",
            "auto",
            "feature/thing",
            &base,
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            false,
        ),
    );
    write_receipt(
        &receipts_dir,
        "07-code-review",
        &fresh_completed_receipt("07-code-review", "auto", "feature/thing", &base, &hash, false),
    );
    write_receipt(
        &receipts_dir,
        "08-review-handoff",
        &fresh_completed_receipt("08-review-handoff", "auto", "feature/thing", &base, &hash, false),
    );
    write_receipt(
        &receipts_dir,
        "09-quality-gates",
        &fresh_completed_receipt(
            "09-quality-gates",
            "user-confirm",
            "feature/thing",
            &base,
            &hash,
            true,
        ),
    );

    let settings = settings_with_manifest(&manifest);
    let input = gate_input(0, "push", &repo.to_string_lossy(), &base, &settings);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), None);
    assert_eq!(code, 0, "exit 0; stderr: {stderr}");
    let v = parse_out(&stdout);
    assert_eq!(v["verdict"].as_str(), Some("fail"), "got: {v}");
    let findings = v["findings"].as_array().expect("findings array");
    let stale = findings
        .iter()
        .find(|f| f["rule"].as_str() == Some("agentic-flow/stale-step"))
        .expect("must have a stale-step finding");
    assert!(stale["file"].as_str().unwrap_or("").contains("06-simplify"));
    // Resume target is the earliest failing step = 06.
    let resume = findings
        .iter()
        .find(|f| f["message"].as_str().unwrap_or("").contains("resume --from"))
        .expect("must carry resume hint");
    assert!(
        resume["message"]
            .as_str()
            .unwrap_or("")
            .contains("resume --from 06"),
        "resume target must be 06; got: {resume}"
    );
}

/// 5. commit trigger: 04-log-issue user-confirm but user_confirmed=false → fail.
#[test]
fn gate_unconfirmed_user_confirm_step_fails() {
    let tmp = TempDir::new().unwrap();
    let (repo, base) = init_git_repo(&tmp);
    let manifest = write_manifest(repo.as_path(), full_manifest());
    let hash = expected_diff_hash(&repo, &base, "commit");

    // cursor at log-issue so the user-confirm sweep includes 04-log-issue.
    write_state(
        &repo,
        r#"{ "version": 1, "current_step": "log-issue", "skipped": ["ideate", "feature-dev"], "history": [] }"#,
    );
    let receipts_dir = repo.join(".agentic-flow").join("receipts");
    // dispatch-impl reached (commit impl path satisfied).
    write_receipt(
        &receipts_dir,
        "05-dispatch-impl",
        &fresh_completed_receipt(
            "05-dispatch-impl",
            "user-confirm",
            "feature/thing",
            &base,
            &hash,
            true,
        ),
    );
    // 04-log-issue completed but NOT confirmed.
    write_receipt(
        &receipts_dir,
        "04-log-issue",
        &fresh_completed_receipt("04-log-issue", "user-confirm", "feature/thing", &base, &hash, false),
    );

    let settings = settings_with_manifest(&manifest);
    let input = gate_input(0, "commit", &repo.to_string_lossy(), &base, &settings);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), None);
    assert_eq!(code, 0, "exit 0; stderr: {stderr}");
    let v = parse_out(&stdout);
    assert_eq!(v["verdict"].as_str(), Some("fail"), "got: {v}");
    let findings = v["findings"].as_array().expect("findings array");
    assert!(
        findings
            .iter()
            .any(|f| f["rule"].as_str() == Some("agentic-flow/unconfirmed-step")
                && f["file"].as_str().unwrap_or("").contains("04-log-issue")),
        "must have unconfirmed-step for 04-log-issue: {findings:?}"
    );
}

/// 6. push: 06-simplify status=skipped (legit) → pass (skip != missing).
#[test]
fn gate_legit_skipped_step_passes() {
    let tmp = TempDir::new().unwrap();
    let (repo, base) = init_git_repo(&tmp);
    let manifest = write_manifest(repo.as_path(), full_manifest());
    let hash = expected_diff_hash(&repo, &base, "push");

    write_state(
        &repo,
        r#"{ "version": 1, "current_step": "quality-gates", "skipped": [], "history": [] }"#,
    );
    let receipts_dir = repo.join(".agentic-flow").join("receipts");
    // 06 is legitimately skipped.
    write_receipt(
        &receipts_dir,
        "06-simplify",
        r#"{ "step": "06-simplify", "status": "skipped", "gating": "auto", "skip_reason": "no code-bearing diff" }"#,
    );
    write_receipt(
        &receipts_dir,
        "07-code-review",
        &fresh_completed_receipt("07-code-review", "auto", "feature/thing", &base, &hash, false),
    );
    write_receipt(
        &receipts_dir,
        "08-review-handoff",
        &fresh_completed_receipt("08-review-handoff", "auto", "feature/thing", &base, &hash, false),
    );
    write_receipt(
        &receipts_dir,
        "09-quality-gates",
        &fresh_completed_receipt(
            "09-quality-gates",
            "user-confirm",
            "feature/thing",
            &base,
            &hash,
            true,
        ),
    );

    let settings = settings_with_manifest(&manifest);
    let input = gate_input(0, "push", &repo.to_string_lossy(), &base, &settings);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), None);
    assert_eq!(code, 0, "exit 0; stderr: {stderr}");
    let v = parse_out(&stdout);
    assert_eq!(v["verdict"].as_str(), Some("pass"), "got: {v}");
}

/// 7. manifest has an unknown extra step → warn (unknown-step), NOT fail.
#[test]
fn gate_unknown_manifest_step_warns_not_fails() {
    let tmp = TempDir::new().unwrap();
    let (repo, base) = init_git_repo(&tmp);
    // Append an unknown step to the manifest.
    let yaml = format!(
        "{}  - id: my-custom-step\n    enabled: true\n    gating: auto\n",
        full_manifest()
    );
    let manifest = write_manifest(repo.as_path(), &yaml);
    let hash = expected_diff_hash(&repo, &base, "push");

    write_state(
        &repo,
        r#"{ "version": 1, "current_step": "quality-gates", "skipped": [], "history": [] }"#,
    );
    let receipts_dir = repo.join(".agentic-flow").join("receipts");
    write_push_required_fresh(&receipts_dir, "feature/thing", &base, &hash);

    let settings = settings_with_manifest(&manifest);
    let input = gate_input(0, "push", &repo.to_string_lossy(), &base, &settings);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), None);
    assert_eq!(code, 0, "exit 0; stderr: {stderr}");
    let v = parse_out(&stdout);
    assert_eq!(
        v["verdict"].as_str(),
        Some("warn"),
        "unknown step must warn not fail; got: {v}"
    );
    let findings = v["findings"].as_array().expect("findings array");
    assert!(
        findings
            .iter()
            .any(|f| f["rule"].as_str() == Some("agentic-flow/unknown-step")),
        "must have unknown-step warning: {findings:?}"
    );
}

/// 8. receipts dir absent → warn (infra), exit 0, never fail.
#[test]
fn gate_missing_receipts_dir_returns_warn() {
    let tmp = TempDir::new().unwrap();
    let (repo, base) = init_git_repo(&tmp);
    let manifest = write_manifest(repo.as_path(), full_manifest());
    write_state(&repo, r#"{ "version": 1, "current_step": "quality-gates", "skipped": [] }"#);
    // No receipts dir created.

    let settings = settings_with_manifest(&manifest);
    let input = gate_input(0, "push", &repo.to_string_lossy(), &base, &settings);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), None);
    assert_eq!(code, 0, "exit 0; stderr: {stderr}");
    let v = parse_out(&stdout);
    assert_eq!(v["verdict"].as_str(), Some("warn"), "got: {v}");
    let findings = v["findings"].as_array().expect("findings array");
    assert!(
        findings
            .iter()
            .any(|f| f["rule"].as_str().unwrap_or("").starts_with("klasp-plugin-agentic-flow/")),
        "infra warn rule prefix expected: {findings:?}"
    );
}

/// 9. malformed receipt JSON → warn, exit 0.
#[test]
fn gate_malformed_receipt_json_returns_warn() {
    let tmp = TempDir::new().unwrap();
    let (repo, base) = init_git_repo(&tmp);
    let manifest = write_manifest(repo.as_path(), full_manifest());
    let hash = expected_diff_hash(&repo, &base, "push");
    write_state(
        &repo,
        r#"{ "version": 1, "current_step": "quality-gates", "skipped": [] }"#,
    );
    let receipts_dir = repo.join(".agentic-flow").join("receipts");
    // Everything fresh except one broken JSON file.
    write_push_required_fresh(&receipts_dir, "feature/thing", &base, &hash);
    write_receipt(&receipts_dir, "07-code-review", "{ this is not valid json ");

    let settings = settings_with_manifest(&manifest);
    let input = gate_input(0, "push", &repo.to_string_lossy(), &base, &settings);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), None);
    assert_eq!(code, 0, "exit 0; stderr: {stderr}");
    let v = parse_out(&stdout);
    // The malformed 07 receipt → both a warn (parse) AND a missing-step error
    // because a broken required receipt cannot silently satisfy. The infra warn
    // must be present; the verdict is fail because 07 is now effectively missing.
    let findings = v["findings"].as_array().expect("findings array");
    assert!(
        findings
            .iter()
            .any(|f| f["rule"].as_str() == Some("klasp-plugin-agentic-flow/receipt-parse-error")),
        "must surface a receipt-parse-error warn: {findings:?}"
    );
}

/// 10. malformed stdin → warn + exit 0 + input-parse-error.
#[test]
fn gate_malformed_stdin_returns_warn_and_exits_zero() {
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some("{not valid json"), None);
    assert_eq!(code, 0, "exit 0 on bad stdin; stderr: {stderr}");
    let v = parse_out(&stdout);
    assert_eq!(v["verdict"].as_str(), Some("warn"));
    assert!(v["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .any(|f| f["rule"].as_str().unwrap_or("").contains("input-parse-error")));
}

/// 11. empty stdin → warn + exit 0.
#[test]
fn gate_empty_stdin_returns_warn_and_exits_zero() {
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(""), None);
    assert_eq!(code, 0, "exit 0 on empty stdin; stderr: {stderr}");
    let v = parse_out(&stdout);
    assert_eq!(v["verdict"].as_str(), Some("warn"));
}

/// 12. unknown protocol_version=99 → best-effort warn, NOT fail.
#[test]
fn gate_unknown_protocol_version_warns_not_fails() {
    let tmp = TempDir::new().unwrap();
    let (repo, base) = init_git_repo(&tmp);
    let manifest = write_manifest(repo.as_path(), full_manifest());
    let hash = expected_diff_hash(&repo, &base, "push");
    write_state(
        &repo,
        r#"{ "version": 1, "current_step": "quality-gates", "skipped": [] }"#,
    );
    let receipts_dir = repo.join(".agentic-flow").join("receipts");
    write_push_required_fresh(&receipts_dir, "feature/thing", &base, &hash);

    let settings = settings_with_manifest(&manifest);
    let input = gate_input(99, "push", &repo.to_string_lossy(), &base, &settings);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), None);
    assert_eq!(code, 0, "exit 0; stderr: {stderr}");
    let v = parse_out(&stdout);
    // Everything is fresh, so only the protocol-warn finding remains → warn.
    assert_eq!(v["verdict"].as_str(), Some("warn"), "got: {v}");
    let findings = v["findings"].as_array().expect("findings array");
    assert!(
        findings
            .iter()
            .any(|f| f["rule"].as_str().unwrap_or("").contains("protocol")),
        "must have a protocol-warn finding: {findings:?}"
    );
}

/// 13. repo_root pointed at a non-git dir → git diff fails → warn + exit 0.
#[test]
fn gate_git_diff_unavailable_returns_warn() {
    // Build a manifest + state + a completed receipt in a NON-git temp dir.
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();
    let manifest = write_manifest(&repo, full_manifest());
    write_state(
        &repo,
        r#"{ "version": 1, "current_step": "quality-gates", "skipped": [] }"#,
    );
    let receipts_dir = repo.join(".agentic-flow").join("receipts");
    // A completed receipt with some hash; since git diff fails, staleness can't
    // be proven, so the step is treated as fresh (not stale) and no FAIL is
    // produced from staleness. The plugin must still exit 0 with no crash.
    write_push_required_fresh(&receipts_dir, "feature/thing", "origin/main", "sha256:abc");

    let settings = settings_with_manifest(&manifest);
    let input = gate_input(0, "push", &repo.to_string_lossy(), "origin/main", &settings);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), None);
    assert_eq!(code, 0, "exit 0 when git unavailable; stderr: {stderr}");
    let v = parse_out(&stdout);
    // Verdict must be pass or warn — never fail purely from a git-diff failure.
    let verdict = v["verdict"].as_str().unwrap_or("");
    assert!(
        verdict == "pass" || verdict == "warn",
        "git failure must not produce fail; got: {v}"
    );
}

/// 14. DIFF_HASH PARITY: the plugin's `canonical_diff_hash` recipe and the test
///     helper must produce byte-identical hashes. We assert this indirectly: a
///     receipt whose diff_hash == the test-computed hash makes the gate PASS,
///     and flipping a single byte makes it FAIL. This is the regression guard
///     that writer and auditor stay byte-identical. (Test 2 already exercises
///     the positive direction; this test exercises both directions explicitly.)
#[test]
fn diff_hash_parity() {
    let tmp = TempDir::new().unwrap();
    let (repo, base) = init_git_repo(&tmp);
    let manifest = write_manifest(repo.as_path(), full_manifest());
    let hash = expected_diff_hash(&repo, &base, "push");

    write_state(
        &repo,
        r#"{ "version": 1, "current_step": "quality-gates", "skipped": [] }"#,
    );
    let receipts_dir = repo.join(".agentic-flow").join("receipts");

    // Positive: exact hash → pass.
    write_push_required_fresh(&receipts_dir, "feature/thing", &base, &hash);
    let settings = settings_with_manifest(&manifest);
    let input = gate_input(0, "push", &repo.to_string_lossy(), &base, &settings);
    let (_, stdout, _) = run_plugin(&["--gate"], Some(&input), None);
    assert_eq!(parse_out(&stdout)["verdict"].as_str(), Some("pass"));

    // Negative: flip one hex char in 06 → stale → fail.
    let mut flipped: Vec<char> = hash.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == '0' { '1' } else { '0' };
    let bad: String = flipped.into_iter().collect();
    write_receipt(
        &receipts_dir,
        "06-simplify",
        &fresh_completed_receipt("06-simplify", "auto", "feature/thing", &base, &bad, false),
    );
    let (_, stdout2, _) = run_plugin(&["--gate"], Some(&input), None);
    assert_eq!(
        parse_out(&stdout2)["verdict"].as_str(),
        Some("fail"),
        "a one-char hash change must make the receipt stale"
    );
}
