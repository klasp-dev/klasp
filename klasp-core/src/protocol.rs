//! Gate wire protocol — versioned, env-var-keyed.
//!
//! Design: [docs/design.md §3.3, §7]. The schema version is held in the
//! `KLASP_GATE_SCHEMA` environment variable exported by the generated hook
//! script, **not** in the JSON stdin payload — this defends against a
//! malicious agent that crafts a `tool_input` field claiming an arbitrary
//! schema version.
//!
//! Versioning is independent of klasp's semver: most binary releases will
//! not bump the schema. Bumping is reserved for genuine wire-format changes
//! (renamed fields, required-field additions, exit-code semantics).

use serde::Deserialize;

/// Wire-protocol schema version. Bump only when the JSON shape, exit-code
/// semantics, or env-var contract changes — *never* for cosmetic releases.
pub const GATE_SCHEMA_VERSION: u32 = 1;

/// The Claude Code `PreToolUse` payload klasp consumes from stdin.
#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct GateInput {
    pub tool_name: String,
    pub tool_input: ToolInput,
}

/// The subset of Claude Code's `tool_input` klasp inspects. Only the `Bash`
/// tool's `command` field matters in v0.1; future fields can be added behind
/// `#[serde(default)]` without bumping the schema.
#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct ToolInput {
    #[serde(default)]
    pub command: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GateError {
    #[error("could not parse gate input as JSON: {0}")]
    Parse(String),
    #[error(
        "klasp-gate: schema mismatch (script={script}, binary={binary}). \
         Re-run `klasp install` to update the hook."
    )]
    SchemaMismatch { script: u32, binary: u32 },
}

pub struct GateProtocol;

impl GateProtocol {
    /// Parse the JSON payload Claude Code writes to the hook's stdin.
    pub fn parse(stdin: &str) -> Result<GateInput, GateError> {
        serde_json::from_str(stdin).map_err(|e| GateError::Parse(e.to_string()))
    }

    /// Compare the env-var schema (set by the shim) with the binary's
    /// compiled-in schema. The shim's value is read from the environment
    /// by the caller and passed in here as a `u32` — this function never
    /// touches the environment itself, keeping it pure and testable.
    pub fn check_schema_env(env_value: u32) -> Result<(), GateError> {
        if env_value == GATE_SCHEMA_VERSION {
            Ok(())
        } else {
            Err(GateError::SchemaMismatch {
                script: env_value,
                binary: GATE_SCHEMA_VERSION,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_claude_payload() {
        let stdin = r#"{
            "tool_name": "Bash",
            "tool_input": { "command": "git commit -m 'wip'" }
        }"#;
        let input = GateProtocol::parse(stdin).expect("should parse");
        assert_eq!(input.tool_name, "Bash");
        assert_eq!(
            input.tool_input.command.as_deref(),
            Some("git commit -m 'wip'")
        );
    }

    #[test]
    fn parses_payload_without_command() {
        let stdin = r#"{ "tool_name": "Read", "tool_input": {} }"#;
        let input = GateProtocol::parse(stdin).expect("should parse");
        assert_eq!(input.tool_name, "Read");
        assert!(input.tool_input.command.is_none());
    }

    #[test]
    fn parses_payload_ignoring_extra_fields() {
        // Forward-compat: unknown future fields must not break parsing.
        let stdin = r#"{
            "tool_name": "Bash",
            "tool_input": { "command": "ls", "extra": 42 },
            "session_id": "abc"
        }"#;
        let input = GateProtocol::parse(stdin).expect("should parse");
        assert_eq!(input.tool_input.command.as_deref(), Some("ls"));
    }

    #[test]
    fn fails_on_malformed_json() {
        let err = GateProtocol::parse("{ not json").expect_err("should fail");
        assert!(matches!(err, GateError::Parse(_)));
    }

    #[test]
    fn fails_on_missing_tool_input() {
        let err = GateProtocol::parse(r#"{ "tool_name": "Bash" }"#).expect_err("should fail");
        assert!(matches!(err, GateError::Parse(_)));
    }

    #[test]
    fn schema_match_passes() {
        assert!(GateProtocol::check_schema_env(GATE_SCHEMA_VERSION).is_ok());
    }

    #[test]
    fn schema_mismatch_returns_error() {
        let err = GateProtocol::check_schema_env(GATE_SCHEMA_VERSION + 1).expect_err("mismatch");
        match err {
            GateError::SchemaMismatch { script, binary } => {
                assert_eq!(script, GATE_SCHEMA_VERSION + 1);
                assert_eq!(binary, GATE_SCHEMA_VERSION);
            }
            other => panic!("expected SchemaMismatch, got {other:?}"),
        }
    }

    #[test]
    fn schema_zero_is_mismatch() {
        // The shim must always export KLASP_GATE_SCHEMA; a missing/zero
        // env var should be treated as a mismatch (the binary defaults to 0
        // when the var is unset, so this guards the fail-open path).
        assert!(GateProtocol::check_schema_env(0).is_err());
    }
}
