//! Integration tests for `klasp init`.
//!
//! Each test spins up a temp directory, optionally seeds a `.git/`
//! subdirectory so `resolve_repo_root` accepts it, then asserts on the
//! exit status, stdout/stderr, and on-disk file contents. Tests run the
//! compiled binary via `env!("CARGO_BIN_EXE_klasp")` so the full clap
//! parsing + cmd dispatch path is exercised.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use klasp_core::ConfigV1;

fn run_init(dir: &Path, extra_args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_klasp"))
        .current_dir(dir)
        .arg("init")
        .args(extra_args)
        // Doctor / config loading consults `CLAUDE_PROJECT_DIR`; init does
        // not, but we strip it so any harness env doesn't leak in.
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn klasp")
}

#[test]
fn init_creates_klasp_toml() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();

    let out = run_init(dir.path(), &[]);
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    let toml_path = dir.path().join("klasp.toml");
    assert!(toml_path.exists(), "klasp.toml should exist after init");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("wrote "),
        "stdout should contain 'wrote ': {stdout}"
    );

    ConfigV1::from_file(&toml_path).expect("written klasp.toml must parse via ConfigV1");
}

#[test]
fn init_toml_parses_via_config_v1() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();

    let out = run_init(dir.path(), &[]);
    assert!(out.status.success());

    let config = ConfigV1::from_file(&dir.path().join("klasp.toml")).expect("parse failed");
    assert_eq!(config.version, 1);
    // v0.3 ships three surfaces; `klasp install --agent all` walks this list.
    assert_eq!(
        config.gate.agents,
        vec!["claude_code", "codex", "aider"]
    );
    assert!(
        config.checks.is_empty(),
        "template ships with no active checks (only commented examples)"
    );
}

#[test]
fn init_refuses_existing_toml_without_force() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    let toml_path = dir.path().join("klasp.toml");
    fs::write(&toml_path, "# user content\n").unwrap();

    let out = run_init(dir.path(), &[]);
    assert!(!out.status.success(), "expected non-zero exit");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already exists"),
        "stderr should mention 'already exists': {stderr}"
    );
    assert!(
        stderr.contains("--force"),
        "stderr should hint at --force: {stderr}"
    );

    let on_disk = fs::read_to_string(&toml_path).unwrap();
    assert_eq!(
        on_disk, "# user content\n",
        "on-disk file must be unchanged when --force is omitted"
    );
}

#[test]
fn init_with_force_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    let toml_path = dir.path().join("klasp.toml");
    fs::write(&toml_path, "# user content\n").unwrap();

    let out = run_init(dir.path(), &["--force"]);
    assert!(
        out.status.success(),
        "expected exit 0 with --force\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let on_disk = fs::read_to_string(&toml_path).unwrap();
    assert!(on_disk.contains("version = 1"));
    assert!(on_disk.contains("[gate]"));
    assert!(!on_disk.contains("# user content"));
}

#[test]
fn init_not_a_git_repo_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_init(dir.path(), &[]);

    assert!(!out.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not a git repository"),
        "stderr should mention git repo: {stderr}"
    );
}
