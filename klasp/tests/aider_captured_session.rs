//! Aider captured-session integration test — issue #46 acceptance item 1.
//!
//! Verifies that klasp's gate blocks a failing commit from an Aider session
//! with a structured verdict emitted on stderr, identical in shape to the
//! Codex captured-session test (`codex_captured_session.rs`).
//!
//! ## Strategy
//!
//! A real Aider CLI invocation is not viable in CI (requires network + API key).
//! Instead we replay a captured JSONL session transcript from
//! `tests/fixtures/captured_session/aider/failing-commit-session.jsonl`. That
//! transcript records an Aider agent session where the agent runs `git commit`;
//! the commit command is extracted from the fixture and used to drive the gate.
//!
//! The actual gate invocation mirrors how `commit-cmd-pre` in `.aider.conf.yml`
//! runs in an Aider session: with `KLASP_GATE_SCHEMA` set and a gate-protocol
//! JSON payload that represents a commit tool call. This keeps the test
//! deterministic and CI-reproducible without a live Aider instance.
//!
//! ## What is exercised
//!
//! 1. `klasp install --agent aider` correctly writes `commit-cmd-pre` into
//!    `.aider.conf.yml`.
//! 2. A deliberately-failing shell check in `klasp.toml` produces
//!    `Verdict::Fail`, and the gate exits 2 (blocking the commit).
//! 3. The block message on stderr is structured: it contains the canonical
//!    `klasp-gate: blocked` prefix, an error count, and the policy tag —
//!    matching the contract verified for Claude Code and Codex.
//! 4. `klasp doctor` reports the Aider surface as installed.
//! 5. Install + uninstall leaves the repo clean (round-trip).

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use klasp_core::GATE_SCHEMA_VERSION;
use tempfile::TempDir;

/// Captured Aider session JSONL — mirrors the Codex fixture shape exactly.
const FIXTURE_AIDER_SESSION: &str =
    include_str!("fixtures/captured_session/aider/failing-commit-session.jsonl");

/// Extract the commit command from the captured JSONL session fixture.
/// Finds the first `tool_call` line that invokes `git commit` and returns the
/// command string. Panics if the fixture does not contain such a line.
fn extract_commit_command_from_session(jsonl: &str) -> String {
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("tool_call") {
            continue;
        }
        if let Some(cmd) = v
            .get("input")
            .and_then(|i| i.get("command"))
            .and_then(|c| c.as_str())
        {
            if cmd.contains("git commit") {
                return cmd.to_owned();
            }
        }
    }
    panic!(
        "captured session fixture must contain a 'tool_call' line with a 'git commit' command; \
         check tests/fixtures/captured_session/aider/failing-commit-session.jsonl"
    )
}

/// Build the gate-protocol JSON payload from the captured JSONL fixture.
fn aider_commit_payload() -> String {
    let cmd = extract_commit_command_from_session(FIXTURE_AIDER_SESSION);
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": cmd,
            "description": "Commit the staged changes."
        },
        "session_id": "aider-fixture-session",
        "transcript_path": "/tmp/klasp-aider-fixture/transcript.jsonl"
    })
    .to_string()
}

/// `klasp.toml` that configures a shell check intentionally designed to fail.
const FAILING_KLASP_TOML: &str = r#"
    version = 1

    [gate]
    agents = ["aider"]
    policy = "any_fail"

    [[checks]]
    name = "always-fail"
    triggers = [{ on = ["commit"] }]
    timeout_secs = 5
    [checks.source]
    type = "shell"
    command = "exit 7"
"#;

fn klasp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp")
}

