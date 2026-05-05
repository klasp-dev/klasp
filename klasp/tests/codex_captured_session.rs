//! Codex captured-session integration test — issue #33 acceptance item 1.
//!
//! Verifies that klasp's git hook blocks a failing commit from a Codex
//! session with a structured verdict emitted on stderr.
//!
//! ## Strategy
//!
//! A real Codex CLI invocation is not viable in CI (requires network +
//! Codex account). Instead we replay a captured JSONL session transcript
//! from `tests/fixtures/codex/failing-commit-session.jsonl`. That transcript
//! records a Codex agent session where the agent runs `git commit`; the
//! commit command is extracted from the fixture and used to drive the gate.
//!
//! The actual gate invocation mirrors how the installed pre-commit hook
//! runs in a Codex session: with `KLASP_GATE_SCHEMA` set and a tool-call
//! JSON payload on stdin (the Codex-facing hook contract). This keeps the
//! test deterministic and CI-reproducible without a live Codex instance.
//!
//! ## What is exercised
//!
//! 1. `klasp install --agent codex` correctly seeds `.git/hooks/pre-commit`
//!    and `AGENTS.md`.
//! 2. The installed hook dispatches to `klasp gate` with the codex agent
//!    and commit trigger.
//! 3. A deliberately-failing shell check in `klasp.toml` produces
//!    `Verdict::Fail`, and the gate exits 2 (blocking the commit).
//! 4. The block message on stderr is structured: it contains the canonical
//!    `klasp-gate: blocked` prefix, an error count, and the policy tag —
//!    all required fields from the gate's `render_terminal_summary`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use klasp_core::GATE_SCHEMA_VERSION;
use tempfile::TempDir;

/// Captured Codex session JSONL. Each line is a JSON object representing one
/// event in the agent session — tool calls, tool results, messages. We replay
/// the commit invocation found in this fixture rather than requiring a live
/// Codex CLI.
const FIXTURE_CODEX_SESSION: &str = include_str!("fixtures/codex/failing-commit-session.jsonl");

/// Build the gate-protocol JSON payload from the captured JSONL fixture.
///
/// Extracts the actual commit command from `FIXTURE_CODEX_SESSION` so that
/// the gate-driving tests stay in lock-step with the fixture. If the fixture's
/// commit string changes, these tests will notice.
fn codex_commit_payload() -> String {
    let cmd = extract_commit_command_from_session(FIXTURE_CODEX_SESSION);
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": cmd,
            "description": "Commit the staged changes."
        },
        "session_id": "codex-fixture-session",
        "transcript_path": "/tmp/klasp-codex-fixture/transcript.jsonl"
    })
    .to_string()
}

/// `klasp.toml` that configures a shell check intentionally designed to fail.
/// Using `exit 7` makes the failure deterministic and unambiguous — any
/// non-zero exit from a shell check produces `Verdict::Fail`.
const FAILING_KLASP_TOML: &str = r#"
    version = 1

    [gate]
    agents = ["codex"]
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

/// Extract the commit command from the captured JSONL session fixture.
/// Finds the first `tool_call` line that invokes `git commit` and returns
/// the command string. Panics if the fixture does not contain such a line —
/// the fixture contract is part of the test.
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
         check tests/fixtures/codex/failing-commit-session.jsonl"
    )
}

