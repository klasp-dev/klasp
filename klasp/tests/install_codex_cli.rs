//! End-to-end CLI integration tests for v0.2 W3 — `klasp install --agent codex`,
//! `--agent all`, unknown-agent handling, and the husky-conflict warning path.
//!
//! Tests run the compiled `klasp` binary via `env!("CARGO_BIN_EXE_klasp")`
//! so the full clap parse + cmd dispatch path is exercised end-to-end.
//! Repos are minimally seeded with `.git/` + the relevant agent fingerprint
//! files (`AGENTS.md`, `.claude/`) before invoking the CLI.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const VALID_TOML_BOTH: &str = r#"version = 1

[gate]
agents = ["claude_code", "codex"]
policy = "any_fail"
"#;

const VALID_TOML_EMPTY_AGENTS: &str = r#"version = 1

[gate]
agents = []
policy = "any_fail"
"#;

fn run_install(dir: &Path, extra_args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_klasp"))
        .current_dir(dir)
        .arg("install")
        .args(extra_args)
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn klasp")
}

fn run_uninstall(dir: &Path, extra_args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_klasp"))
        .current_dir(dir)
        .arg("uninstall")
        .args(extra_args)
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn klasp")
}

fn fresh_codex_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::create_dir(dir.path().join(".git").join("hooks")).unwrap();
    // AGENTS.md presence is the codex auto-detect signal.
    fs::write(dir.path().join("AGENTS.md"), "# Project\n").unwrap();
    dir
}

fn fresh_dual_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::create_dir(dir.path().join(".git").join("hooks")).unwrap();
    fs::create_dir(dir.path().join(".claude")).unwrap();
    fs::write(dir.path().join("AGENTS.md"), "# Project\n").unwrap();
    dir
}

