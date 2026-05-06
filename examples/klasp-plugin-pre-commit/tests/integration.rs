//! Integration tests for `klasp-plugin-pre-commit`.
//!
//! Each test drives the compiled binary directly via `CARGO_BIN_EXE_klasp-plugin-pre-commit`.
//! Fixture pre-commit scripts are written to a tempdir and prepended to PATH.
//!
//! Tests are gated on `#[cfg(unix)]` because the bash-shim approach is Unix-only.
//! On Windows, the plugin still compiles but these specific integration tests
//! are skipped (a future Windows-native test would need a different fixture).
//!
//! Test list:
//! 1. `describe_emits_protocol_v0`
//! 2. `gate_with_no_failing_hooks_returns_pass`
//! 3. `gate_with_failing_hooks_returns_fail`
//! 4. `gate_with_missing_pre_commit_returns_warn`
//! 5. `gate_truncates_excessive_findings`
//! 6. `gate_input_with_unknown_protocol_version_warns`

#![cfg(unix)]

use std::io::Write as IoWrite;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// Absolute path to the compiled plugin binary.
fn plugin_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp-plugin-pre-commit")
}

/// System directories always included in PATH so subprocesses (including the
/// fake pre-commit shim) can find `bash`, `cat`, `echo`, etc.
const SYSTEM_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Build a PATH string with the given bin dir prepended to system dirs.
fn make_path(bin_dir: &std::path::Path) -> String {
    format!("{}:{}", bin_dir.display(), SYSTEM_PATH)
}

/// Write a shell script to `dir/pre-commit`, chmod +x, return the dir path.
/// Uses `#!/bin/bash` directly to avoid `env bash` resolution issues when PATH
/// is restricted in tests.
fn install_fake_pre_commit(dir: &TempDir, script_body: &str) -> std::path::PathBuf {
    let bin_path = dir.path().join("pre-commit");
    std::fs::write(&bin_path, script_body).expect("write fake pre-commit");
    let mut perms = std::fs::metadata(&bin_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&bin_path, perms).expect("chmod");
    dir.path().to_path_buf()
}

/// Build a minimal `PluginGateInput` JSON string.
fn gate_input(protocol_version: u32, trigger_kind: &str, files: &[&str]) -> String {
    let files_json: Vec<String> = files.iter().map(|f| format!("\"{f}\"")).collect();
    format!(
        r#"{{
  "protocol_version": {protocol_version},
  "schema_version": 2,
  "trigger": {{
    "kind": "{trigger_kind}",
    "files": [{files}]
  }},
  "config": {{
    "type": "pre-commit",
    "args": []
  }},
  "repo_root": "/tmp",
  "base_ref": "origin/main"
}}"#,
        files = files_json.join(", ")
    )
}

/// Invoke the plugin with the given args, optional stdin, and optional PATH override.
/// Returns (exit_code, stdout, stderr).
fn run_plugin(args: &[&str], stdin: Option<&str>, path: Option<&str>) -> (i32, String, String) {
    let mut cmd = Command::new(plugin_bin());
    for a in args {
        cmd.arg(a);
    }
    if let Some(p) = path {
        cmd.env("PATH", p);
    }
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

// ── Tests ─────────────────────────────────────────────────────────────────────

/// 1. `--describe` must emit protocol_version=0, correct name, config_types,
///    and supports.verdict_v0=true.
#[test]
fn describe_emits_protocol_v0() {
    let (code, stdout, stderr) = run_plugin(&["--describe"], None, None);
    assert_eq!(code, 0, "exit must be 0; stderr: {stderr}");

    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("--describe output is not valid JSON: {e}\nout={stdout}"));

    assert_eq!(
        v["protocol_version"].as_u64(),
        Some(0),
        "protocol_version must be 0"
    );
    let name = v["name"].as_str().unwrap_or("");
    assert!(
        name.starts_with("klasp-plugin-pre-commit"),
        "name must start with 'klasp-plugin-pre-commit', got: {name}"
    );
    let config_types = v["config_types"].as_array().expect("config_types is array");
    assert!(
        config_types
            .iter()
            .any(|t| t.as_str() == Some("pre-commit")),
        "config_types must contain 'pre-commit'"
    );
    assert_eq!(
        v["supports"]["verdict_v0"].as_bool(),
        Some(true),
        "supports.verdict_v0 must be true"
    );
}

