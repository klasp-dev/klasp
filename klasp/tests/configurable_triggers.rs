//! Integration tests for user-configurable `[[trigger]]` blocks (#45).
//!
//! Tests verify parsing, validation, and matching logic for the
//! `UserTriggerConfig` → `UserTrigger` pipeline.

use klasp_core::ConfigV1;

// ── Helper ──────────────────────────────────────────────────────────────────

fn parse(toml: &str) -> ConfigV1 {
    ConfigV1::parse(toml).expect("config should parse")
}

fn should_fail(toml: &str) {
    ConfigV1::parse(toml).expect_err("config should fail to parse");
}

// ── 1. Pattern trigger fires on matching command ─────────────────────────────

#[test]
fn user_trigger_with_pattern_fires_on_match() {
    let toml = r#"
        version = 1
        [gate]

        [[trigger]]
        name = "jj-push"
        pattern = "^jj git push"
    "#;
    let config = parse(toml);
    assert_eq!(config.triggers.len(), 1);
    let triggers = config.compiled_triggers();
    assert!(triggers[0].matches("jj git push -m main", "claude_code"));
}

// ── 2. Pattern trigger doesn't fire on non-matching command ──────────────────

#[test]
fn user_trigger_with_pattern_no_match_passes_through() {
    let toml = r#"
        version = 1
        [gate]

        [[trigger]]
        name = "jj-push"
        pattern = "^jj git push"
    "#;
    let config = parse(toml);
    let triggers = config.compiled_triggers();
    // A standard git push should not match the jj-specific pattern.
    assert!(!triggers[0].matches("git push origin main", "claude_code"));
}

// ── 3. Agents filter blocks unlisted agents ───────────────────────────────────

#[test]
fn user_trigger_with_agents_filter_blocks_other_agents() {
    let toml = r#"
        version = 1
        [gate]

        [[trigger]]
        name = "jj-push"
        pattern = "^jj"
        agents = ["claude_code"]
    "#;
    let config = parse(toml);
    let triggers = config.compiled_triggers();
    // claude_code should match.
    assert!(triggers[0].matches("jj git push", "claude_code"));
    // codex should not match because it isn't in the agents list.
    assert!(!triggers[0].matches("jj git push", "codex"));
}

// ── 4. Commands allowlist matches exact literal ───────────────────────────────

#[test]
fn user_trigger_with_commands_allowlist_matches_literal() {
    let toml = r#"
        version = 1
        [gate]

        [[trigger]]
        name = "gh-pr"
        commands = ["gh pr create"]
    "#;
    let config = parse(toml);
    let triggers = config.compiled_triggers();
    assert!(triggers[0].matches("gh pr create", "claude_code"));
    // Partial matches should NOT fire.
    assert!(!triggers[0].matches("gh pr create --draft", "claude_code"));
}

// ── 5. User triggers extend built-in triggers ─────────────────────────────────

#[test]
fn user_trigger_extends_built_in_triggers() {
    // Config has both a user trigger and a check with no trigger filter.
    // Both the built-in git-commit detection and the user trigger should
    // co-exist — parsing doesn't remove built-in support.
    let toml = r#"
        version = 1
        [gate]

        [[trigger]]
        name = "jj-push"
        pattern = "^jj git push"

        [[checks]]
        name = "fmt"
        [checks.source]
        type = "shell"
        command = "true"
    "#;
    let config = parse(toml);
    // User triggers are in config.triggers.
    assert_eq!(config.triggers.len(), 1);
    // Built-in Trigger::classify still works independently.
    use klasp_core::Trigger;
    assert!(Trigger::classify("git commit -m 'wip'").is_some());
    assert!(Trigger::classify("git push origin main").is_some());
    // And the user trigger compiles fine alongside it.
    let triggers = config.compiled_triggers();
    assert!(triggers[0].matches("jj git push -m main", "any_agent"));
}

// ── 6. Invalid regex in user trigger is a config error ───────────────────────

#[test]
fn invalid_user_trigger_pattern_emits_config_error() {
    let toml = r#"
        version = 1
        [gate]

        [[trigger]]
        name = "bad-regex"
        pattern = "[this is not valid regex("
    "#;
    should_fail(toml);
}

// ── 7. Trigger with no pattern and no commands is a config error ──────────────

#[test]
fn user_trigger_no_pattern_no_commands_is_config_error() {
    let toml = r#"
        version = 1
        [gate]

        [[trigger]]
        name = "empty-trigger"
    "#;
    should_fail(toml);
}

// ── 8. Fixture round-trip: jj-push fixture parses and matches correctly ───────

#[test]
fn fixture_jj_push_parses_and_matches() {
    let fixture = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/trigger/user-trigger-jj-push.toml"
    ))
    .expect("fixture missing");
    let config = parse(&fixture);
    let triggers = config.compiled_triggers();
    assert_eq!(triggers.len(), 1, "expected one user trigger");
    // Pattern match.
    assert!(triggers[0].matches("jj git push --no-ff", "claude_code"));
    // Commands allowlist exact match.
    assert!(triggers[0].matches("jj git push", "claude_code"));
    assert!(triggers[0].matches("jj git push -m main", "claude_code"));
    // Agent filter: codex should not match.
    assert!(!triggers[0].matches("jj git push", "codex"));
}

// ── 9. Fixture round-trip: gh-pr fixture parses and matches correctly ─────────

#[test]
fn fixture_gh_pr_parses_and_matches() {
    let fixture = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/trigger/user-trigger-gh-pr.toml"
    ))
    .expect("fixture missing");
    let config = parse(&fixture);
    let triggers = config.compiled_triggers();
    assert_eq!(triggers.len(), 1, "expected one user trigger");
    // Exact command match only.
    assert!(triggers[0].matches("gh pr create", "claude_code"));
    assert!(!triggers[0].matches("gh pr create --draft", "claude_code"));
    // No agent filter on this trigger.
    assert!(triggers[0].matches("gh pr create", "codex"));
}
