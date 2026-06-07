//! `PreCommit` — first named recipe source (v0.2 W4).
//!
//! Translates `[checks.source] type = "pre_commit"` into a `pre-commit run`
//! invocation; maps the stage-aware exit code to a [`klasp_core::Verdict`].
//! Exit `0` → [`Verdict::Pass`], `1` → [`Verdict::Fail`] with per-hook
//! findings parsed from stdout (`"<hook>....Failed"` lines, stable since
//! pre-commit 3.0), other codes → [`Verdict::Fail`] with a generic
//! "unexpected exit" finding. Versions older than
//! [`MIN_SUPPORTED_VERSION`] surface a `Verdict::Warn`. `verdict_path` is
//! deferred per [docs/design.md §14].

use std::time::Duration;

use klasp_core::{
    CheckConfig, CheckResult, CheckSource, CheckSourceConfig, CheckSourceError, Finding, GitEvent,
    RepoState, Severity, Verdict,
};

use super::recipe_util::{
    self, fail_with_optional_warning as util_fail_with_optional_warning, shell_quote,
};
use super::shell::{run_with_timeout, ShellOutcome, DEFAULT_TIMEOUT_SECS};

/// Tool slug prepended to every `Finding` rule this recipe emits.
const RULE_PREFIX: &str = "pre_commit";

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
/// the per-hook summary line format; 2.x is out of scope. There is
/// deliberately no upper bound — pre-commit's per-hook summary format
/// has been stable since 3.0, and emitting a "newer than tested"
/// notice on every release would just add toil. If hook output ever
/// stops parsing on a future version, the recipe falls back to the
/// generic stderr message and the user files an issue.
const MIN_SUPPORTED_VERSION: (u32, u32) = (3, 0);

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

        let command = build_command(state.git_event, &hook_stage, config_path.as_deref());
        let timeout = Duration::from_secs(config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
        let outcome = run_with_timeout(&command, &state.root, &state.base_ref, timeout)?;

        let version_warning = recipe_util::sniff_version_warning(
            "pre-commit",
            MIN_SUPPORTED_VERSION,
            &state.root,
            false,
        );
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
/// The command shape depends on the firing trigger:
///
/// - `GitEvent::Commit` (PreToolUse intercepting `git commit`): the staged
///   index has the diff that's about to land but `HEAD` doesn't yet include
///   it. Passing `--from-ref`/`--to-ref` would scope pre-commit to already-
///   committed history and miss the staged changes entirely. Instead we emit
///   `pre-commit run --hook-stage commit` with no ref arguments — that is
///   exactly pre-commit's own default when invoked from a `.git/hooks/pre-commit`
///   shim, which runs on the staging area.
///
/// - `GitEvent::Push` (PreToolUse intercepting `git push`): the new commits
///   are already in `HEAD`, so scoping to `--from-ref ${KLASP_BASE_REF}
///   --to-ref HEAD` is correct. This is the prior behaviour, preserved.
///
/// `${KLASP_BASE_REF}` is preferred over the resolved-at-build-time
/// `state.base_ref` because the env var is the documented contract for
/// every klasp shell-flavoured source — keeping the recipes consistent
/// with the v0.1 user-authored `command = "…"` form means a copy-paste
/// from one to the other is mechanical. The shell source's
/// `run_with_timeout` exports the var into the child env identically.
fn build_command(
    event: GitEvent,
    hook_stage: &str,
    config_path: Option<&std::path::Path>,
) -> String {
    let mut parts: Vec<String> = vec!["pre-commit".into(), "run".into()];
    parts.push("--hook-stage".into());

    match event {
        GitEvent::Commit => {
            // Staged-index form: let pre-commit's own staging-area detection
            // handle scope. No --from-ref/--to-ref — those scope to committed
            // history, not the index.
            parts.push(shell_quote(hook_stage));
        }
        GitEvent::Push => {
            // Push form: new commits are already in HEAD; scope to the ref range.
            parts.push(shell_quote(hook_stage));
            parts.push("--from-ref".into());
            parts.push("${KLASP_BASE_REF}".into());
            parts.push("--to-ref".into());
            parts.push("HEAD".into());
        }
    }

    if let Some(path) = config_path {
        parts.push("-c".into());
        parts.push(shell_quote(&path.to_string_lossy()));
    }

    parts.join(" ")
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
            // Surface the version warning alongside the hook failures: an
            // out-of-range pre-commit version is exactly the case where
            // the parser may have produced incomplete findings, so the
            // operator needs to see the warning *and* the failure detail.
            if let Some(warning) = version_warning {
                findings.insert(0, finding(check_name, warning, Severity::Warn));
            }
            let hook_failures = findings
                .iter()
                .filter(|f| matches!(f.severity, Severity::Error))
                .count();
            let message = format!(
                "pre-commit failed ({} hook{})",
                hook_failures,
                if hook_failures == 1 { "" } else { "s" }
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
            fail_with_optional_warning(check_name, detail, version_warning)
        }
        None => fail_with_optional_warning(
            check_name,
            format!("pre-commit `{check_name}` was terminated before producing an exit code"),
            version_warning,
        ),
    }
}

