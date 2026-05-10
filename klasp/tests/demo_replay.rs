//! Integration tests for `klasp demo` — feedback-loop replay verification.
//!
//! Each test writes an inline fixture to a tempfile and runs `klasp demo
//! --fixture <path>`, then asserts on exit code and stdout/stderr content.

use std::io::Write;
use std::process::Command;

fn klasp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp")
}

/// Write `content` to a named tempfile and return the path. The file is
/// kept alive for the duration of the test via the returned `tempfile::NamedTempFile`.
fn fixture_file(content: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().expect("create tempfile");
    f.write_all(content.as_bytes()).expect("write fixture");
    f
}

/// Run `klasp demo --fixture <path>` and return `(exit_code, stdout, stderr)`.
fn run_demo(fixture_path: &std::path::Path, extra_args: &[&str]) -> (i32, String, String) {
    let out = Command::new(klasp_bin())
        .arg("demo")
        .arg("--fixture")
        .arg(fixture_path)
        .args(extra_args)
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn klasp demo");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let code = out.status.code().unwrap_or(-1);
    (code, stdout, stderr)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

/// Fixture with one gate-block sequence where the assistant message correctly
/// references the filename from the verdict. Expect exit 0.
#[test]
fn demo_replay_feedback_loop_verified() {
    let fixture = fixture_file(
        r#"{"type":"message","role":"user","content":"Stage and commit."}
{"type":"tool_call","id":"tc_001","name":"Bash","input":{"command":"git add -A && git commit -m 'wip'"}}
{"type":"tool_result","tool_call_id":"tc_001","output":"klasp-gate: blocked (1 errors, 1 findings total, policy=AnyFail):\n  - [error][tsc] src/lib/foo.ts:3:1 — Cannot find module 'lodash'","exit_code":1}
{"type":"message","role":"assistant","content":"Gate blocked. Fixing src/lib/foo.ts:3 — removing the lodash import."}
"#,
    );

    let (code, stdout, _stderr) = run_demo(fixture.path(), &[]);
    assert_eq!(
        code, 0,
        "expected exit 0 when assistant references the blocked filename\nstdout: {stdout}",
    );
    assert!(
        stdout.contains("demo replay: OK"),
        "expected 'demo replay: OK' in stdout\nstdout: {stdout}",
    );
    assert!(
        stdout.contains("1 gate-feedback loop"),
        "expected count of verified loops in stdout\nstdout: {stdout}",
    );
}

/// Fixture where the assistant message after a gate block does NOT reference
/// the filename. Expect exit 1.
#[test]
fn demo_replay_missing_reference_fails() {
    let fixture = fixture_file(
        r#"{"type":"message","role":"user","content":"Stage and commit."}
{"type":"tool_call","id":"tc_001","name":"Bash","input":{"command":"git add -A && git commit -m 'wip'"}}
{"type":"tool_result","tool_call_id":"tc_001","output":"klasp-gate: blocked (1 errors, 1 findings total, policy=AnyFail):\n  - [error][tsc] src/lib/foo.ts:3:1 — Cannot find module 'lodash'","exit_code":1}
{"type":"message","role":"assistant","content":"I'll fix the issues and try again."}
"#,
    );

    let (code, _stdout, stderr) = run_demo(fixture.path(), &[]);
    assert_eq!(
        code, 1,
        "expected exit 1 when assistant does not reference the blocked filename\nstderr: {stderr}",
    );
    assert!(
        stderr.contains("demo replay: FAIL"),
        "expected 'demo replay: FAIL' in stderr\nstderr: {stderr}",
    );
    assert!(
        stderr.contains("src/lib/foo.ts"),
        "expected missing filename in the FAIL message\nstderr: {stderr}",
    );
}

/// Fixture with no gate-block sequences at all. Expect exit 0 with warning.
#[test]
fn demo_replay_no_gate_blocks_is_ok() {
    let fixture = fixture_file(
        r#"{"type":"message","role":"user","content":"Stage and commit."}
{"type":"tool_call","id":"tc_001","name":"Bash","input":{"command":"git add -A && git commit -m 'wip'"}}
{"type":"tool_result","tool_call_id":"tc_001","output":"[main abc1234] wip\n 1 file changed","exit_code":0}
{"type":"message","role":"assistant","content":"Commit succeeded."}
"#,
    );

    let (code, stdout, _stderr) = run_demo(fixture.path(), &[]);
    assert_eq!(
        code, 0,
        "expected exit 0 when no gate blocks are present\nstdout: {stdout}",
    );
    assert!(
        stdout.contains("no klasp-gate blocked sequences found"),
        "expected advisory warning in stdout\nstdout: {stdout}",
    );
}

/// Fixture with a malformed JSON line. Expect exit 1.
#[test]
fn demo_replay_malformed_json_fails() {
    let fixture = fixture_file(
        r#"{"type":"message","role":"user","content":"ok"}
not-valid-json
{"type":"message","role":"assistant","content":"done"}
"#,
    );

    let (code, _stdout, stderr) = run_demo(fixture.path(), &[]);
    assert_eq!(
        code, 1,
        "expected exit 1 on malformed JSON\nstderr: {stderr}",
    );
    assert!(
        stderr.contains("malformed JSON") || stderr.contains("fixture"),
        "expected error message in stderr\nstderr: {stderr}",
    );
}

/// Verify using the pre-written passing fixture from the fixtures directory.
#[test]
fn demo_replay_passing_fixture_file() {
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/demo/claude-feedback-loop.jsonl");

    let (code, stdout, _stderr) = run_demo(&fixture_path, &[]);
    assert_eq!(
        code, 0,
        "expected exit 0 for the pre-written passing fixture\nstdout: {stdout}",
    );
    assert!(
        stdout.contains("demo replay: OK"),
        "expected OK in stdout\nstdout: {stdout}",
    );
}

/// Verify using the pre-written missing-ref fixture from the fixtures directory.
#[test]
fn demo_replay_missing_ref_fixture_file() {
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/demo/claude-feedback-loop-missing-ref.jsonl");

    let (code, _stdout, stderr) = run_demo(&fixture_path, &[]);
    assert_eq!(
        code, 1,
        "expected exit 1 for the pre-written missing-ref fixture\nstderr: {stderr}",
    );
    assert!(
        stderr.contains("demo replay: FAIL"),
        "expected FAIL in stderr\nstderr: {stderr}",
    );
}

/// --verbose flag produces step-by-step output without changing the exit code.
#[test]
fn demo_replay_verbose_flag_works() {
    let fixture = fixture_file(
        r#"{"type":"message","role":"user","content":"Stage and commit."}
{"type":"tool_call","id":"tc_001","name":"Bash","input":{"command":"git add -A && git commit -m 'wip'"}}
{"type":"tool_result","tool_call_id":"tc_001","output":"klasp-gate: blocked (1 errors, 1 findings total, policy=AnyFail):\n  - [error][tsc] src/lib/foo.ts:3:1 — Cannot find module 'lodash'","exit_code":1}
{"type":"message","role":"assistant","content":"Gate blocked. Fixing src/lib/foo.ts:3 — removing the lodash import."}
"#,
    );

    let (code, stdout, _stderr) = run_demo(fixture.path(), &["--verbose"]);
    assert_eq!(code, 0, "verbose mode must not change exit code\nstdout: {stdout}");
    assert!(
        stdout.contains("gate block"),
        "expected verbose gate block output\nstdout: {stdout}",
    );
}
