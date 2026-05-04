//! `PreCommit` — first named recipe source (v0.2 W4).
//!
//! Translates a `[checks.source] type = "pre_commit"` config entry into a
//! `pre-commit run …` shell invocation, then maps pre-commit's stage-aware
//! exit code to a [`klasp_core::Verdict`]. Sibling to
//! [`super::shell::ShellSource`]: both impls route off the
//! `CheckSourceConfig` tag, the v0.2 named-recipe extension shape
//! committed to in [docs/design.md §3.2].
//!
//! Exit-code mapping: `0` → [`Verdict::Pass`], `1` → [`Verdict::Fail`]
//! with per-hook findings parsed from stdout (`"<hook>....Failed"` lines,
//! format stable across pre-commit 3.x and 4.x — see fixtures at
//! `tests/fixtures/pre_commit/`), other codes → [`Verdict::Fail`] with
//! a generic "unexpected exit" finding naming the code. Version sniffing
//! is lazy and silent on probe failure (some wrappers don't honour
//! `--version`); a version outside [`MIN_SUPPORTED_VERSION`] /
//! [`MAX_TESTED_VERSION`] surfaces a `Verdict::Warn` rather than blocking.
//!
//! `verdict_path` is deferred per [docs/design.md §14] — the recipe knows
//! pre-commit's output format internally, no generic JSON-path projection
//! needed.

use std::process::Command;
use std::time::Duration;

use klasp_core::{
    CheckConfig, CheckResult, CheckSource, CheckSourceConfig, CheckSourceError, Finding, RepoState,
    Severity, Verdict,
};

use super::shell::{run_with_timeout, ShellOutcome, DEFAULT_TIMEOUT_SECS};

/// Stable identifier this source advertises through `CheckSource::source_id`.
const SOURCE_ID: &str = "pre_commit";

/// Default `--hook-stage` when the user omits it. Matches what
/// `pre-commit run` does when invoked from a `.git/hooks/pre-commit`
/// shim, so klasp's gate fires the same hooks a human commit would.
const DEFAULT_HOOK_STAGE: &str = "pre-commit";

/// Default `-c <path>` when the user omits `config_path`. Documented but
/// unused at run time: pre-commit defaults to this filename internally,
/// so the recipe omits the `-c` flag entirely when `config_path` is
/// `None` rather than passing the same value pre-commit would pick on
/// its own. Kept here so the user-facing default in [`docs/recipes.md`]
/// has a single canonical source.
#[allow(dead_code)]
const DEFAULT_CONFIG_PATH: &str = ".pre-commit-config.yaml";

/// Lowest pre-commit release whose stdout format matches the parser
/// in [`parse_failed_hooks`]. 3.0 dropped Python 2 support and rewrote
/// the per-hook summary line format; 2.x is out of scope.
const MIN_SUPPORTED_VERSION: (u32, u32) = (3, 0);

/// Highest pre-commit major.minor we've actively tested. New majors that
/// land while klasp is unmaintained still work — the recipe emits a
/// stderr notice and keeps running, on the bet that pre-commit's stable
/// stdout format is stable. The notice gives operators a breadcrumb
/// when something does break.
const MAX_TESTED_VERSION: (u32, u32) = (4, 0);

/// `CheckSource` for `type = "pre_commit"` config entries. Stateless;
/// safe to clone or share. Constructed once via
/// [`super::SourceRegistry::default_v1`].
#[derive(Default)]
pub struct PreCommitSource {
    _private: (),
}

