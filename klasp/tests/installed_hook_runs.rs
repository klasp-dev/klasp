//! Regression tests for issue #91: installed hooks must not be rejected by clap.
//!
//! Before the fix, `klasp gate --agent <X>` and `klasp gate --trigger <commit|push>`
//! caused clap to exit 2 ("unexpected argument found"), which blocked every Codex
//! and Aider commit/push regardless of verdict.
//!
//! These tests spawn the **actual installed hook script** or the actual command
//! written by `klasp install` (not `klasp gate` directly) to verify that the flags
//! emitted by the hook templates no longer cause clap-rejection.
//!
//! ## Exit-code semantics
//!
//! - Exit 0 → gate passed (no checks configured, or all checks passed).
//! - Exit 2 → gate blocked (a check produced `Verdict::Fail`).
//! - Any other exit → unexpected; may indicate a clap-rejection (pre-fix exit 2
//!   would collide with gate-block exit 2 — tests distinguish by using a payload
//!   that cannot be classified as a commit/push when no checks are configured).
//!
//! The "no checks" (pass) case is the critical clap-regression guard: if clap
//! rejected the flags the exit code would be 2, but with no checks configured
//! the gate would also exit 0. We verify exit 0 on `--agent aider` directly.
//! For the failing-check cases, exit 2 from clap and exit 2 from gate-block are
//! indistinguishable by code alone, so we additionally assert on stderr content.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use klasp_core::GATE_SCHEMA_VERSION;
use tempfile::TempDir;

/// The compiled `klasp` binary path.
fn klasp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp")
}

/// A gate-protocol JSON payload representing a `git commit` tool call. This
/// is the payload shape both Codex and Aider sessions pipe to the gate.
fn commit_payload() -> &'static str {
    r#"{
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": "git commit -m test",
            "description": "Commit staged changes."
        },
        "session_id": "regression-test-session"
    }"#
}

/// A klasp.toml with a single always-failing shell check (triggers on commit).
const FAILING_TOML: &str = r#"version = 1

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

/// A minimal klasp.toml with no checks — gate always passes.
const PASSING_TOML: &str = r#"version = 1

[gate]
agents = ["codex"]
policy = "any_fail"
"#;

/// Set up a minimal fake git repo (`.git/` dir by hand, no `git init`) plus
/// `AGENTS.md` (codex detect signal), run `klasp install --agent codex`, and
/// write a `klasp.toml`. Returns the TempDir so callers keep it alive.
fn fresh_codex_repo(toml_body: &str) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::create_dir(dir.path().join(".git/hooks")).unwrap();
    fs::write(dir.path().join("AGENTS.md"), "# Project\n").unwrap();
    fs::write(dir.path().join("klasp.toml"), toml_body).unwrap();

    let out = Command::new(klasp_bin())
        .current_dir(dir.path())
        .args(["install", "--agent", "codex"])
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn klasp install --agent codex");
    assert!(
        out.status.success(),
        "klasp install --agent codex failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    dir
}

