//! User-configurable `[[trigger]]` blocks parsed from `klasp.toml`.
//!
//! Design: issue #45. User triggers extend the built-in commit/push regex
//! (see `trigger.rs`) — they add new matching rules without replacing the
//! built-ins. A command matched by any user trigger fires the gate just
//! as a built-in match would.
//!
//! Validation rules (enforced at config-load time, not at match time):
//! - At least one of `pattern` or `commands` must be present.
//! - `pattern` must be a valid Rust regex (compiled eagerly to catch errors early).
//! - `agents` is optional — empty means "fire for all agents".

use regex::Regex;
use serde::{de, Deserialize, Serialize};

use crate::error::{KlaspError, Result};

/// A single user-defined `[[trigger]]` block from `klasp.toml`.
///
/// ```toml
/// [[trigger]]
/// name = "jj-push"
/// pattern = "^jj git push"
/// agents = ["claude_code"]
/// commands = ["jj git push", "jj git push -m main"]
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UserTriggerConfig {
    /// Human-readable name for error messages and diagnostics.
    pub name: String,

    /// Optional regex matched against the full tool-input command string.
    /// Must compile as a Rust regex. At least one of `pattern` / `commands`
    /// is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,

    /// Restrict this trigger to specific agents (e.g. `["claude_code"]`).
    /// Empty or absent means the trigger fires for all agents.
    #[serde(default)]
    pub agents: Vec<String>,

    /// Exact command strings that fire this trigger.  Matched after `pattern`
    /// — a command that matches either fires the gate.
    #[serde(default)]
    pub commands: Vec<String>,
}

/// A compiled, validated user trigger ready for matching.
///
/// Constructed from [`UserTriggerConfig`] via [`UserTrigger::validate`].
/// `Regex` is `Clone` (cheap, internally `Arc`), so cloning a `UserTrigger`
/// is cheap; the regex's compiled state is shared rather than re-compiled.
#[derive(Debug, Clone)]
pub struct UserTrigger {
    pub name: String,
    pub pattern: Option<Regex>,
    pub agents: Vec<String>,
    pub commands: Vec<String>,
}

impl UserTrigger {
    /// Validate and compile a [`UserTriggerConfig`] into a [`UserTrigger`].
    ///
    /// Returns `KlaspError::ConfigParse` if:
    /// - Both `pattern` and `commands` are absent/empty (at least one required).
    /// - `pattern` is present but is not a valid regex.
    pub fn validate(cfg: &UserTriggerConfig) -> Result<Self> {
        let has_pattern = cfg.pattern.is_some();
        let has_commands = !cfg.commands.is_empty();

        if !has_pattern && !has_commands {
            return Err(KlaspError::ConfigParse(
                <toml::de::Error as de::Error>::custom(format!(
                    "trigger {:?}: at least one of `pattern` or `commands` is required",
                    cfg.name
                )),
            ));
        }

        let pattern = match &cfg.pattern {
            Some(p) => Some(Regex::new(p).map_err(|e| {
                KlaspError::ConfigParse(<toml::de::Error as de::Error>::custom(format!(
                    "trigger {:?}: invalid regex {:?}: {e}",
                    cfg.name, p
                )))
            })?),
            None => None,
        };

        Ok(UserTrigger {
            name: cfg.name.clone(),
            pattern,
            agents: cfg.agents.clone(),
            commands: cfg.commands.clone(),
        })
    }

    /// Returns `true` if this trigger matches `cmd` for the given `agent`.
    ///
    /// Matching logic:
    ///
    /// 1. If `agents` is non-empty, `agent` must be listed.
    /// 2. If `pattern` is set, it is tested against the full `cmd`.
    /// 3. If `commands` is non-empty, `cmd` is tested for an exact match.
    ///
    /// A `pattern` or `commands` match is sufficient — they are OR'd.
    pub fn matches(&self, cmd: &str, agent: &str) -> bool {
        if !self.agents.is_empty() && !self.agents.iter().any(|a| a == agent) {
            return false;
        }
        let pattern_match = self.pattern.as_ref().is_some_and(|re| re.is_match(cmd));
        let command_match = self.commands.iter().any(|c| c == cmd);
        pattern_match || command_match
    }
}

/// Validate all `[[trigger]]` entries from config, returning compiled triggers.
///
/// Called once during config load so bad regexes fail early rather than
/// silently at gate-run time. The returned `Vec` is in the same order as
/// the input slice.
pub fn validate_user_triggers(cfgs: &[UserTriggerConfig]) -> Result<Vec<UserTrigger>> {
    cfgs.iter().map(UserTrigger::validate).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(
        name: &str,
        pattern: Option<&str>,
        agents: &[&str],
        commands: &[&str],
    ) -> UserTriggerConfig {
        UserTriggerConfig {
            name: name.into(),
            pattern: pattern.map(String::from),
            agents: agents.iter().map(|s| s.to_string()).collect(),
            commands: commands.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn pattern_only_trigger_validates() {
        let t = UserTrigger::validate(&cfg("t", Some("^jj"), &[], &[])).unwrap();
        assert!(t.pattern.is_some());
    }

    #[test]
    fn commands_only_trigger_validates() {
        let t = UserTrigger::validate(&cfg("t", None, &[], &["gh pr create"])).unwrap();
        assert!(t.pattern.is_none());
        assert_eq!(t.commands, vec!["gh pr create"]);
    }

    #[test]
    fn no_pattern_no_commands_is_error() {
        let err = UserTrigger::validate(&cfg("empty", None, &[], &[])).unwrap_err();
        assert!(matches!(err, KlaspError::ConfigParse(_)));
    }

    #[test]
    fn invalid_regex_is_error() {
        let err = UserTrigger::validate(&cfg("bad", Some("[invalid"), &[], &[])).unwrap_err();
        assert!(matches!(err, KlaspError::ConfigParse(_)));
    }

    #[test]
    fn pattern_matches_command() {
        let t = UserTrigger::validate(&cfg("t", Some("^jj git push"), &[], &[])).unwrap();
        assert!(t.matches("jj git push -m main", "claude_code"));
    }

    #[test]
    fn pattern_does_not_match_unrelated_command() {
        let t = UserTrigger::validate(&cfg("t", Some("^jj git push"), &[], &[])).unwrap();
        assert!(!t.matches("git push origin main", "claude_code"));
    }

    #[test]
    fn commands_allowlist_exact_match() {
        let t = UserTrigger::validate(&cfg("t", None, &[], &["gh pr create"])).unwrap();
        assert!(t.matches("gh pr create", "claude_code"));
        assert!(!t.matches("gh pr create --draft", "claude_code"));
    }

    #[test]
    fn agents_filter_blocks_unlisted_agent() {
        let t = UserTrigger::validate(&cfg("t", Some("^jj"), &["claude_code"], &[])).unwrap();
        assert!(t.matches("jj git push", "claude_code"));
        assert!(!t.matches("jj git push", "codex"));
    }

    #[test]
    fn empty_agents_matches_any_agent() {
        let t = UserTrigger::validate(&cfg("t", Some("^jj"), &[], &[])).unwrap();
        assert!(t.matches("jj git push", "codex"));
        assert!(t.matches("jj git push", "claude_code"));
    }
}