impl PreCommitSource {
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl CheckSource for PreCommitSource {
    fn source_id(&self) -> &str {
        SOURCE_ID
    }

    fn supports_config(&self, config: &CheckConfig) -> bool {
        matches!(config.source, CheckSourceConfig::PreCommit { .. })
    }

    fn run(
        &self,
        config: &CheckConfig,
        state: &RepoState,
    ) -> Result<CheckResult, CheckSourceError> {
        let (hook_stage, config_path) = match &config.source {
            CheckSourceConfig::PreCommit {
                hook_stage,
                config_path,
            } => (
                hook_stage
                    .as_deref()
                    .unwrap_or(DEFAULT_HOOK_STAGE)
                    .to_string(),
                config_path.clone(),
            ),
            other => {
                return Err(CheckSourceError::Other(
                    format!("PreCommitSource cannot run {other:?}").into(),
                ));
            }
        };

        let command = build_command(&hook_stage, config_path.as_deref(), &state.base_ref);
        let timeout = Duration::from_secs(config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
        let outcome = run_with_timeout(&command, &state.root, &state.base_ref, timeout)?;

        let version_warning = sniff_version_warning(&state.root);
        let verdict = outcome_to_verdict(&config.name, &outcome, version_warning.as_deref());

        Ok(CheckResult {
            source_id: SOURCE_ID.to_string(),
            check_name: config.name.clone(),
            verdict,
            raw_stdout: Some(outcome.stdout),
            raw_stderr: Some(outcome.stderr),
        })
    }
}

/// Render the `pre-commit run …` command klasp will hand to `sh -c`.
///
/// `${KLASP_BASE_REF}` is preferred over the resolved-at-build-time
/// `state.base_ref` because the env var is the documented contract for
/// every klasp shell-flavoured source — keeping the recipes consistent
/// with the v0.1 user-authored `command = "…"` form means a copy-paste
/// from one to the other is mechanical. The shell source's
/// `run_with_timeout` exports the var into the child env identically.
fn build_command(
    hook_stage: &str,
    config_path: Option<&std::path::Path>,
    _base_ref: &str,
) -> String {
    let mut parts: Vec<String> = vec!["pre-commit".into(), "run".into()];
    parts.push("--hook-stage".into());
    parts.push(shell_quote(hook_stage));
    parts.push("--from-ref".into());
    parts.push("${KLASP_BASE_REF}".into());
    parts.push("--to-ref".into());
    parts.push("HEAD".into());

    if let Some(path) = config_path {
        parts.push("-c".into());
        parts.push(shell_quote(&path.to_string_lossy()));
    }

    parts.join(" ")
}

/// Single-quote a value for inclusion in a `sh -c "<command>"` string.
/// Embedded single quotes become `'\''`, the standard POSIX trick. Used
/// only for user-supplied strings (`hook_stage`, `config_path`); the
/// flag literals are static and don't need quoting.
fn shell_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
}

fn outcome_to_verdict(
    check_name: &str,
    outcome: &ShellOutcome,
    version_warning: Option<&str>,
) -> Verdict {
    match outcome.status_code {
        // Pass run, but surface any version warning as a non-blocking Warn
        // so operators see they're outside the tested range.
        Some(0) => match version_warning {
            None => Verdict::Pass,
            Some(warning) => Verdict::Warn {
                findings: vec![finding(check_name, warning, Severity::Warn)],
                message: Some(warning.to_string()),
            },
        },
        Some(1) => {
            let mut findings = parse_failed_hooks(check_name, &outcome.stdout);
            if findings.is_empty() {
                let trimmed = outcome.stderr.trim();
                let detail = if trimmed.is_empty() {
                    format!("pre-commit reported failures for check `{check_name}`")
                } else {
                    format!("pre-commit reported failures for check `{check_name}`: {trimmed}")
                };
                findings.push(finding(check_name, &detail, Severity::Error));
            }
            let message = format!(
                "pre-commit failed ({} hook{})",
                findings.len(),
                if findings.len() == 1 { "" } else { "s" }
            );
            Verdict::Fail { findings, message }
        }
        Some(other) => {
            let trimmed = outcome.stderr.trim();
            let detail = if trimmed.is_empty() {
                format!(
                    "pre-commit `{check_name}` exited with unexpected status {other}; \
                     this usually means a tooling error inside pre-commit itself"
                )
            } else {
                format!(
                    "pre-commit `{check_name}` exited with unexpected status \
                     {other}: {trimmed}"
                )
            };
            single_fail(check_name, detail)
        }
        None => single_fail(
            check_name,
            format!("pre-commit `{check_name}` was terminated before producing an exit code"),
        ),
    }
}

/// One-line `Finding` builder. Centralises the `pre_commit:<name>` rule
/// slug so a single edit can re-shape every emitted finding.
fn finding(check_name: &str, message: &str, severity: Severity) -> Finding {
    Finding {
        rule: format!("pre_commit:{check_name}"),
        message: message.to_string(),
        file: None,
        line: None,
        severity,
    }
}

/// `Verdict::Fail` with a single error-level finding whose message is also
/// the verdict's top-level `message`. Used for the unexpected-exit and
/// terminated-without-exit branches where there's nothing to parse out
/// of stdout.
fn single_fail(check_name: &str, detail: String) -> Verdict {
    Verdict::Fail {
        findings: vec![finding(check_name, &detail, Severity::Error)],
        message: detail,
    }
}

/// Parse pre-commit's per-hook summary lines (`"<hook>....Failed"`) into
/// findings. Format is stable from 3.0 through 4.x. Anchors on the
/// trailing `"Failed"` token and strips the padding dots back to recover
/// the hook description.
fn parse_failed_hooks(check_name: &str, stdout: &str) -> Vec<Finding> {
    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            let head = line.strip_suffix("Failed")?;
            let head = head.trim_end_matches(|c: char| c == '.' || c.is_whitespace());
            (!head.is_empty()).then(|| {
                finding(
                    check_name,
                    &format!("hook `{head}` failed"),
                    Severity::Error,
                )
            })
        })
        .collect()
}

