//! Integration test: merge into a realistic fallow-shaped `.claude/settings.json`.
//!
//! Per the W2 issue (#2): "Unit tests for settings.json merge with a real
//! fallow settings.json fixture (proves sibling hooks survive)".

use klasp_agents_claude::ClaudeCodeSurface;
use klasp_agents_claude::{merge_hook_entry, unmerge_hook_entry};
use serde_json::Value;

const FALLOW_FIXTURE: &str = include_str!("fixtures/settings_with_fallow.json");
const KLASP_CMD: &str = ClaudeCodeSurface::HOOK_COMMAND;

fn parse(s: &str) -> Value {
    serde_json::from_str(s).expect("output must be valid JSON")
}

#[test]
fn fallow_fixture_round_trip_preserves_every_sibling() {
    let merged = merge_hook_entry(FALLOW_FIXTURE, KLASP_CMD).expect("merge succeeds");
    let v = parse(&merged);

    // Top-level siblings untouched.
    assert_eq!(v["theme"], "dark");
    assert_eq!(v["permissions"]["allow"][0], "Read");
    assert_eq!(v["permissions"]["allow"][1], "Glob");
    assert_eq!(v["permissions"]["allow"][2], "Grep");
    assert_eq!(v["permissions"]["deny"].as_array().unwrap().len(), 0);
    assert_eq!(v["env"]["FALLOW_AUDIT"], "1");

    // PostToolUse untouched.
    let post = v["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 1);
    assert_eq!(post[0]["matcher"], "Bash");
    assert_eq!(post[0]["hooks"][0]["command"], "fallow record");

    // PreToolUse: still has the non-Bash matcher untouched.
    let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
    let write_edit = pre
        .iter()
        .find(|m| m["matcher"] == "Write|Edit|MultiEdit")
        .expect("Write|Edit|MultiEdit matcher preserved");
    assert_eq!(write_edit["hooks"][0]["command"], "fallow lint --staged");

    // PreToolUse: Bash matcher now has fallow's hook AND klasp's, in that order.
    let bash = pre
        .iter()
        .find(|m| m["matcher"] == "Bash")
        .expect("Bash matcher exists");
    let inner = bash["hooks"].as_array().unwrap();
    assert_eq!(inner.len(), 2, "fallow + klasp = 2");
    assert_eq!(
        inner[0]["command"], "${CLAUDE_PROJECT_DIR}/.claude/hooks/fallow-gate.sh",
        "fallow's entry survives byte-for-byte"
    );
    assert_eq!(inner[0]["timeout"], 5000);
    assert_eq!(inner[1]["command"], KLASP_CMD);
    assert_eq!(inner[1]["type"], "command");
}

#[test]
fn fallow_fixture_re_merge_is_byte_identical() {
    let once = merge_hook_entry(FALLOW_FIXTURE, KLASP_CMD).unwrap();
    let twice = merge_hook_entry(&once, KLASP_CMD).unwrap();
    assert_eq!(once, twice);
}

#[test]
fn fallow_fixture_uninstall_restores_fallow_only() {
    let merged = merge_hook_entry(FALLOW_FIXTURE, KLASP_CMD).unwrap();
    let restored = unmerge_hook_entry(&merged, KLASP_CMD).unwrap();
    let v = parse(&restored);

    // Fallow's Bash hook must still be there.
    let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
    let bash = pre
        .iter()
        .find(|m| m["matcher"] == "Bash")
        .expect("Bash matcher preserved on uninstall");
    let inner = bash["hooks"].as_array().unwrap();
    assert_eq!(inner.len(), 1);
    assert_eq!(
        inner[0]["command"],
        "${CLAUDE_PROJECT_DIR}/.claude/hooks/fallow-gate.sh"
    );

    // Sibling matchers and PostToolUse intact.
    assert!(pre.iter().any(|m| m["matcher"] == "Write|Edit|MultiEdit"));
    assert_eq!(
        v["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
        "fallow record"
    );
}
