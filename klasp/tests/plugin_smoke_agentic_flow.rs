//! Plugin-protocol smoke test for the `klasp-plugin-agentic-flow` reference
//! plugin — issue #98.
//!
//! End-to-end validation that the plugin (in `examples/klasp-plugin-agentic-flow/`)
//! builds standalone (no klasp-core dep) and speaks the v0 plugin protocol via
//! `--describe`. This proves third-party plugin viability against the real
//! binary, not a mock.
//!
//! Audit correctness (missing/stale/unconfirmed/skip/unknown/infra handling) is
//! covered by the plugin's own `examples/klasp-plugin-agentic-flow/tests/integration.rs`;
//! this file does NOT duplicate that. It mirrors `plugin_smoke.rs` exactly so
//! each reference plugin's launch-gating smoke test stays isolated.

use std::path::PathBuf;
use std::process::Command;

/// Resolve the path to the `examples/klasp-plugin-agentic-flow/` directory.
fn plugin_example_dir() -> PathBuf {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    // manifest is klasp/  — go up one level to workspace root, then into examples.
    manifest
        .parent()
        .unwrap_or(&manifest)
        .join("examples")
        .join("klasp-plugin-agentic-flow")
}

/// Build the reference plugin binary and assert `--describe` reports
/// `protocol_version = 0`. Hard-fails on cargo or build error — a launch-gating
/// smoke test should be loud, not best-effort.
#[test]
fn plugin_agentic_flow_protocol_version_v0() {
    let example_dir = plugin_example_dir();

    let build_output = Command::new("cargo")
        .args(["build", "--bin", "klasp-plugin-agentic-flow"])
        .current_dir(&example_dir)
        .output()
        .expect("cargo must be on PATH for the launch smoke test");

    assert!(
        build_output.status.success(),
        "klasp-plugin-agentic-flow build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr),
    );

    let plugin_bin = example_dir
        .join("target")
        .join("debug")
        .join("klasp-plugin-agentic-flow");
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
