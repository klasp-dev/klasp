//! Plugin-protocol smoke test — issue #46 acceptance item 2.
//!
//! End-to-end validation that the `klasp-plugin-pre-commit` reference plugin
//! (in `examples/klasp-plugin-pre-commit/`) builds standalone (no klasp-core
//! dep) and speaks the v0 plugin protocol via `--describe`. This proves
//! third-party plugin viability against the real binary, not a mock.
//!
//! Pass/fail gate-dispatch behaviour against mock plugins is already covered
//! by `klasp/tests/plugin_protocol.rs`; this file does NOT duplicate that.

use std::path::PathBuf;
use std::process::Command;

/// Resolve the path to the `examples/klasp-plugin-pre-commit/` directory.
fn plugin_example_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    // manifest is klasp/  — go up one level to workspace root, then into examples.
    manifest
        .parent()
        .unwrap_or(&manifest)
        .join("examples")
        .join("klasp-plugin-pre-commit")
}

/// Build the reference plugin binary and assert `--describe` reports
/// `protocol_version = 0`. Hard-fails on cargo or build error — a launch-gating
/// smoke test should be loud, not best-effort.
#[test]
fn plugin_pre_commit_protocol_version_v0() {
    let example_dir = plugin_example_dir();

    let build_output = Command::new("cargo")
        .args(["build", "--bin", "klasp-plugin-pre-commit"])
        .current_dir(&example_dir)
        .output()
        .expect("cargo must be on PATH for the launch smoke test");

    assert!(
        build_output.status.success(),
        "klasp-plugin-pre-commit build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr),
    );

    let plugin_bin = example_dir
        .join("target")
        .join("debug")
        .join("klasp-plugin-pre-commit");
    assert!(
        plugin_bin.exists(),
        "built binary missing at {}",
        plugin_bin.display(),
    );

    let describe_out = Command::new(&plugin_bin)
        .arg("--describe")
        .output()
        .expect("spawn --describe");

    assert!(
        describe_out.status.success(),
        "--describe must exit 0; stderr: {}",
        String::from_utf8_lossy(&describe_out.stderr)
    );

    let json_str = String::from_utf8_lossy(&describe_out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(json_str.trim()).expect("--describe must emit valid JSON");

    let protocol_version = json
        .get("protocol_version")
        .and_then(|v| v.as_u64())
        .expect("--describe JSON must have 'protocol_version' field");
    assert_eq!(
        protocol_version, 0,
        "PLUGIN_PROTOCOL_VERSION must be 0 (experimental); got {protocol_version}"
    );

    let name = json
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        name.starts_with("klasp-plugin-"),
        "plugin name must start with 'klasp-plugin-'; got {name:?}"
    );

    let verdict_v0 = json
        .get("supports")
        .and_then(|s| s.get("verdict_v0"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(
        verdict_v0,
        "--describe must declare supports.verdict_v0 = true"
    );
}