/// Lazily run `pre-commit --version`, parse the major.minor, and return a
/// warning when it falls outside the supported range. `None` means the
/// version is fine *or* we couldn't probe pre-commit (some wrappers
/// don't honour `--version`); both cases swallow the warning.
fn sniff_version_warning(cwd: &std::path::Path) -> Option<String> {
    let output = Command::new("pre-commit")
        .arg("--version")
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let (major, minor) = parse_version(&raw)?;
    let actual = (major, minor);
    if actual < MIN_SUPPORTED_VERSION {
        let (rmaj, rmin) = MIN_SUPPORTED_VERSION;
        return Some(format!(
            "pre-commit {major}.{minor} is older than the minimum tested version \
             {rmaj}.{rmin}; output parsing may be incomplete"
        ));
    }
    if actual > MAX_TESTED_VERSION {
        let (rmaj, rmin) = MAX_TESTED_VERSION;
        return Some(format!(
            "pre-commit {major}.{minor} is newer than the latest tested version \
             {rmaj}.{rmin}; if hook output looks wrong, file an issue at \
             https://github.com/klasp-dev/klasp/issues"
        ));
    }
    None
}

/// Parse `"pre-commit 3.8.0\n"` → `Some((3, 8))`. Tolerant: takes the
/// last whitespace-separated token from the first non-empty line and
/// parses its first two dot-separated segments. Returns `None` if no
/// version-shaped token is found.
fn parse_version(raw: &str) -> Option<(u32, u32)> {
    let line = raw.lines().find(|l| !l.trim().is_empty())?;
    let token = line.split_whitespace().last()?;
    let mut parts = token.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use klasp_core::{CheckConfig, CheckSourceConfig};

    use super::*;

    fn pre_commit_check() -> CheckConfig {
        CheckConfig {
            name: "lint".into(),
            triggers: vec![],
            source: CheckSourceConfig::PreCommit {
                hook_stage: None,
                config_path: None,
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

    fn outcome(code: Option<i32>, stdout: &str, stderr: &str) -> ShellOutcome {
        ShellOutcome {
            status_code: code,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn supports_config_only_for_pre_commit() {
        let source = PreCommitSource::new();
        assert!(source.supports_config(&pre_commit_check()));
        assert!(!source.supports_config(&shell_check()));
    }

    #[test]
    fn build_command_uses_defaults_when_unset() {
        let cmd = build_command("pre-commit", None, "deadbeef");
        assert_eq!(
            cmd,
            "pre-commit run --hook-stage 'pre-commit' --from-ref ${KLASP_BASE_REF} --to-ref HEAD"
        );
    }

    #[test]
    fn build_command_passes_config_path() {
        let cmd = build_command("pre-push", Some(Path::new("tools/p.yaml")), "x");
        assert_eq!(
            cmd,
            "pre-commit run --hook-stage 'pre-push' --from-ref ${KLASP_BASE_REF} --to-ref HEAD \
             -c 'tools/p.yaml'"
        );
    }

    #[test]
    fn shell_quote_handles_embedded_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn outcome_zero_with_version_warning_is_warn() {
        let v = outcome_to_verdict("lint", &outcome(Some(0), "", ""), Some("too new"));
        match v {
            Verdict::Warn { findings, message } => {
                assert_eq!(findings.len(), 1);
                assert_eq!(findings[0].severity, Severity::Warn);
                assert_eq!(message.as_deref(), Some("too new"));
            }
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn outcome_one_with_failed_hooks_yields_findings() {
        let stdout = concat!(
            "trim trailing whitespace.................................................Passed\n",
            "ruff.....................................................................Failed\n",
            "mypy.....................................................................Failed\n",
        );
        let v = outcome_to_verdict("lint", &outcome(Some(1), stdout, ""), None);
        match v {
            Verdict::Fail { findings, message } => {
                assert_eq!(findings.len(), 2);
                assert!(findings[0].message.contains("ruff"));
                assert!(findings[1].message.contains("mypy"));
                assert!(message.contains("2 hooks"));
                assert_eq!(findings[0].rule, "pre_commit:lint");
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn outcome_one_without_parseable_stdout_falls_back_to_generic_finding() {
        let v = outcome_to_verdict("lint", &outcome(Some(1), "", "boom"), None);
        match v {
            Verdict::Fail { findings, message } => {
                assert_eq!(findings.len(), 1);
                assert!(findings[0].message.contains("boom"));
                assert!(message.contains("1 hook"));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn outcome_unexpected_exit_code_carries_status_in_message() {
        let v = outcome_to_verdict("lint", &outcome(Some(130), "", "Interrupted"), None);
        match v {
            Verdict::Fail { message, .. } => {
                assert!(message.contains("130"), "message = {message}");
                assert!(message.contains("Interrupted"));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn outcome_no_exit_code_is_fail() {
        let v = outcome_to_verdict("lint", &outcome(None, "", ""), None);
        assert!(matches!(v, Verdict::Fail { .. }));
    }

    #[test]
    fn parse_version_extracts_major_minor() {
        assert_eq!(parse_version("pre-commit 3.8.0"), Some((3, 8)));
        assert_eq!(parse_version("pre-commit 4.0.1\n"), Some((4, 0)));
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("not a version"), None);
    }

    #[test]
    fn parse_failed_hooks_handles_passed_and_skipped() {
        let stdout = concat!(
            "ruff.....................................................................Passed\n",
            "mypy.....................................................................Skipped\n",
            "black....................................................................Failed\n",
        );
        let findings = parse_failed_hooks("lint", stdout);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("black"));
    }

    #[test]
    fn pre_commit_config_round_trips_path_buf() {
        // `config_path` is a `PathBuf` rather than a `String`; this guards
        // against an accidental flip back to `String` (which would lose
        // the typed-path API on the surface).
        let c = CheckConfig {
            name: "lint".into(),
            triggers: vec![],
            source: CheckSourceConfig::PreCommit {
                hook_stage: Some("pre-push".into()),
                config_path: Some(PathBuf::from("tools/p.yaml")),
            },
            timeout_secs: None,
        };
        match c.source {
            CheckSourceConfig::PreCommit { config_path, .. } => {
                assert_eq!(config_path.as_deref(), Some(Path::new("tools/p.yaml")));
            }
            _ => unreachable!(),
        }
    }
}
