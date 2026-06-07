//! Shared harness for the recipe integration tests
//! (`pre_commit_recipe`, `fallow_recipe`, `cargo_recipe`, `pytest_recipe`).
//!
//! Each recipe test drives the real `klasp gate` binary against a fake
//! tool shim on `PATH`. The shim itself is tool-specific (it has to know
//! the binary name and the recipe's `--version` / output contract), but
//! everything around it — locating the `klasp` binary, prepending a
//! directory to `PATH`, spawning `klasp gate` with the Claude-hook stdin
//! payload, and writing fixture / `klasp.toml` files — is identical across
//! all four recipes. Those byte-for-byte duplicates live here.
//!
//! Declared as `mod common;` in each test file. Cargo compiles each
//! integration-test file as its own crate, so some helpers may be unused
//! in any single file; `#![allow(dead_code)]` keeps that from tripping
//! `-D warnings`.

#![allow(dead_code)]

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use klasp_core::GATE_SCHEMA_VERSION;
use tempfile::TempDir;

/// Absolute path to the freshly-built `klasp` binary under test. Cargo
/// exports `CARGO_BIN_EXE_klasp` to integration tests.
pub fn klasp_bin() -> &'static str {
    env!("CARGO_BIN_EXE_klasp")
}

/// Build a `PATH` value with `dir` prepended to the current `PATH` (or
/// just `dir` when `PATH` is unset). Used so the fake tool shim resolves
/// ahead of any real installation on the runner.
pub fn prepend_path(dir: &Path) -> OsString {
    match std::env::var_os("PATH") {
        Some(existing) => {
            let mut prefix = OsString::from(dir.as_os_str());
            prefix.push(":");
            prefix.push(existing);
            prefix
        }
        None => OsString::from(dir.as_os_str()),
    }
}

/// Spawn `klasp gate` with `fake_dir` prepended to `PATH`, feeding
/// `stdin_payload` (a Claude-hook JSON event) on stdin. `extra_env` wires
/// the per-test fixture path / exit code into the shim's env without the
/// harness having to know which fixture is in play.
///
/// Returns the child's exit code and captured stderr (also echoed to the
/// test's own stderr so a failing assertion shows the gate's block message).
pub fn spawn_gate(
    stdin_payload: &str,
    project_dir: &Path,
    fake_dir: &Path,
    extra_env: &[(&str, &str)],
) -> (Option<i32>, String) {
    let path_var = prepend_path(fake_dir);

    let mut cmd = Command::new(klasp_bin());
    cmd.arg("gate")
        .env("KLASP_GATE_SCHEMA", GATE_SCHEMA_VERSION.to_string())
        .env("CLAUDE_PROJECT_DIR", project_dir)
        .env("PATH", &path_var)
        .current_dir(project_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().expect("spawn klasp binary");
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(stdin_payload.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait for klasp");

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !stderr.is_empty() {
        eprintln!("klasp gate stderr:\n{stderr}");
    }
    (output.status.code(), stderr)
}

/// Write a fixture file into `scratch` and return its path. The tool shim
/// reads from here at run time, so the path must outlive the gate child.
pub fn write_fixture(scratch: &TempDir, name: &str, body: &str) -> PathBuf {
    let path = scratch.path().join(name);
    std::fs::write(&path, body).expect("write fixture");
    path
}

/// Write a `klasp.toml` into `project_dir`.
pub fn write_klasp_toml(project_dir: &Path, body: &str) {
    std::fs::write(project_dir.join("klasp.toml"), body).expect("write klasp.toml");
}
