//! Schema-version contract test.
//!
//! Per [docs/design.md §7], the wire-protocol schema is committed in three
//! places that must agree:
//!
//! 1. The `klasp_core::GATE_SCHEMA_VERSION` constant compiled into the
//!    binary.
//! 2. The `KLASP_GATE_SCHEMA=N` line exported by the rendered hook script
//!    `klasp install` would write today (read here from the
//!    `klasp_agents_claude::ClaudeCodeSurface::render_hook_script` output).
//! 3. The committed `tests/fixtures/klasp-gate-v1.sh` golden file, which
//!    represents the script content for `GATE_SCHEMA_VERSION = 1`.
//!
//! All three numbers must be identical. A developer who bumps the binary's
//! constant cannot satisfy the test by editing only the renderer or only
//! the fixture: the test demands every place agree, surfacing the bump in
//! review rather than letting a stale shim silently fail open at runtime.

use klasp_agents_claude::render_hook_script;
use klasp_core::GATE_SCHEMA_VERSION;

const FIXTURE_SCRIPT: &str = include_str!("fixtures/klasp-gate-v1.sh");

/// Pull the integer `N` out of an `export KLASP_GATE_SCHEMA=N` line.
/// Returns `None` if the export is missing or unparseable, which the
/// individual test asserts against directly so the failure message names
/// the offending source.
fn parse_schema_export(script: &str) -> Option<u32> {
    for raw in script.lines() {
        let line = raw.trim();
        let body = line.strip_prefix("export ").unwrap_or(line);
        if let Some(rest) = body.strip_prefix("KLASP_GATE_SCHEMA=") {
            // Strip an optional trailing comment, surrounding whitespace,
            // and surrounding quotes — bash exports are forgiving.
            let value = rest.split('#').next().unwrap_or("").trim();
            let value = value.trim_matches(|c: char| c == '"' || c == '\'');
            if let Ok(n) = value.parse::<u32>() {
                return Some(n);
            }
        }
    }
    None
}

#[test]
fn fixture_script_exports_current_schema() {
    let n = parse_schema_export(FIXTURE_SCRIPT).expect(
        "tests/fixtures/klasp-gate-v1.sh must contain a parseable \
         `export KLASP_GATE_SCHEMA=N` line",
    );
    assert_eq!(
        n, GATE_SCHEMA_VERSION,
        "fixture script's KLASP_GATE_SCHEMA ({n}) disagrees with binary's \
         GATE_SCHEMA_VERSION ({GATE_SCHEMA_VERSION}). Bumping the constant \
         requires re-rendering tests/fixtures/klasp-gate-v1.sh — both must \
         agree."
    );
}

#[test]
fn rendered_script_exports_current_schema() {
    let rendered = render_hook_script(GATE_SCHEMA_VERSION);
    let n = parse_schema_export(&rendered).expect(
        "render_hook_script output must contain a parseable \
         `export KLASP_GATE_SCHEMA=N` line",
    );
    assert_eq!(
        n, GATE_SCHEMA_VERSION,
        "rendered script's KLASP_GATE_SCHEMA ({n}) disagrees with binary's \
         GATE_SCHEMA_VERSION ({GATE_SCHEMA_VERSION}). Renderer and constant \
         must agree."
    );
}

#[test]
fn fixture_and_rendered_script_agree() {
    // Belt-and-braces: the previous two tests pin both sources to
    // GATE_SCHEMA_VERSION, but the contract is *equality* between them.
    // Asserting that explicitly produces a clearer error message when the
    // mismatch case ever arises.
    let rendered = render_hook_script(GATE_SCHEMA_VERSION);
    let from_fixture =
        parse_schema_export(FIXTURE_SCRIPT).expect("fixture must contain KLASP_GATE_SCHEMA");
    let from_rendered =
        parse_schema_export(&rendered).expect("rendered script must contain KLASP_GATE_SCHEMA");
    assert_eq!(
        from_fixture, from_rendered,
        "fixture script and freshly-rendered script disagree on \
         KLASP_GATE_SCHEMA — the fixture is stale. Re-render via \
         klasp_agents_claude::render_hook_script and commit the result.",
    );
}