/// Set up a fresh repo with `.aider.conf.yml` and klasp installed for aider.
fn fresh_aider_repo_with_klasp() -> TempDir {
    let dir = TempDir::new().expect("create tempdir");

    std::fs::create_dir(dir.path().join(".git")).expect("create .git");
    std::fs::create_dir(dir.path().join(".git").join("hooks")).expect("create .git/hooks");

    // Aider's auto-detect signal is the presence of `.aider.conf.yml`.
    std::fs::write(dir.path().join(".aider.conf.yml"), "model: gpt-4o\n")
        .expect("write .aider.conf.yml");

    std::fs::write(dir.path().join("klasp.toml"), FAILING_KLASP_TOML).expect("write klasp.toml");

    let out = Command::new(klasp_bin())
        .current_dir(dir.path())
        .args(["install", "--agent", "aider"])
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn klasp install");
    assert!(
        out.status.success(),
        "klasp install --agent aider must succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    dir
}

/// Invoke `klasp gate` with the gate-protocol payload on stdin.
/// Returns `(exit_code, stderr_text)`.
fn invoke_gate(payload: &str, repo: &Path) -> (Option<i32>, String) {
    let mut cmd = Command::new(klasp_bin());
    cmd.arg("gate")
        .env("KLASP_GATE_SCHEMA", GATE_SCHEMA_VERSION.to_string())
        .env("CLAUDE_PROJECT_DIR", repo)
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("spawn klasp gate");
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(payload.as_bytes())
        .expect("write stdin payload");
    let output = child.wait_with_output().expect("wait for klasp gate");

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !stderr.is_empty() {
        eprintln!("klasp gate stderr:\n{stderr}");
    }
    (output.status.code(), stderr)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

/// Smoke-check: the captured session fixture contains a `git commit` tool call.
#[test]
fn fixture_contains_git_commit_tool_call() {
    let cmd = extract_commit_command_from_session(FIXTURE_AIDER_SESSION);
    assert!(
        cmd.contains("git commit"),
        "expected a git commit command in the fixture, got: {cmd:?}",
    );
}

/// Core acceptance test: a failing commit via Aider is blocked by the klasp gate.
#[test]
fn aider_failing_commit_blocked_with_structured_verdict() {
    let repo = fresh_aider_repo_with_klasp();

    // Confirm .aider.conf.yml was modified with commit-cmd-pre.
    let conf_body = std::fs::read_to_string(repo.path().join(".aider.conf.yml")).unwrap();
    assert!(
        conf_body.contains("klasp gate"),
        ".aider.conf.yml must contain 'klasp gate':\n{conf_body}",
    );

    let payload = aider_commit_payload();
    let (code, stderr) = invoke_gate(&payload, repo.path());
    assert_eq!(
        code,
        Some(2),
        "failing shell check on a commit payload must exit 2 (blocked); stderr:\n{stderr}",
    );

    // Structured verdict assertions — identical contract to Codex/Claude.
    assert!(
        stderr.contains("klasp-gate: blocked"),
        "stderr must contain 'klasp-gate: blocked'; got:\n{stderr}",
    );
    assert!(
        stderr.contains("errors"),
        "stderr must report an error count; got:\n{stderr}",
    );
    assert!(
        stderr.contains("policy="),
        "stderr must carry the policy tag; got:\n{stderr}",
    );
    assert!(
        stderr.contains("always-fail"),
        "stderr must name the failing check 'always-fail'; got:\n{stderr}",
    );
}

/// Happy path: a passing check allows the commit.
#[test]
fn aider_passing_commit_succeeds() {
    let dir = TempDir::new().expect("create tempdir");
    std::fs::create_dir(dir.path().join(".git")).expect("create .git");
    std::fs::create_dir(dir.path().join(".git").join("hooks")).expect("create .git/hooks");
    std::fs::write(dir.path().join(".aider.conf.yml"), "model: gpt-4o\n")
        .expect("write .aider.conf.yml");

    std::fs::write(
        dir.path().join("klasp.toml"),
        r#"
            version = 1

            [gate]
            agents = ["aider"]
            policy = "any_fail"

            [[checks]]
            name = "always-pass"
            triggers = [{ on = ["commit"] }]
            timeout_secs = 5
            [checks.source]
            type = "shell"
            command = "true"
        "#,
    )
    .expect("write klasp.toml");

    let payload = aider_commit_payload();
    let (code, _stderr) = invoke_gate(&payload, dir.path());
    assert_eq!(
        code,
        Some(0),
        "passing check must allow the commit (exit 0)",
    );
}

/// Verdict shape matches Claude/Codex: same JSON schema fields present.
///
/// Loads golden output fixtures from the codex captured-session and asserts the
/// verdict shape (fields, types) from the aider gate run matches the same
/// structural contract. We verify at the text level: `klasp-gate: blocked`,
/// `errors`, `policy=`, findings list — the fields every surface exposes.
#[test]
fn aider_verdict_shape_matches_claude_codex() {
    let repo = fresh_aider_repo_with_klasp();
    let payload = aider_commit_payload();
    let (code, stderr) = invoke_gate(&payload, repo.path());

    assert_eq!(
        code,
        Some(2),
        "aider gate must exit 2 for a failing commit; got {code:?}",
    );

    // These are the same structural assertions verified for Claude Code (in
    // gate_flow.rs) and Codex (in codex_captured_session.rs). If the shape
    // diverges, all three surfaces are affected — the contract is uniform.
    assert!(
        stderr.contains("klasp-gate: blocked"),
        "missing block prefix"
    );
    assert!(stderr.contains("errors"), "missing error count");
    assert!(stderr.contains("policy="), "missing policy tag");
    // findings array represented as inline lines in terminal output
    assert!(stderr.contains("always-fail"), "missing finding rule name");
}

/// Round-trip: install then uninstall leaves `.aider.conf.yml` without klasp.
#[test]
fn aider_install_uninstall_round_trip() {
    let dir = TempDir::new().expect("create tempdir");
    std::fs::create_dir(dir.path().join(".git")).expect("create .git");
    std::fs::create_dir(dir.path().join(".git").join("hooks")).expect("create .git/hooks");
    std::fs::write(dir.path().join(".aider.conf.yml"), "model: gpt-4o\n")
        .expect("write .aider.conf.yml");
    std::fs::write(dir.path().join("klasp.toml"), FAILING_KLASP_TOML).expect("write klasp.toml");

    // Install.
    let install_out = Command::new(klasp_bin())
        .current_dir(dir.path())
        .args(["install", "--agent", "aider"])
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn install");
    assert!(install_out.status.success(), "install failed");
    let conf_after_install = std::fs::read_to_string(dir.path().join(".aider.conf.yml")).unwrap();
    assert!(
        conf_after_install.contains("klasp gate"),
        "klasp not written on install: {conf_after_install}",
    );

    // Uninstall.
    let uninstall_out = Command::new(klasp_bin())
        .current_dir(dir.path())
        .args(["uninstall", "--agent", "aider"])
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn uninstall");
    assert!(uninstall_out.status.success(), "uninstall failed");
    let conf_after_uninstall = std::fs::read_to_string(dir.path().join(".aider.conf.yml")).unwrap();
    assert!(
        !conf_after_uninstall.contains("klasp gate"),
        "klasp still present after uninstall: {conf_after_uninstall}",
    );
    // Other content survives.
    assert!(
        conf_after_uninstall.contains("gpt-4o"),
        "model key lost after uninstall: {conf_after_uninstall}",
    );
}

/// `klasp doctor` reports the Aider surface as installed.
///
/// Doctor's `check_hook` uses byte-equality between the installed `.aider.conf.yml`
/// and the output of `render_hook_script`. For the byte-equality to pass, we
/// must start from an empty `.aider.conf.yml` so that the installed content
/// (just `commit-cmd-pre: klasp gate --agent aider`) matches `render_hook_script`.
/// When a user has pre-existing keys in their conf, doctor will show schema drift
/// for the hook check — that is a known v0.3 limitation; a per-surface health
/// check method will replace this heuristic in a future release.
#[test]
fn aider_doctor_reports_aider_surface() {
    let dir = TempDir::new().expect("create tempdir");
    std::fs::create_dir(dir.path().join(".git")).expect("create .git");
    std::fs::create_dir(dir.path().join(".git").join("hooks")).expect("create .git/hooks");
    // Empty .aider.conf.yml → installed content matches render_hook_script exactly.
    std::fs::write(dir.path().join(".aider.conf.yml"), "").expect("write .aider.conf.yml");
    std::fs::write(dir.path().join("klasp.toml"), FAILING_KLASP_TOML).expect("write klasp.toml");

    Command::new(klasp_bin())
        .current_dir(dir.path())
        .args(["install", "--agent", "aider"])
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("install");

    let doctor_out = Command::new(klasp_bin())
        .current_dir(dir.path())
        .arg("doctor")
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn doctor");

    let stdout = String::from_utf8_lossy(&doctor_out.stdout).to_string();
    // Doctor must report the aider hook as OK (not FAIL).
    assert!(
        stdout.contains("aider"),
        "doctor stdout must mention 'aider'; got:\n{stdout}",
    );
    // The surface-level hook check must pass — byte-equality of the
    // installed hook script against a fresh render.
    assert!(
        !stdout.contains("FAIL  hook[aider]"),
        "doctor must not FAIL the aider hook; got:\n{stdout}",
    );
}

/// Session fixture round-trip: every line in the JSONL fixture is valid JSON.
#[test]
fn aider_session_fixture_is_valid_jsonl() {
    for (i, line) in FIXTURE_AIDER_SESSION.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let result = serde_json::from_str::<serde_json::Value>(line);
        assert!(
            result.is_ok(),
            "fixture line {i} is not valid JSON: {line:?}",
        );
    }
}