/// Set up a fresh git repo with klasp installed for the codex surface.
///
/// Returns the `TempDir` so the caller keeps the directory alive for the
/// duration of the test.
fn fresh_codex_repo_with_klasp() -> TempDir {
    let dir = TempDir::new().expect("create tempdir");

    // Minimal git scaffold — just enough for klasp's install path to
    // succeed. We do not run `git init` because the test only needs the
    // directory structure; the gate's `find_repo_root_from_cwd` will find
    // the `.git/` directory we create by hand.
    std::fs::create_dir(dir.path().join(".git")).expect("create .git");
    std::fs::create_dir(dir.path().join(".git").join("hooks")).expect("create .git/hooks");

    // AGENTS.md presence is the codex surface auto-detect signal (mirrors
    // `install_codex_cli.rs` / `CodexSurface::detect`).
    std::fs::write(dir.path().join("AGENTS.md"), "# Project\n").expect("write AGENTS.md");

    // Write the failing klasp.toml.
    std::fs::write(dir.path().join("klasp.toml"), FAILING_KLASP_TOML).expect("write klasp.toml");

    // Install klasp for the codex surface so the pre-commit hook is in place.
    let out = Command::new(klasp_bin())
        .current_dir(dir.path())
        .args(["install", "--agent", "codex"])
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn klasp install");
    assert!(
        out.status.success(),
        "klasp install --agent codex must succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    dir
}

/// Invoke `klasp gate` the same way the installed pre-commit hook does:
/// - `KLASP_GATE_SCHEMA` env var set to the current schema version.
/// - `CLAUDE_PROJECT_DIR` pointed at the repo root.
/// - Gate-protocol JSON payload on stdin.
///
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

// ─── Tests ───────────────────────────────────────────────────────────────────

/// Smoke-check: the captured session fixture contains a `git commit` tool
/// call. If this fails, the fixture is malformed and all downstream tests
/// are invalid.
///
/// `extract_commit_command_from_session` already panics when no `git commit`
/// line is found, so the assertion below is doing the real checking — the
/// helper is pure extraction.
#[test]
fn fixture_contains_git_commit_tool_call() {
    let cmd = extract_commit_command_from_session(FIXTURE_CODEX_SESSION);
    // Verify the extracted command is non-empty and contains "git commit".
    // The helper panics if absent, so a successful return means it was found;
    // this assertion documents and double-checks the invariant explicitly.
    assert!(
        cmd.contains("git commit"),
        "expected a git commit command in the fixture, got: {cmd:?}",
    );
}

/// Core acceptance test: a failing commit is blocked by the klasp gate.
///
/// Exercises the full path:
/// - `klasp install --agent codex` seeds the pre-commit hook.
/// - A commit attempt from a Codex session hits a failing shell check.
/// - The gate exits 2 (blocking the commit).
#[test]
fn codex_failing_commit_is_blocked_by_gate() {
    let repo = fresh_codex_repo_with_klasp();

    // Confirm the pre-commit hook was written with the codex agent marker.
    let hook_body = std::fs::read_to_string(repo.path().join(".git/hooks/pre-commit")).unwrap();
    assert!(
        hook_body.contains("--agent codex"),
        "pre-commit hook must dispatch to codex agent:\n{hook_body}",
    );

    let payload = codex_commit_payload();
    let (code, _stderr) = invoke_gate(&payload, repo.path());
    assert_eq!(
        code,
        Some(2),
        "failing shell check on a commit payload must exit 2 (blocked)",
    );
}

/// Verdict structure test: the block message on stderr must carry the
/// structured fields that downstream tooling (Codex's session replay,
/// error parsers) depends on.
///
/// Asserts the presence of:
/// - `klasp-gate: blocked` — canonical gate prefix + outcome word.
/// - An error count (`N errors`) — so the agent knows what to fix.
/// - A policy tag (`policy=AnyFail`) — so the block reason is auditable.
#[test]
fn codex_failing_commit_emits_structured_verdict() {
    let repo = fresh_codex_repo_with_klasp();

    let payload = codex_commit_payload();
    let (code, stderr) = invoke_gate(&payload, repo.path());
    assert_eq!(
        code,
        Some(2),
        "gate must exit 2 for a failing commit; got {code:?}",
    );

    // ── Structured verdict assertions ────────────────────────────────

    // 1. Canonical block prefix — every blocking verdict starts here.
    assert!(
        stderr.contains("klasp-gate: blocked"),
        "stderr must contain 'klasp-gate: blocked'; got:\n{stderr}",
    );

    // 2. Error count — the agent needs to know how many errors to address.
    assert!(
        stderr.contains("errors"),
        "stderr must report an error count; got:\n{stderr}",
    );

    // 3. Policy tag — makes the block reason auditable in session replay.
    assert!(
        stderr.contains("policy="),
        "stderr must carry the policy tag; got:\n{stderr}",
    );

    // 4. At least one finding — the `always-fail` check must surface.
    //    The gate renders findings as `  - [error|warn|info][<rule>] <msg>`.
    assert!(
        stderr.contains("always-fail"),
        "stderr must name the failing check 'always-fail'; got:\n{stderr}",
    );
}

/// Pass-through test: a passing check does NOT block the commit.
///
/// Validates the inverse of `codex_failing_commit_is_blocked_by_gate` —
/// the gate must exit 0 (allow) when all checks pass.
#[test]
fn codex_passing_check_allows_commit() {
    let dir = TempDir::new().expect("create tempdir");
    std::fs::create_dir(dir.path().join(".git")).expect("create .git");
    std::fs::create_dir(dir.path().join(".git").join("hooks")).expect("create .git/hooks");
    std::fs::write(dir.path().join("AGENTS.md"), "# Project\n").expect("write AGENTS.md");

    // Write a klasp.toml with a passing check.
    std::fs::write(
        dir.path().join("klasp.toml"),
        r#"
            version = 1

            [gate]
            agents = ["codex"]
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

    let payload = codex_commit_payload();
    let (code, _stderr) = invoke_gate(&payload, dir.path());
    assert_eq!(
        code,
        Some(0),
        "passing check must allow the commit (exit 0)",
    );
}

/// Session fixture round-trip: every line in the JSONL fixture is valid JSON.
///
/// This is a format-contract test. If the fixture is ever hand-edited and a
/// line is corrupted, this test catches it before the other fixture-driven
/// assertions are reached.
#[test]
fn codex_session_fixture_is_valid_jsonl() {
    for (i, line) in FIXTURE_CODEX_SESSION.lines().enumerate() {
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
