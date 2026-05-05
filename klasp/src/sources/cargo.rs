//! `Cargo` — fourth named recipe source (v0.2 W6).
//!
//! Translates `[checks.source] type = "cargo"` into `cargo <subcommand>
//! [-p <package>] [--workspace] [<extra_args>]` and maps cargo's exit
//! code to a [`klasp_core::Verdict`]. For `check` / `clippy` / `build`
//! the recipe asks cargo for `--message-format=json` and walks the
//! `compiler-message` stream to extract per-diagnostic findings; for
//! `cargo test` it parses the trailing `test result: <status>. N
//! passed; M failed` summary line because cargo's JSON test reporter
//! is still nightly-gated.
//!
//! Submodule split: per-message diagnostic walking lives in
//! [`messages`]; verdict shaping + version sniffing in [`verdict`].
//! The split keeps each file under the project's 500-line cap and
//! mirrors the W5 `fallow.rs` / `fallow/json.rs` layout.

use std::time::Duration;

use klasp_core::{
    CheckConfig, CheckResult, CheckSource, CheckSourceConfig, CheckSourceError, RepoState,
};

use super::shell::{run_with_timeout, ShellOutcome, DEFAULT_TIMEOUT_SECS};

/// Stable identifier this source advertises through `CheckSource::source_id`.
const SOURCE_ID: &str = "cargo";

/// Cap on findings emitted into a verdict so a wall of compiler errors
/// from a fresh checkout doesn't drown the agent's stderr.
pub(super) const MAX_FINDINGS: usize = 50;

/// Allowed values for `subcommand`. Mirrored in [`docs/recipes.md`] so
/// the user-facing documentation has a single canonical source.
const ALLOWED_SUBCOMMANDS: &[&str] = &["check", "clippy", "test", "build"];

mod messages;
mod verdict;
use verdict::{fail_with_optional_warning, outcome_to_verdict, sniff_version_warning};

/// `CheckSource` for `type = "cargo"` config entries. Stateless;
/// safe to clone or share.
#[derive(Default)]
pub struct CargoSource {
    _private: (),
}

impl CargoSource {
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl CheckSource for CargoSource {
    fn source_id(&self) -> &str {
        SOURCE_ID
    }

    fn supports_config(&self, config: &CheckConfig) -> bool {
        matches!(config.source, CheckSourceConfig::Cargo { .. })
    }

    fn run(
        &self,
        config: &CheckConfig,
        state: &RepoState,
    ) -> Result<CheckResult, CheckSourceError> {
        let (subcommand, extra_args, package) = match &config.source {
            CheckSourceConfig::Cargo {
                subcommand,
                extra_args,
                package,
            } => (subcommand.clone(), extra_args.clone(), package.clone()),
            other => {
                return Err(CheckSourceError::Other(
                    format!("CargoSource cannot run {other:?}").into(),
                ));
            }
        };

        let version_warning = sniff_version_warning(&state.root);

        if !ALLOWED_SUBCOMMANDS.contains(&subcommand.as_str()) {
            let detail = format!(
                "cargo recipe `{}`: unknown subcommand `{subcommand}` \
                 (expected one of: {})",
                config.name,
                ALLOWED_SUBCOMMANDS.join(", ")
            );
            let v = fail_with_optional_warning(&config.name, detail, version_warning.as_deref());
            return Ok(CheckResult {
                source_id: SOURCE_ID.to_string(),
                check_name: config.name.clone(),
                verdict: v,
                raw_stdout: None,
                raw_stderr: None,
            });
        }

        let command = build_command(&subcommand, package.as_deref(), extra_args.as_deref());
        let timeout = Duration::from_secs(config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
        let outcome = run_with_timeout(&command, &state.root, &state.base_ref, timeout)?;

        let v = outcome_to_verdict(
            &config.name,
            &subcommand,
            &outcome,
            version_warning.as_deref(),
        );

        Ok(CheckResult {
            source_id: SOURCE_ID.to_string(),
            check_name: config.name.clone(),
            verdict: v,
            raw_stdout: Some(outcome.stdout),
            raw_stderr: Some(outcome.stderr),
        })
    }
}

/// Render the `cargo …` command klasp will hand to `sh -c`.
///
/// Order of args:
/// 1. `cargo <sub>`
/// 2. `-p <package>` *or* `--workspace` (mutually exclusive — package
///    overrides because it's the user's narrower intent).
/// 3. `--message-format=json` for non-test subcommands, **unless** the
///    user already supplied a `--message-format=` flag in `extra_args`.
///    Without that carve-out, the user's flag would win at the cargo
///    side (cargo honours the last `--message-format`) but the recipe
///    would still attempt to JSON-walk the resulting non-JSON output
///    and silently emit an empty diagnostics list.
/// 4. `extra_args` last so the user can extend the recipe's defaults
///    (e.g. clipping `-- -D warnings` onto `cargo clippy`).
fn build_command(subcommand: &str, package: Option<&str>, extra_args: Option<&str>) -> String {
    let mut parts: Vec<String> = vec!["cargo".into(), subcommand.to_string()];

    match package {
        Some(p) => {
            parts.push("-p".into());
            parts.push(shell_quote(p));
        }
        None => {
            parts.push("--workspace".into());
        }
    }

    let user_overrides_format = extra_args
        .map(|s| s.contains("--message-format"))
        .unwrap_or(false);
    if subcommand != "test" && !user_overrides_format {
        parts.push("--message-format=json".into());
    }

    if let Some(extra) = extra_args {
        let trimmed = extra.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed.to_string());
        }
    }

