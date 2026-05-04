//! Snapshot test for the generated `klasp-gate.sh`.
//!
//! Per [docs/design.md §10] ("Snapshot tests"): when the template changes,
//! the developer must review the diff explicitly. Run `cargo insta review`
//! to accept changes.

use klasp_agents_claude::render_hook_script;

#[test]
fn hook_script_v1_snapshot() {
    insta::assert_snapshot!("hook_script_v1", render_hook_script(1));
}

#[test]
fn hook_script_v7_snapshot() {
    // Ensures the schema version interpolates everywhere it should — bumping
    // the constant in klasp-core changes both the marker comment and the
    // export, but nothing else.
    insta::assert_snapshot!("hook_script_v7", render_hook_script(7));
}