/// 2. When pre-commit exits 0 (no hook failures), plugin returns `pass`.
#[test]
fn gate_with_no_failing_hooks_returns_pass() {
    let tmp = TempDir::new().expect("tempdir");
    let script = "#!/bin/bash\ncase \"${1:-}\" in --version) echo \"pre-commit 3.8.0\"; exit 0 ;; esac\nexit 0\n";
    let bin_dir = install_fake_pre_commit(&tmp, script);
    let path = make_path(&bin_dir);

    let input = gate_input(0, "commit", &[]);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), Some(&path));
    assert_eq!(code, 0, "plugin must exit 0; stderr: {stderr}");

    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("output not valid JSON: {e}\nout={stdout}"));
    assert_eq!(
        v["verdict"].as_str(),
        Some("pass"),
        "verdict must be pass; got: {v}"
    );
}

/// 3. When pre-commit emits a hook failure line and exits 1, plugin returns `fail`
///    with a finding whose rule contains the hook id.
#[test]
fn gate_with_failing_hooks_returns_fail() {
    let tmp = TempDir::new().expect("tempdir");
    let script = concat!(
        "#!/bin/bash\n",
        "case \"${1:-}\" in --version) echo \"pre-commit 3.8.0\"; exit 0 ;; esac\n",
        "echo \"myhook.......................................................................Failed\"\n",
        "echo \"- exit code: 1\"\n",
        "exit 1\n",
    );
    let bin_dir = install_fake_pre_commit(&tmp, script);
    let path = make_path(&bin_dir);

    let input = gate_input(0, "commit", &[]);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), Some(&path));
    assert_eq!(code, 0, "plugin must exit 0 even on fail; stderr: {stderr}");

    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("output not valid JSON: {e}\nout={stdout}"));
    assert_eq!(v["verdict"].as_str(), Some("fail"), "verdict must be fail");
    let findings = v["findings"].as_array().expect("findings is array");
    assert!(
        !findings.is_empty(),
        "there must be at least one finding for the failed hook"
    );
    let rule = findings[0]["rule"].as_str().unwrap_or("");
    assert!(
        rule.contains("myhook"),
        "finding rule must reference 'myhook'; got: {rule}"
    );
}

/// 4. When pre-commit is not on PATH, plugin returns `warn` with
///    rule = "klasp-plugin-pre-commit/binary-missing".
#[test]
fn gate_with_missing_pre_commit_returns_warn() {
    let input = gate_input(0, "commit", &[]);
    // Use a PATH that has system dirs (so the plugin binary works) but no pre-commit.
    let tmp = TempDir::new().expect("tempdir");
    // Empty bin dir — no pre-commit binary present.
    let path = make_path(tmp.path());

    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), Some(&path));
    assert_eq!(code, 0, "plugin must exit 0; stderr: {stderr}");

    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("output not valid JSON: {e}\nout={stdout}"));
    assert_eq!(
        v["verdict"].as_str(),
        Some("warn"),
        "verdict must be warn when binary is missing"
    );
    let findings = v["findings"].as_array().expect("findings is array");
    assert!(
        findings
            .iter()
            .any(|f| f["rule"].as_str() == Some("klasp-plugin-pre-commit/binary-missing")),
        "must have binary-missing finding; findings: {findings:?}"
    );
}