/// Set up a minimal repo and run `klasp install --agent aider`. Returns the
/// TempDir and the `commit-cmd-pre` string read from `.aider.conf.yml`.
fn fresh_aider_repo(toml_body: &str) -> (TempDir, String) {
    let dir = TempDir::new().expect("tempdir");
    // Aider's install path calls `find_repo_root_from_cwd`, which requires a `.git` dir.
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::write(dir.path().join("klasp.toml"), toml_body).unwrap();

    let out = Command::new(klasp_bin())
        .current_dir(dir.path())
        .args(["install", "--agent", "aider"])
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn klasp install --agent aider");
    assert!(
        out.status.success(),
        "klasp install --agent aider failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let conf_body =
        fs::read_to_string(dir.path().join(".aider.conf.yml")).expect("read .aider.conf.yml");
    let cmd = extract_commit_cmd_pre(&conf_body);
    (dir, cmd)
}

/// Parse the `commit-cmd-pre:` scalar value from a minimal `.aider.conf.yml`.
/// Panics when the key is absent — callers only call this after a successful
/// `klasp install --agent aider`.
fn extract_commit_cmd_pre(yml: &str) -> String {
    for line in yml.lines() {
        if let Some(rest) = line.strip_prefix("commit-cmd-pre:") {
            return rest.trim().to_string();
        }
    }
    panic!(".aider.conf.yml did not contain a `commit-cmd-pre` key:\n{yml}")
}

/// Invoke `bash <hook-path>` with the given payload on stdin. Adds `klasp`'s
/// binary directory to PATH so the hook's `exec klasp gate …` resolves.
/// Returns the captured `Output`.
fn run_hook_via_bash(hook_path: &Path, repo: &Path, payload: &str) -> Output {
    // The hook script does `exec klasp gate …`, so `klasp` must be on PATH.
    let bin_dir = PathBuf::from(klasp_bin())
        .parent()
        .expect("klasp binary has parent dir")
        .to_path_buf();
    let path_var = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let mut child = Command::new("bash")
        .arg(hook_path)
        .env("PATH", &path_var)
        .env("CLAUDE_PROJECT_DIR", repo)
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bash <hook>");

    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(payload.as_bytes())
        .expect("write payload to hook stdin");

    child.wait_with_output().expect("wait for hook")
}

/// Invoke `klasp gate --agent aider` (the literal command written by
/// `klasp install --agent aider`) with the given payload on stdin.
fn run_aider_gate_cmd(cmd_str: &str, repo: &Path, payload: &str) -> Output {
    // Split the cmd_str into binary + args (no shell quoting to worry about
    // for the value klasp writes: "klasp gate --agent aider").
    let mut parts = cmd_str.split_whitespace();
    let bin_name = parts.next().expect("non-empty commit-cmd-pre command");

    // Resolve the binary via the same PATH trick used above.
    let bin_dir = PathBuf::from(klasp_bin())
        .parent()
        .expect("klasp binary has parent dir")
        .to_path_buf();
    let path_var = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // If the binary name is just "klasp", resolve it through the PATH we build.
    let resolved_bin = if bin_name == "klasp" {
        klasp_bin().to_string()
    } else {
        bin_name.to_string()
    };

    let mut child = Command::new(&resolved_bin)
        .args(parts.collect::<Vec<_>>())
        .env("PATH", &path_var)
        .env("KLASP_GATE_SCHEMA", GATE_SCHEMA_VERSION.to_string())
        .env("CLAUDE_PROJECT_DIR", repo)
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn klasp gate --agent aider");

    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");

    child.wait_with_output().expect("wait for klasp gate")
}

// ─── Test 1: Codex installed hook — pass case ────────────────────────────────

/// The installed pre-commit hook must exit 0 when no checks are configured.
///
/// Pre-fix: `klasp gate --agent codex --trigger commit` caused clap to exit 2
/// ("unexpected argument"). With the fix the flags are accepted-and-ignored,
/// and the gate exits 0 (fail-open on no configured checks).
#[cfg(unix)]
#[test]
fn codex_installed_hook_pass_exits_0() {
    let repo = fresh_codex_repo(PASSING_TOML);
    let hook_path = repo.path().join(".git/hooks/pre-commit");

    // Confirm the hook contains the flags that previously caused clap rejection.
    let hook_body = fs::read_to_string(&hook_path).unwrap();
    assert!(
        hook_body.contains("--agent codex") && hook_body.contains("--trigger commit"),
        "hook must contain --agent and --trigger flags:\n{hook_body}",
    );

    let out = run_hook_via_bash(&hook_path, repo.path(), commit_payload());
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    eprintln!("hook stderr:\n{stderr}");

    assert_eq!(
        out.status.code(),
        Some(0),
        "installed pre-commit hook must exit 0 with no checks configured \
         (pre-fix this would be clap-rejection exit 2);\nstderr:\n{stderr}",
    );
}

// ─── Test 2: Codex installed hook — fail case ────────────────────────────────

/// The installed pre-commit hook must exit 2 when a check fails — and the
/// exit-2 must come from the gate verdict, not from a clap-rejection.
///
/// We distinguish the two by asserting `klasp-gate: blocked` appears on stderr,
/// which clap never emits.
#[cfg(unix)]
#[test]
fn codex_installed_hook_with_failing_check_exits_2() {
    let repo = fresh_codex_repo(FAILING_TOML);
    let hook_path = repo.path().join(".git/hooks/pre-commit");

    let out = run_hook_via_bash(&hook_path, repo.path(), commit_payload());
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    eprintln!("hook stderr:\n{stderr}");

    assert_eq!(
        out.status.code(),
        Some(2),
        "failing check must exit 2;\nstderr:\n{stderr}",
    );
    // Gate-verdict exit 2 emits "klasp-gate: blocked"; clap-rejection does not.
    assert!(
        stderr.contains("klasp-gate:"),
        "exit-2 must come from gate verdict (not clap-rejection); \
         expected 'klasp-gate:' prefix on stderr:\n{stderr}",
    );
}

// ─── Test 3: Aider commit-cmd-pre — pass case ───────────────────────────────

/// The command written by `klasp install --agent aider` must exit 0 when no
/// checks are configured.
///
/// Pre-fix: `klasp gate --agent aider` caused clap to exit 2.
#[test]
fn aider_installed_commit_cmd_pre_pass_exits_0() {
    let aider_toml = PASSING_TOML.replace("codex", "aider");
    let (repo, cmd_str) = fresh_aider_repo(&aider_toml);

    assert_eq!(
        cmd_str, "klasp gate --agent aider",
        "commit-cmd-pre must contain the expected command: {cmd_str:?}",
    );

    let out = run_aider_gate_cmd(&cmd_str, repo.path(), commit_payload());
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    eprintln!("aider gate stderr:\n{stderr}");

    assert_eq!(
        out.status.code(),
        Some(0),
        "aider commit-cmd-pre must exit 0 with no checks configured \
         (pre-fix this was clap-rejection exit 2);\nstderr:\n{stderr}",
    );
}

// ─── Test 4: Aider commit-cmd-pre — fail case ───────────────────────────────

/// The command written by `klasp install --agent aider` must exit 2 when a
/// check fails, and that exit-2 must come from the gate verdict (not clap).
#[test]
fn aider_installed_commit_cmd_pre_with_failing_check_exits_2() {
    let aider_toml = FAILING_TOML.replace("codex", "aider");
    let (repo, cmd_str) = fresh_aider_repo(&aider_toml);

    let out = run_aider_gate_cmd(&cmd_str, repo.path(), commit_payload());
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    eprintln!("aider gate stderr:\n{stderr}");

    assert_eq!(
        out.status.code(),
        Some(2),
        "aider gate with failing check must exit 2;\nstderr:\n{stderr}",
    );
    assert!(
        stderr.contains("klasp-gate:"),
        "exit-2 must come from gate verdict (not clap-rejection); \
         expected 'klasp-gate:' prefix on stderr:\n{stderr}",
    );
}

// ─── Test 5: Claude Code path unbroken ──────────────────────────────────────

/// Sanity check: the existing Claude Code path (no `--agent`/`--trigger` flags)
/// must continue to work after the `GateArgs` change. This guards against
/// accidentally introducing a required positional or breaking the defaults.
#[test]
fn claude_gate_without_agent_trigger_flags_still_exits_0() {
    let dir = TempDir::new().expect("tempdir");
    fs::write(dir.path().join("klasp.toml"), PASSING_TOML).unwrap();

    let mut child = Command::new(klasp_bin())
        .arg("gate")
        .env("KLASP_GATE_SCHEMA", GATE_SCHEMA_VERSION.to_string())
        .env("CLAUDE_PROJECT_DIR", dir.path())
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn klasp gate");

    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(commit_payload().as_bytes())
        .expect("write payload");

    let out = child.wait_with_output().expect("wait");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    eprintln!("claude gate stderr:\n{stderr}");

    assert_eq!(
        out.status.code(),
        Some(0),
        "klasp gate without --agent/--trigger must still exit 0;\nstderr:\n{stderr}",
    );
}
