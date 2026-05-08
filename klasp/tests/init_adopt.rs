//! Integration tests for `klasp init --adopt`.
//!
//! Covers issue #97 acceptance criteria:
//!   - inspect prints findings without writing files
//!   - mirror writes klasp.toml that mirrors detected gates
//!   - mirror never modifies hook configs (husky, lefthook, pre-commit, plain git hooks)
//!   - chain mode is rejected with an explanatory message (exit code 2)
//!   - package-manager-aware lint-staged command selection
//!   - force semantics for mirror mode when klasp.toml already exists
//!
//! Each test creates its own TempDir. No global state is touched.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Run `klasp init --adopt` with extra args from `dir`.
fn run_init_adopt(dir: &Path, extra_args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_klasp"))
        .current_dir(dir)
        .arg("init")
        .arg("--adopt")
        .args(extra_args)
        .env_remove("CLAUDE_PROJECT_DIR")
        .output()
        .expect("spawn klasp")
}

/// Build a minimal git repo fixture (`.git/` + `.git/hooks/`) using `git init`.
///
/// If `git` is not available on PATH, returns `None` — callers should skip
/// the test rather than panic (guard with `let Some(dir) = fixture_repo() else { return; }`).
fn fixture_repo() -> Option<tempfile::TempDir> {
    if which::which("git").is_err() {
        eprintln!("git not on PATH — skipping test");
        return None;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status()
        .expect("spawn git");
    if !status.success() {
        eprintln!("git init failed — skipping test");
        return None;
    }
    Some(dir)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// ─── inspect mode ───────────────────────────────────────────────────────────

/// AC: inspect mode prints "No existing gates detected." when the repo has
/// no recognisable gate infrastructure. No klasp.toml is written.
#[test]
fn inspect_no_gates_prints_no_existing_gates_message() {
    let Some(dir) = fixture_repo() else { return };

    let out = run_init_adopt(dir.path(), &["--mode", "inspect"]);

    assert!(
        out.status.success(),
        "expected exit 0\nstdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );

    let so = stdout(&out);
    assert!(
        so.contains("No existing gates detected"),
        "stdout should report no gates detected:\n{so}"
    );

    assert!(
        !dir.path().join("klasp.toml").exists(),
        "inspect must not write klasp.toml"
    );
}

/// AC: inspect mode prints the pre-commit finding (gate type, mirror snippet,
/// Next: block) without writing klasp.toml.
#[test]
fn inspect_pre_commit_only_prints_finding() {
    let Some(dir) = fixture_repo() else { return };

    fs::write(dir.path().join(".pre-commit-config.yaml"), "repos: []\n").unwrap();

    let out = run_init_adopt(dir.path(), &["--mode", "inspect"]);

    assert!(
        out.status.success(),
        "expected exit 0\nstdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );

    let so = stdout(&out);
    assert!(
        so.contains("pre-commit framework") || so.contains("pre_commit"),
        "stdout should mention pre-commit framework:\n{so}"
    );
    assert!(
        so.contains("pre_commit"),
        "stdout should show the proposed mirror type:\n{so}"
    );
    assert!(
        so.contains("Next:"),
        "stdout should include a Next: block:\n{so}"
    );

    assert!(
        !dir.path().join("klasp.toml").exists(),
        "inspect must not write klasp.toml"
    );
}

/// AC: inspect mode does not modify the filesystem. Directory tree before and
/// after running inspect must be identical.
#[test]
fn inspect_does_not_modify_filesystem() {
    let Some(dir) = fixture_repo() else { return };

    // Seed all three fixture types.
    fs::write(dir.path().join(".pre-commit-config.yaml"), "repos: []\n").unwrap();
    fs::create_dir_all(dir.path().join(".husky")).unwrap();
    fs::write(
        dir.path().join(".husky/pre-commit"),
        "#!/bin/sh\nnpx --no -- lint-staged\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("lefthook.yml"),
        "pre-commit:\n  commands:\n    lint:\n      run: pnpm lint\n",
    )
    .unwrap();

    // Snapshot directory before.
    let before = collect_dir_snapshot(dir.path());

    run_init_adopt(dir.path(), &["--mode", "inspect"]);

    let after = collect_dir_snapshot(dir.path());

    assert_eq!(
        before, after,
        "inspect must not modify any files in the repo"
    );
}

/// Collect a sorted list of (relative path, contents) pairs for asserting
/// directory identity. Skips `.git/` internals that git modifies on access.
fn collect_dir_snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    collect_dir_snapshot_inner(root, root, &mut entries);
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

fn collect_dir_snapshot_inner(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        // Skip .git internals — git may update index/HEAD on read.
        if rel.starts_with(".git/") || rel == ".git" {
            continue;
        }
        if path.is_dir() {
            collect_dir_snapshot_inner(root, &path, out);
        } else {
            let contents = fs::read(&path).unwrap_or_default();
            out.push((rel, contents));
        }
    }
}

// ─── mirror mode ────────────────────────────────────────────────────────────

/// AC: mirror mode writes klasp.toml containing exactly one [[checks]] block
/// with `source.type = "pre_commit"` when .pre-commit-config.yaml is present.
/// AC: mirror mode never modifies .pre-commit-config.yaml.
#[test]
fn mirror_pre_commit_writes_klasp_toml() {
    let Some(dir) = fixture_repo() else { return };

    let pre_commit_yaml = "repos: []\n";
    fs::write(dir.path().join(".pre-commit-config.yaml"), pre_commit_yaml).unwrap();

    let out = run_init_adopt(dir.path(), &["--mode", "mirror"]);

    assert!(
        out.status.success(),
        "expected exit 0\nstdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );

    let toml_path = dir.path().join("klasp.toml");
    assert!(toml_path.exists(), "mirror mode must write klasp.toml");

    let toml_str = fs::read_to_string(&toml_path).unwrap();

    // Must parse as valid ConfigV1.
    let config = klasp_core::ConfigV1::from_file(&toml_path)
        .expect("written klasp.toml must parse via ConfigV1");

    // Must contain exactly one check with source type pre_commit.
    let pre_commit_checks: Vec<_> = config
        .checks
        .iter()
        .filter(|ch| matches!(ch.source, klasp_core::CheckSourceConfig::PreCommit { .. }))
        .collect();
    assert_eq!(
        pre_commit_checks.len(),
        1,
        "expected exactly one pre_commit check, got {} checks\ntoml:\n{}",
        pre_commit_checks.len(),
        toml_str
    );

    // .pre-commit-config.yaml must be byte-identical.
    let yaml_after = fs::read_to_string(dir.path().join(".pre-commit-config.yaml")).unwrap();
    assert_eq!(
        yaml_after, pre_commit_yaml,
        "mirror mode must not modify .pre-commit-config.yaml"
    );
}

/// AC: Husky + lint-staged detector picks a package-manager-aware command.
/// With pnpm-lock.yaml present, the shell command must be `pnpm exec lint-staged`.
#[test]
fn mirror_husky_lint_staged_uses_pkg_manager_command() {
    let Some(dir) = fixture_repo() else { return };

    // Create .husky/pre-commit referencing lint-staged.
    fs::create_dir_all(dir.path().join(".husky")).unwrap();
    fs::write(
        dir.path().join(".husky/pre-commit"),
        "#!/bin/sh\nnpx --no -- lint-staged\n",
    )
    .unwrap();

    // package.json with lint-staged config.
    fs::write(
        dir.path().join("package.json"),
        r#"{"lint-staged": {"*.ts": "tsc --noEmit"}}"#,
    )
    .unwrap();

    // pnpm lockfile — should make the detector pick pnpm exec lint-staged.
    fs::write(
        dir.path().join("pnpm-lock.yaml"),
        "lockfileVersion: '6.0'\n",
    )
    .unwrap();

    let out = run_init_adopt(dir.path(), &["--mode", "mirror"]);

    assert!(
        out.status.success(),
        "expected exit 0\nstdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );

    let toml_path = dir.path().join("klasp.toml");
    assert!(toml_path.exists(), "mirror mode must write klasp.toml");

    let toml_str = fs::read_to_string(&toml_path).unwrap();

    assert!(
        toml_str.contains("pnpm exec lint-staged"),
        "expected `pnpm exec lint-staged` in klasp.toml (pnpm-lock.yaml present):\n{toml_str}"
    );
}

/// AC: Lefthook detector emits a per-command shell check.
/// `lefthook.yml` with `pre-commit: commands: lint: run: pnpm lint`
/// should produce `[[checks]] name = "lint"` with `command = "pnpm lint"`.
#[test]
fn mirror_lefthook_emits_per_command_check() {
    let Some(dir) = fixture_repo() else { return };

    fs::write(
        dir.path().join("lefthook.yml"),
        "pre-commit:\n  commands:\n    lint:\n      run: pnpm lint\n",
    )
    .unwrap();

    let out = run_init_adopt(dir.path(), &["--mode", "mirror"]);

    assert!(
        out.status.success(),
        "expected exit 0\nstdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );

    let toml_path = dir.path().join("klasp.toml");
    assert!(toml_path.exists(), "mirror mode must write klasp.toml");

    let toml_str = fs::read_to_string(&toml_path).unwrap();

    assert!(
        toml_str.contains("pnpm lint"),
        "expected `pnpm lint` shell command in klasp.toml from lefthook:\n{toml_str}"
    );

    // Also verify it parses cleanly.
    klasp_core::ConfigV1::from_file(&toml_path)
        .expect("lefthook-mirror klasp.toml must parse via ConfigV1");
}

/// AC: plain `.git/hooks/pre-commit` user script must never be overwritten by
/// mirror mode. The hook file must be byte-identical post-run.
#[test]
fn mirror_plain_git_hook_does_not_overwrite_hook() {
    let Some(dir) = fixture_repo() else { return };

    let hooks_dir = dir.path().join(".git/hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    let original_hook = b"#!/bin/sh\necho 'my custom hook'\nexit 0\n";
    let hook_path = hooks_dir.join("pre-commit");
    fs::write(&hook_path, original_hook).unwrap();

    let out = run_init_adopt(dir.path(), &["--mode", "mirror"]);

    // May succeed or fail, but must not modify the hook.
    let hook_after = fs::read(&hook_path).unwrap();
    assert_eq!(
        hook_after, original_hook,
        "mirror mode must not overwrite .git/hooks/pre-commit"
    );

    // stdout/stderr are informational only — no assertion on exit code here
    // since a plain hook with no other gates might produce no klasp.toml.
    let _ = out;
}

/// AC: mirror mode without --force must exit non-zero and report
/// "klasp.toml already exists" when a klasp.toml is already present.
#[test]
fn mirror_existing_klasp_toml_without_force_errors() {
    let Some(dir) = fixture_repo() else { return };

    fs::write(dir.path().join(".pre-commit-config.yaml"), "repos: []\n").unwrap();
    fs::write(dir.path().join("klasp.toml"), "# existing content\n").unwrap();

    let out = run_init_adopt(dir.path(), &["--mode", "mirror"]);

    assert!(
        !out.status.success(),
        "expected non-zero exit when klasp.toml exists and --force omitted\nstdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );

    let se = stderr(&out);
    assert!(
        se.contains("already exists"),
        "stderr must mention 'already exists':\n{se}"
    );

    // Original content must be preserved.
    let on_disk = fs::read_to_string(dir.path().join("klasp.toml")).unwrap();
    assert_eq!(on_disk, "# existing content\n");
}

/// AC: mirror mode with --force overwrites an existing klasp.toml and the
/// result parses cleanly.
#[test]
fn mirror_existing_klasp_toml_with_force_overwrites() {
    let Some(dir) = fixture_repo() else { return };

    fs::write(dir.path().join(".pre-commit-config.yaml"), "repos: []\n").unwrap();
    fs::write(dir.path().join("klasp.toml"), "# existing content\n").unwrap();

    let out = run_init_adopt(dir.path(), &["--mode", "mirror", "--force"]);

    assert!(
        out.status.success(),
        "expected exit 0 with --force\nstdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );

    let toml_path = dir.path().join("klasp.toml");
    assert!(toml_path.exists());

    // Must parse — the overwritten file is a valid klasp.toml.
    klasp_core::ConfigV1::from_file(&toml_path)
        .expect("overwritten klasp.toml must parse via ConfigV1");

    let on_disk = fs::read_to_string(&toml_path).unwrap();
    assert!(
        !on_disk.contains("# existing content"),
        "force should overwrite the old content:\n{on_disk}"
    );
}

/// AC: a Husky hook with two substantive commands (e.g. `pnpm lint\npnpm test`)
/// must produce two `[[checks]]` entries in klasp.toml — one per command.
#[test]
fn mirror_husky_multi_command_emits_multiple_checks() {
    let Some(dir) = fixture_repo() else { return };

    fs::create_dir_all(dir.path().join(".husky")).unwrap();
    fs::write(
        dir.path().join(".husky/pre-commit"),
        "#!/bin/sh\n. \"$(dirname -- \"$0\")/_/husky.sh\"\npnpm lint\npnpm test\n",
    )
    .unwrap();

    let out = run_init_adopt(dir.path(), &["--mode", "mirror"]);

    assert!(
        out.status.success(),
        "expected exit 0\nstdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );

    let toml_path = dir.path().join("klasp.toml");
    assert!(toml_path.exists(), "mirror mode must write klasp.toml");

    let config = klasp_core::ConfigV1::from_file(&toml_path)
        .expect("written klasp.toml must parse via ConfigV1");

    let shell_checks: Vec<_> = config
        .checks
        .iter()
        .filter(|ch| matches!(ch.source, klasp_core::CheckSourceConfig::Shell { .. }))
        .collect();

    assert_eq!(
        shell_checks.len(),
        2,
        "expected 2 shell checks for 2-command hook body, got {}:\n{}",
        shell_checks.len(),
        fs::read_to_string(&toml_path).unwrap()
    );

    let names: Vec<&str> = shell_checks.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"lint"),
        "expected a 'lint' check; got: {names:?}"
    );
    assert!(
        names.contains(&"test"),
        "expected a 'test' check; got: {names:?}"
    );
}

// ─── chain mode ─────────────────────────────────────────────────────────────

/// AC: chain mode is rejected with exit code 2 and an explanatory message that
/// mentions "chain mode is not supported" and suggests --mode mirror.
#[test]
fn chain_mode_rejects_with_explanatory_message() {
    let Some(dir) = fixture_repo() else { return };

    fs::write(dir.path().join(".pre-commit-config.yaml"), "repos: []\n").unwrap();

    let out = run_init_adopt(dir.path(), &["--mode", "chain"]);

    assert_eq!(
        out.status.code(),
        Some(2),
        "chain mode must exit with code 2\nstdout: {}\nstderr: {}",
        stdout(&out),
        stderr(&out)
    );

    let se = stderr(&out);
    assert!(
        se.to_lowercase().contains("chain") && se.to_lowercase().contains("not supported"),
        "stderr must mention that chain mode is not supported:\n{se}"
    );
    assert!(
        se.contains("mirror"),
        "stderr must suggest --mode mirror as the alternative:\n{se}"
    );
}