    parts.join(" ")
}

fn shell_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use klasp_core::{CheckConfig, CheckSourceConfig, GitEvent, RepoState, Verdict};

    use super::*;

    fn cargo_check_config(subcommand: &str) -> CheckConfig {
        CheckConfig {
            name: "build".into(),
            triggers: vec![],
            source: CheckSourceConfig::Cargo {
                subcommand: subcommand.into(),
                extra_args: None,
                package: None,
            },
            timeout_secs: None,
        }
    }

    fn shell_check() -> CheckConfig {
        CheckConfig {
            name: "shell".into(),
            triggers: vec![],
            source: CheckSourceConfig::Shell {
                command: "true".into(),
            },
            timeout_secs: None,
        }
    }

    #[test]
    fn supports_config_only_for_cargo() {
        let source = CargoSource::new();
        assert!(source.supports_config(&cargo_check_config("check")));
        assert!(!source.supports_config(&shell_check()));
    }

    #[test]
    fn build_command_workspace_default() {
        let cmd = build_command("check", None, None);
        assert_eq!(cmd, "cargo check --workspace --message-format=json");
    }

    #[test]
    fn build_command_with_package_skips_workspace() {
        let cmd = build_command("clippy", Some("klasp-core"), None);
        assert_eq!(cmd, "cargo clippy -p 'klasp-core' --message-format=json");
    }

    #[test]
    fn build_command_test_skips_message_format() {
        let cmd = build_command("test", None, None);
        assert_eq!(cmd, "cargo test --workspace");
    }

    #[test]
    fn build_command_appends_extra_args_last() {
        let cmd = build_command("clippy", None, Some("-- -D warnings"));
        assert_eq!(
            cmd,
            "cargo clippy --workspace --message-format=json -- -D warnings"
        );
    }

    #[test]
    fn build_command_drops_blank_extra_args() {
        let cmd = build_command("check", None, Some("   "));
        assert_eq!(cmd, "cargo check --workspace --message-format=json");
    }

    #[test]
    fn shell_quote_handles_embedded_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn unknown_subcommand_fails_without_running_cargo() {
        let cfg = CheckConfig {
            name: "build".into(),
            triggers: vec![],
            source: CheckSourceConfig::Cargo {
                subcommand: "uninstall".into(),
                extra_args: None,
                package: None,
            },
            timeout_secs: None,
        };
        let state = RepoState {
            root: std::env::temp_dir(),
            git_event: GitEvent::Commit,
            base_ref: "HEAD~1".into(),
        };
        let result = CargoSource::new()
            .run(&cfg, &state)
            .expect("unknown subcommand surfaces as Verdict::Fail, not a runtime error");
        match result.verdict {
            Verdict::Fail { message, .. } => {
                assert!(message.contains("uninstall"));
                assert!(message.contains("expected one of"));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }
}