/// Build a `Verdict::Fail` whose findings carry the unexpected-exit
/// detail plus an optional version-warning prepended at `Severity::Warn`.
/// Thin wrapper over [`recipe_util::fail_with_optional_warning`] that
/// pins the `pre_commit:` rule prefix.
fn fail_with_optional_warning(
    check_name: &str,
    detail: String,
    version_warning: Option<&str>,
) -> Verdict {
    util_fail_with_optional_warning(RULE_PREFIX, check_name, detail, version_warning)
}

/// One-line `Finding` builder. Centralises the `pre_commit:<name>` rule
/// slug so a single edit can re-shape every emitted finding. Thin wrapper
/// over [`recipe_util::note`] — pre-commit findings never carry a file /
/// line, so the top-level `note` form is exactly right.
fn finding(check_name: &str, message: &str, severity: Severity) -> Finding {
    recipe_util::note(RULE_PREFIX, check_name, message, severity)
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
    fn build_command_commit_trigger_omits_ref_range() {
        // Commit trigger must NOT include --from-ref/--to-ref; those scope to
        // committed history and miss the staged index entirely (issue #64).
        let cmd = build_command(GitEvent::Commit, "pre-commit", None);
        assert_eq!(cmd, "pre-commit run --hook-stage 'pre-commit'");
        assert!(
            !cmd.contains("--from-ref"),
            "commit trigger must not contain --from-ref: {cmd}"
        );
        assert!(
            !cmd.contains("--to-ref"),
            "commit trigger must not contain --to-ref: {cmd}"
        );
    }

    #[test]
    fn build_command_push_trigger_includes_ref_range() {
        // Push trigger must include --from-ref/--to-ref so pre-commit scopes
        // to the commits being pushed (current behaviour, preserved).
        let cmd = build_command(GitEvent::Push, "pre-commit", None);
        assert_eq!(
            cmd,
            "pre-commit run --hook-stage 'pre-commit' --from-ref ${KLASP_BASE_REF} --to-ref HEAD"
        );
    }

    #[test]
    fn build_command_push_trigger_passes_config_path() {
        let cmd = build_command(GitEvent::Push, "pre-push", Some(Path::new("tools/p.yaml")));
        assert_eq!(
            cmd,
            "pre-commit run --hook-stage 'pre-push' --from-ref ${KLASP_BASE_REF} --to-ref HEAD \
             -c 'tools/p.yaml'"
        );
    }

    #[test]
    fn build_command_commit_trigger_passes_config_path() {
        let cmd = build_command(
            GitEvent::Commit,
            "pre-commit",
            Some(Path::new("tools/p.yaml")),
        );
        assert_eq!(
            cmd,
            "pre-commit run --hook-stage 'pre-commit' -c 'tools/p.yaml'"
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
        // Version parsing now lives in `recipe_util`; this guards the
        // pre-commit-shaped banners against a future parser regression.
        assert_eq!(recipe_util::parse_version("pre-commit 3.8.0"), Some((3, 8)));
        assert_eq!(
            recipe_util::parse_version("pre-commit 4.0.1\n"),
            Some((4, 0))
        );
        assert_eq!(recipe_util::parse_version(""), None);
        assert_eq!(recipe_util::parse_version("not a version"), None);
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