/// 5. When pre-commit emits >100 hook failure lines, findings are capped at 100
///    and a sentinel truncation finding is appended.
#[test]
fn gate_truncates_excessive_findings() {
    let tmp = TempDir::new().expect("tempdir");

    // Generate 200 "Failed" hook lines in a /bin/bash script.
    let mut lines = String::from(
        "#!/bin/bash\ncase \"${1:-}\" in --version) echo 'pre-commit 3.8.0'; exit 0 ;; esac\n",
    );
    for i in 0..200_u32 {
        lines.push_str(&format!(
            "echo \"hook-{i:03}.......................................................................Failed\"\n"
        ));
    }
    lines.push_str("exit 1\n");

    let bin_dir = install_fake_pre_commit(&tmp, &lines);
    let path = make_path(&bin_dir);

    let input = gate_input(0, "commit", &[]);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), Some(&path));
    assert_eq!(code, 0, "plugin must exit 0; stderr: {stderr}");

    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("output not valid JSON: {e}\nout={stdout}"));
    let findings = v["findings"].as_array().expect("findings is array");

    // 100 hook findings + 1 truncation sentinel
    assert_eq!(
        findings.len(),
        101,
        "expected 100 hook findings + 1 truncation sentinel, got {}",
        findings.len()
    );
    let last = &findings[100];
    assert_eq!(
        last["rule"].as_str(),
        Some("klasp-plugin-pre-commit/truncated"),
        "last finding must be truncation sentinel"
    );
    assert!(
        last["message"].as_str().unwrap_or("").contains("not shown"),
        "truncation finding must say 'not shown'"
    );
}

/// 6. When `PluginGateInput.protocol_version` is unknown (e.g. 99), the plugin
///    emits a warn finding but does not crash and does not produce `fail` just from
///    the protocol mismatch.
#[test]
fn gate_input_with_unknown_protocol_version_warns() {
    let tmp = TempDir::new().expect("tempdir");
    let script = "#!/bin/bash\ncase \"${1:-}\" in --version) echo \"pre-commit 3.8.0\"; exit 0 ;; esac\nexit 0\n";
    let bin_dir = install_fake_pre_commit(&tmp, script);
    let path = make_path(&bin_dir);

    let input = gate_input(99, "commit", &[]);
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(&input), Some(&path));
    assert_eq!(
        code, 0,
        "plugin must exit 0 even on unknown version; stderr: {stderr}"
    );

    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("output not valid JSON: {e}\nout={stdout}"));

    // The pre-commit check itself passed (exit 0), so the only finding should be
    // the protocol-warn. Verdict must be warn (not fail).
    assert_eq!(
        v["verdict"].as_str(),
        Some("warn"),
        "with protocol mismatch but passing pre-commit, verdict must be warn; got: {v}"
    );

    let findings = v["findings"].as_array().expect("findings is array");
    assert!(
        findings.iter().any(|f| {
            let rule = f["rule"].as_str().unwrap_or("");
            rule.contains("protocol") || rule.contains("klasp-plugin-pre-commit")
        }),
        "must have a protocol-warn finding; findings: {findings:?}"
    );
}

/// `--gate` with malformed JSON on stdin → exit 0 + warn-verdict JSON.
/// Verifies the plugin honours its documented "exit 0 in all cases" contract
/// even when the input from klasp is broken — this is the path third-party
/// authors are most likely to copy incorrectly.
#[test]
fn gate_with_malformed_input_returns_warn_and_exits_zero() {
    let (code, stdout, stderr) =
        run_plugin(&["--gate"], Some("{not valid json"), None);
    assert_eq!(
        code, 0,
        "plugin must exit 0 on malformed stdin; stderr: {stderr}"
    );

    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("plugin must emit valid JSON even on bad input: {e}\nstdout={stdout}"));
    assert_eq!(v["verdict"].as_str(), Some("warn"));
    let findings = v["findings"].as_array().expect("findings is array");
    assert!(
        findings.iter().any(|f| {
            f["rule"].as_str().unwrap_or("").contains("input-parse-error")
        }),
        "must have an input-parse-error finding; findings: {findings:?}"
    );
}

/// `--gate` with empty stdin → exit 0 + warn-verdict JSON.
#[test]
fn gate_with_empty_input_returns_warn_and_exits_zero() {
    let (code, stdout, stderr) = run_plugin(&["--gate"], Some(""), None);
    assert_eq!(
        code, 0,
        "plugin must exit 0 on empty stdin; stderr: {stderr}"
    );

    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("plugin must emit valid JSON on empty input: {e}\nstdout={stdout}"));
    assert_eq!(v["verdict"].as_str(), Some("warn"));
}