fn write_toml(repo_root: &Path, body: &str) {
    fs::write(repo_root.join("klasp.toml"), body).unwrap();
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// ─── Acceptance #1: --agent codex installs only the Codex surface ──────────

#[test]
fn install_agent_codex_writes_agents_md_and_hooks_only() {
    let repo = fresh_codex_repo();

    let out = run_install(repo.path(), &["--agent", "codex"]);
    assert!(
        out.status.success(),
        "expected exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout(&out),
        stderr(&out)
    );

    // Codex artefacts written.
    let agents_md = fs::read_to_string(repo.path().join("AGENTS.md")).unwrap();
    assert!(
        agents_md.contains("klasp:managed:start"),
        "AGENTS.md must contain managed block:\n{agents_md}"
    );

    let pre_commit = repo.path().join(".git/hooks/pre-commit");
    assert!(
        pre_commit.exists(),
        "pre-commit hook must be written for codex"
    );
    let pre_commit_body = fs::read_to_string(&pre_commit).unwrap();
    assert!(
        pre_commit_body.contains("--agent codex"),
        "pre-commit must dispatch to codex agent:\n{pre_commit_body}"
    );

    let pre_push = repo.path().join(".git/hooks/pre-push");
    assert!(pre_push.exists(), "pre-push hook must be written for codex");

    // Claude artefacts NOT written — `--agent codex` is exclusive.
    assert!(
        !repo.path().join(".claude/hooks/klasp-gate.sh").exists(),
        "claude hook must NOT be written when --agent codex"
    );
    assert!(
        !repo.path().join(".claude/settings.json").exists(),
        "claude settings.json must NOT be written when --agent codex"
    );
}

// ─── Acceptance #2: --agent all installs every surface in [gate].agents ────

#[test]
fn install_agent_all_writes_every_listed_surface() {
    let repo = fresh_dual_repo();
    write_toml(repo.path(), VALID_TOML_BOTH);

    let out = run_install(repo.path(), &["--agent", "all"]);
    assert!(
        out.status.success(),
        "expected exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout(&out),
        stderr(&out)
    );

    // Both surfaces wrote their artefacts.
    assert!(
        repo.path().join(".claude/hooks/klasp-gate.sh").exists(),
        "claude hook must be written"
    );
    assert!(
        repo.path().join(".claude/settings.json").exists(),
        "claude settings.json must be written"
    );

    let agents_md = fs::read_to_string(repo.path().join("AGENTS.md")).unwrap();
    assert!(agents_md.contains("klasp:managed:start"));

    assert!(
        repo.path().join(".git/hooks/pre-commit").exists(),
        "pre-commit must be written"
    );
    assert!(
        repo.path().join(".git/hooks/pre-push").exists(),
        "pre-push must be written"
    );

    let so = stdout(&out);
    assert!(so.contains("claude_code: installed"), "stdout:\n{so}");
    assert!(so.contains("codex: installed"), "stdout:\n{so}");
}

// ─── Acceptance #3: unknown agent name produces a clear error ──────────────

#[test]
fn install_unknown_agent_exits_nonzero_with_supported_list() {
    let repo = fresh_codex_repo();

    let out = run_install(repo.path(), &["--agent", "foo"]);
    assert!(!out.status.success(), "expected non-zero exit");

    let se = stderr(&out);
    assert!(
        se.contains("unknown agent \"foo\""),
        "stderr should name the unknown agent: {se}"
    );
    assert!(
        se.contains("claude_code") && se.contains("codex"),
        "stderr should list supported agents: {se}"
    );
    // No artefacts written.
    assert!(!repo.path().join(".git/hooks/pre-commit").exists());
}

// ─── Acceptance #4: --agent all with empty [gate].agents is a no-op ────────

#[test]
fn install_agent_all_with_empty_list_warns_and_exits_0() {
    let repo = fresh_dual_repo();
    write_toml(repo.path(), VALID_TOML_EMPTY_AGENTS);

    let out = run_install(repo.path(), &["--agent", "all"]);
    assert!(
        out.status.success(),
        "expected exit 0 on empty agents list, got {:?}\nstderr:\n{}",
        out.status,
        stderr(&out)
    );

    let se = stderr(&out);
    assert!(
        se.contains("warning:"),
        "stderr should carry a `warning:` line: {se}"
    );
    assert!(
        se.contains("[gate].agents = []") || se.contains("`[gate].agents = []`"),
        "warning should explain the empty agents array: {se}"
    );

    // Nothing written — explicit no-op.
    assert!(!repo.path().join(".claude/hooks/klasp-gate.sh").exists());
    assert!(!repo.path().join(".git/hooks/pre-commit").exists());
    let agents_md = fs::read_to_string(repo.path().join("AGENTS.md")).unwrap();
    assert!(
        !agents_md.contains("klasp:managed:start"),
        "AGENTS.md must remain untouched on no-op"
    );
}

// ─── Acceptance #2 (continued): warnings surface for foreign hook managers ─

#[test]
fn install_agent_codex_skips_husky_pre_commit_with_warning() {
    let repo = fresh_codex_repo();

    // Seed husky-managed pre-commit. The fingerprint `_/husky.sh` triggers
    // `git_hooks::detect_conflict::Husky`.
    let husky_hook = "#!/usr/bin/env sh\n\
        . \"$(dirname -- \"$0\")/_/husky.sh\"\n\
        npm test\n";
    fs::write(repo.path().join(".git/hooks/pre-commit"), husky_hook).unwrap();

    let out = run_install(repo.path(), &["--agent", "codex"]);
    // Conflict is non-fatal: install completes successfully.
    assert!(
        out.status.success(),
        "husky conflict must NOT fail install\nstdout:\n{}\nstderr:\n{}",
        stdout(&out),
        stderr(&out)
    );

    let se = stderr(&out);
    assert!(
        se.contains("warning:"),
        "stderr must carry a `warning:` line: {se}"
    );
    assert!(
        se.contains("pre-commit"),
        "warning must name the pre-commit hook: {se}"
    );
    assert!(
        se.contains("husky"),
        "warning must name husky as the foreign tool: {se}"
    );
    // The actionable suggestion is mandatory per issue #29.
    assert!(
        se.contains("klasp gate") && se.contains("--trigger commit"),
        "warning must include the manual-install incantation: {se}"
    );

    // Husky hook untouched byte-for-byte.
    let pre_commit_after = fs::read_to_string(repo.path().join(".git/hooks/pre-commit")).unwrap();
    assert_eq!(
        pre_commit_after, husky_hook,
        "husky pre-commit must not be modified"
    );

    // pre-push had no conflict — gets klasp's block.
    let pre_push = fs::read_to_string(repo.path().join(".git/hooks/pre-push")).unwrap();
    assert!(pre_push.contains("# >>> klasp managed start <<<"));

    // AGENTS.md was always safe to write.
    let agents_md = fs::read_to_string(repo.path().join("AGENTS.md")).unwrap();
    assert!(agents_md.contains("klasp:managed:start"));
}

// ─── Acceptance #5: uninstall mirrors install ──────────────────────────────

#[test]
fn uninstall_agent_codex_removes_only_codex_artefacts() {
    let repo = fresh_dual_repo();
    write_toml(repo.path(), VALID_TOML_BOTH);

    // Install both, then uninstall only codex.
    let out = run_install(repo.path(), &["--agent", "all"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let out = run_uninstall(repo.path(), &["--agent", "codex"]);
    assert!(
        out.status.success(),
        "uninstall failed:\nstderr:\n{}",
        stderr(&out)
    );

    // Codex artefacts gone (the hook files round-trip to deleted because
    // klasp owned them end-to-end on a fresh install).
    let agents_md = fs::read_to_string(repo.path().join("AGENTS.md")).unwrap();
    assert!(
        !agents_md.contains("klasp:managed:start"),
        "AGENTS.md block must be stripped"
    );
    assert!(
        !repo.path().join(".git/hooks/pre-commit").exists(),
        "pre-commit deleted after uninstall"
    );
    assert!(
        !repo.path().join(".git/hooks/pre-push").exists(),
        "pre-push deleted after uninstall"
    );

    // Claude artefacts preserved.
    assert!(
        repo.path().join(".claude/hooks/klasp-gate.sh").exists(),
        "claude hook must survive `uninstall --agent codex`"
    );
    assert!(repo.path().join(".claude/settings.json").exists());
}

#[test]
fn uninstall_agent_all_removes_every_listed_surface() {
    let repo = fresh_dual_repo();
    write_toml(repo.path(), VALID_TOML_BOTH);

    let out = run_install(repo.path(), &["--agent", "all"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let out = run_uninstall(repo.path(), &["--agent", "all"]);
    assert!(out.status.success(), "{}", stderr(&out));

    // Both surfaces' hooks removed.
    assert!(!repo.path().join(".claude/hooks/klasp-gate.sh").exists());
    assert!(!repo.path().join(".git/hooks/pre-commit").exists());
    assert!(!repo.path().join(".git/hooks/pre-push").exists());

    let agents_md = fs::read_to_string(repo.path().join("AGENTS.md")).unwrap();
    assert!(!agents_md.contains("klasp:managed:start"));
}

#[test]
fn uninstall_agent_all_walks_registry_regardless_of_config() {
    // `uninstall --agent all` is the safety-net path: it walks every
    // registered surface, ignoring `[gate].agents`. A user who installed
    // both surfaces, dropped one from config, then ran uninstall must
    // still get every previously-installed surface cleaned up — orphan
    // hook scripts and managed blocks would otherwise persist forever.
    //
    // Asserts:
    //  - exit 0 even when `[gate].agents = []`
    //  - "nothing to remove" lines for both registered surfaces (fresh
    //    repo, nothing was installed)
    let repo = fresh_dual_repo();
    write_toml(repo.path(), VALID_TOML_EMPTY_AGENTS);

    let out = run_uninstall(repo.path(), &["--agent", "all"]);
    assert!(
        out.status.success(),
        "expected exit 0\nstderr:\n{}",
        stderr(&out)
    );
    let so = stdout(&out);
    assert!(
        so.contains("claude_code: nothing to remove") && so.contains("codex: nothing to remove"),
        "expected uninstall to walk every registered surface; got stdout:\n{so}"
    );
}

#[test]
fn uninstall_unknown_agent_exits_nonzero() {
    let repo = fresh_codex_repo();

    let out = run_uninstall(repo.path(), &["--agent", "foo"]);
    assert!(!out.status.success(), "expected non-zero exit");
    assert!(stderr(&out).contains("unknown agent \"foo\""));
}

// ─── Idempotency: re-running --agent all is a no-op ────────────────────────

#[test]
fn install_agent_all_is_idempotent() {
    let repo = fresh_dual_repo();
    write_toml(repo.path(), VALID_TOML_BOTH);

    let first = run_install(repo.path(), &["--agent", "all"]);
    assert!(first.status.success());

    let second = run_install(repo.path(), &["--agent", "all"]);
    assert!(second.status.success());
    let so = stdout(&second);
    assert!(
        so.contains("claude_code: already installed") && so.contains("codex: already installed"),
        "second run must be a no-op:\n{so}"
    );
}
