//! `Pytest` — third named recipe source (v0.2 W6).
//!
//! Translates `[checks.source] type = "pytest"` into a `pytest`
//! invocation, optionally asks pytest to write a JUnit XML report, and
//! maps the exit code to a [`klasp_core::Verdict`]. Exit `0` →
//! [`Verdict::Pass`], `1` → [`Verdict::Fail`] with per-failure findings
//! parsed from the JUnit XML when `junit_xml = true`, falling back to
//! a generic count-based finding when it isn't. Other exit codes
//! (collection errors, internal errors, `KeyboardInterrupt`) map to a
//! [`Verdict::Fail`] whose detail names pytest's documented exit-code
//! semantics.
//!
//! Submodule split: JUnit XML walking lives in [`junit`]; verdict
//! shaping + version sniffing in [`verdict`]. The split keeps each
//! file under the project's 500-line cap and mirrors the W5
//! `fallow.rs` / `fallow/json.rs` layout.

use std::path::Path;
use std::time::Duration;

use klasp_core::{
    CheckConfig, CheckResult, CheckSource, CheckSourceConfig, CheckSourceError, RepoState,
};

use super::shell::{run_with_timeout, ShellOutcome, DEFAULT_TIMEOUT_SECS};

/// Stable identifier this source advertises through `CheckSource::source_id`.
const SOURCE_ID: &str = "pytest";

/// Cap on findings emitted into a verdict so a wall of test failures
/// doesn't drown the agent's stderr.
pub(super) const MAX_FINDINGS: usize = 50;

/// Filename pytest is asked to write its JUnit XML to. Lives at the
/// repo root so a follow-up CI job can pick it up without coordinating
/// paths; pytest re-writes the file on every run so stale data doesn't
/// accumulate.
pub(super) const JUNIT_REPORT_PATH: &str = ".klasp-pytest-junit.xml";

mod junit;
mod verdict;
use verdict::{outcome_to_verdict, sniff_version_warning};

/// `CheckSource` for `type = "pytest"` config entries. Stateless;
/// safe to clone or share.
#[derive(Default)]
pub struct PytestSource {
    _private: (),
}

impl PytestSource {
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl CheckSource for PytestSource {
    fn source_id(&self) -> &str {
        SOURCE_ID
    }

    fn supports_config(&self, config: &CheckConfig) -> bool {
        matches!(config.source, CheckSourceConfig::Pytest { .. })
    }

    fn run(
        &self,
        config: &CheckConfig,
        state: &RepoState,
    ) -> Result<CheckResult, CheckSourceError> {
        let (extra_args, config_path, junit_xml) = match &config.source {
            CheckSourceConfig::Pytest {
                extra_args,
                config_path,
                junit_xml,
            } => (extra_args.clone(), config_path.clone(), *junit_xml),
            other => {
                return Err(CheckSourceError::Other(
                    format!("PytestSource cannot run {other:?}").into(),
                ));
            }
        };

        let want_junit = junit_xml.unwrap_or(false);
        let junit_path = if want_junit {
            Some(state.root.join(JUNIT_REPORT_PATH))
        } else {
            None
        };

        let command = build_command(
            extra_args.as_deref(),
            config_path.as_deref(),
            junit_path.as_deref(),
        );
        let timeout = Duration::from_secs(config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
        let outcome = run_with_timeout(&command, &state.root, &state.base_ref, timeout)?;

        let version_warning = sniff_version_warning(&state.root);
        let junit_payload = junit_path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok());
        let v = outcome_to_verdict(
            &config.name,
            &outcome,
            junit_payload.as_deref(),
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

/// Render the `pytest …` command klasp will hand to `sh -c`. Order:
/// `-c <config>` then `--junitxml=<path>` then `extra_args` so a user
/// who supplies their own `--junitxml=…` in `extra_args` wins (pytest
/// honours the last `--junitxml` on the line).
fn build_command(
    extra_args: Option<&str>,
    config_path: Option<&Path>,
    junit_path: Option<&Path>,
) -> String {
    let mut parts: Vec<String> = vec!["pytest".into()];
    if let Some(path) = config_path {
        parts.push("-c".into());
        parts.push(shell_quote(&path.to_string_lossy()));
    }
    if let Some(path) = junit_path {
        // pytest accepts `--junitxml=<path>`; we emit the `=` form so
        // the shim path doesn't have to deal with arg-splitting in
        // tests.
        parts.push(format!(
            "--junitxml={}",
            shell_quote(&path.to_string_lossy())
        ));
    }
    if let Some(extra) = extra_args {
        let trimmed = extra.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed.to_string());
        }
    }
    parts.join(" ")
}

/// Single-quote a value for inclusion in a `sh -c "<command>"` string.
/// Embedded single quotes become `'\''`, the standard POSIX trick.
fn shell_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use klasp_core::{CheckConfig, CheckSourceConfig};

    use super::*;

    fn pytest_check() -> CheckConfig {
        CheckConfig {
            name: "tests".into(),
            triggers: vec![],
            source: CheckSourceConfig::Pytest {
                extra_args: None,
                config_path: None,
                junit_xml: None,
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
    fn supports_config_only_for_pytest() {
        let source = PytestSource::new();
        assert!(source.supports_config(&pytest_check()));
        assert!(!source.supports_config(&shell_check()));
    }

    #[test]
    fn build_command_minimal() {
        assert_eq!(build_command(None, None, None), "pytest");
    }

    #[test]
    fn build_command_with_config_path_and_junit() {
        let cmd = build_command(
            None,
            Some(Path::new("pytest.ini")),
            Some(Path::new(".klasp-pytest-junit.xml")),
        );
        assert!(cmd.starts_with("pytest -c 'pytest.ini'"));
        assert!(cmd.contains("--junitxml='.klasp-pytest-junit.xml'"));
    }

    #[test]
    fn build_command_with_extra_args_appended_last() {
        let cmd = build_command(Some("-x -q tests/"), None, None);
        assert_eq!(cmd, "pytest -x -q tests/");
    }

    #[test]
    fn build_command_drops_blank_extra_args() {
        // A whitespace-only extra_args field shouldn't smuggle a stray
        // empty token onto the command line — the recipe just runs
        // bare `pytest`.
        assert_eq!(build_command(Some("   "), None, None), "pytest");
    }

    #[test]
    fn shell_quote_handles_embedded_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }
}
