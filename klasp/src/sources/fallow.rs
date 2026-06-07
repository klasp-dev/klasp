//! `Fallow` — second named recipe source (v0.2 W5).
//!
//! Translates `[checks.source] type = "fallow"` into a `fallow audit
//! --format json` invocation, then maps the top-level `verdict` plus
//! per-finding entries (complexity, dead-code, duplication) to a
//! [`klasp_core::Verdict`]. Pre-2.x and post-2.x versions surface a
//! non-blocking `Severity::Warn` note. JSON parsing helpers live in
//! [`json`]; version sniffing in [`version`].

use std::path::Path;
use std::time::Duration;

use klasp_core::{
    CheckConfig, CheckResult, CheckSource, CheckSourceConfig, CheckSourceError, Finding, GitEvent,
    RepoState, Severity, Verdict,
};
use serde_json::Value;

use super::recipe_util::{
    self, fail_with_optional_warning as util_fail_with_optional_warning, shell_quote,
};
use super::shell::{run_with_timeout, ShellOutcome, DEFAULT_TIMEOUT_SECS};

/// Stable identifier this source advertises through `CheckSource::source_id`.
const SOURCE_ID: &str = "fallow";

/// Tool slug prepended to every `Finding` rule this recipe emits. Matches
/// [`SOURCE_ID`]; named separately so the rule-slug intent is explicit at
/// the builder call sites.
const RULE_PREFIX: &str = "fallow";

/// Lowest fallow release whose audit JSON schema matches the parser in
/// [`json::collect_findings`]. klasp 0.2 was developed against fallow
/// 2.62.0; 1.x is out of scope. There is deliberately no upper bound —
/// fallow's audit schema has been stable through 2.x, and emitting
/// "newer than tested" on every release would be toil-without-signal
/// (W4 hit the same trade-off).
const MIN_SUPPORTED_VERSION: (u32, u32) = (2, 0);

/// Cap on findings emitted into a verdict. Bounds block-message size on
/// huge audits; fallow's own rendered output truncates similarly.
pub(super) const MAX_FINDINGS: usize = 50;

mod json;
use json::{collect_findings, summarise};

/// `CheckSource` for `type = "fallow"` config entries. Stateless;
/// safe to clone or share. Constructed once via
/// [`super::SourceRegistry::default_v1`].
#[derive(Default)]
pub struct FallowSource {
    _private: (),
}

impl FallowSource {
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

impl CheckSource for FallowSource {
    fn source_id(&self) -> &str {
        SOURCE_ID
    }

    fn supports_config(&self, config: &CheckConfig) -> bool {
        matches!(config.source, CheckSourceConfig::Fallow { .. })
    }

    fn run(
        &self,
        config: &CheckConfig,
        state: &RepoState,
    ) -> Result<CheckResult, CheckSourceError> {
        let (config_path, base) = match &config.source {
            CheckSourceConfig::Fallow { config_path, base } => (config_path.clone(), base.clone()),
            other => {
                return Err(CheckSourceError::Other(
                    format!("FallowSource cannot run {other:?}").into(),
                ));
            }
        };

        let command = build_command(state.git_event, base.as_deref(), config_path.as_deref());
        let timeout = Duration::from_secs(config.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
        let outcome = run_with_timeout(&command, &state.root, &state.base_ref, timeout)?;

        let version_warning = recipe_util::sniff_version_warning(
            RULE_PREFIX,
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

/// Render the `fallow audit …` command klasp will hand to `sh -c`.
///
/// The command shape depends on the firing trigger:
///
/// - `GitEvent::Commit` (PreToolUse intercepting `git commit`): the staged
///   index has the diff that's about to land but `HEAD` doesn't yet include
///   it. Passing `--base` would scope fallow to committed history and miss
///   the staged changes entirely. Instead we emit `fallow audit` with no
///   `--base` flag — fallow's default is to run against the working tree,
///   which includes staged content.
///
/// - `GitEvent::Push` (PreToolUse intercepting `git push`): the new commits
///   are already in `HEAD`, so scoping to `--base ${KLASP_BASE_REF}` is
///   correct. This is the prior behaviour, preserved.
///
/// `${KLASP_BASE_REF}` is preferred over the resolved-at-build-time
/// `state.base_ref` because the env var is the documented contract for
/// every klasp shell-flavoured source. The shell source's `run_with_timeout`
/// exports the var into the child env identically.
/// `--quiet` suppresses fallow's progress output, and `--format json`
/// gives us the structured verdict the parser below consumes.
fn build_command(event: GitEvent, base: Option<&str>, config_path: Option<&Path>) -> String {
    let mut parts: Vec<String> = vec!["fallow".into(), "audit".into()];
    parts.push("--format".into());
    parts.push("json".into());
    parts.push("--quiet".into());

    match event {
        GitEvent::Commit => {
            // Staged-index form: no --base flag; fallow runs against the
            // working tree (which includes staged content). Issue #64.
        }
        GitEvent::Push => {
            // Push form: new commits are in HEAD; scope to the ref-base.
            parts.push("--base".into());
            parts.push(match base {
                Some(b) => shell_quote(b),
                None => "${KLASP_BASE_REF}".into(),
            });
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
    // Parse fallow's JSON first; an unparseable payload means the recipe
    // can't trust the exit code on its own, so degrade to a generic
    // failure rather than guessing at the verdict.
    let parsed: Option<Value> = serde_json::from_str(outcome.stdout.trim()).ok();
    let Some(json) = parsed else {
        return fail_with_optional_warning(
            check_name,
            unparseable_detail(check_name, outcome),
            version_warning,
        );
    };

    let verdict_str = json.get("verdict").and_then(Value::as_str).unwrap_or("");
    let mut findings = collect_findings(check_name, &json);

    match verdict_str {
        "pass" => match version_warning {
            None => Verdict::Pass,
            Some(warning) => Verdict::Warn {
                findings: vec![note(check_name, warning, Severity::Warn)],
                message: Some(warning.to_string()),
            },
        },
        "warn" => {
            if let Some(warning) = version_warning {
                findings.insert(0, note(check_name, warning, Severity::Warn));
            }
            // Demote any Error severities to Warn so a recipe-level
            // verdict of `"warn"` never carries a blocking finding into
            // the merge step.
            for f in &mut findings {
                if matches!(f.severity, Severity::Error) {
                    f.severity = Severity::Warn;
                }
            }
            Verdict::Warn {
                findings,
                message: summarise(&json, "warn"),
            }
        }
        "fail" => {
            if let Some(warning) = version_warning {
                findings.insert(0, note(check_name, warning, Severity::Warn));
            }
            if findings
                .iter()
                .all(|f| !matches!(f.severity, Severity::Error))
            {
                // Verdict says `fail` but no per-finding row carried an
                // error severity — fall back to a generic block message
                // so the agent has something to act on.
                findings.push(note(
                    check_name,
                    &format!("fallow `{check_name}` reported a fail verdict"),
                    Severity::Error,
                ));
            }
            Verdict::Fail {
                findings,
                message: summarise(&json, "fail")
                    .unwrap_or_else(|| format!("fallow `{check_name}` reported a fail verdict")),
            }
        }
        other => {
            let detail = format!(
                "fallow `{check_name}` returned an unexpected verdict `{other}`; \
                 this usually means the audit JSON schema has drifted"
            );
            fail_with_optional_warning(check_name, detail, version_warning)
        }
    }
}

fn unparseable_detail(check_name: &str, outcome: &ShellOutcome) -> String {
    let trimmed = outcome.stderr.trim();
    let stderr_hint = if trimmed.is_empty() {
        String::new()
    } else {
        format!(": {trimmed}")
    };
    let exit_hint = match outcome.status_code {
        Some(c) => format!(" (exit {c})"),
        None => " (terminated before producing an exit code)".to_string(),
    };
    format!("fallow `{check_name}` produced unparseable JSON{exit_hint}{stderr_hint}")
}

/// Build a `Verdict::Fail` whose findings carry the supplied detail plus
/// an optional version-warning prepended at `Severity::Warn`. Thin wrapper
/// over [`recipe_util::fail_with_optional_warning`] pinning the `fallow:`
/// rule prefix.
fn fail_with_optional_warning(
    check_name: &str,
    detail: String,
    version_warning: Option<&str>,
) -> Verdict {
    util_fail_with_optional_warning(RULE_PREFIX, check_name, detail, version_warning)
}

/// `Finding` builder. `rule_suffix = ""` produces a top-level rule
/// (`fallow:<check>`), suitable for recipe-level notices. A non-empty
/// suffix nests the rule (`fallow:<check>:<suffix>`) for per-finding
/// rows so a future filter can target one category at a time. Thin
/// wrapper over [`recipe_util::finding`] pinning the `fallow:` prefix;
/// re-exported to [`json`] via `use super::finding`.
fn finding(
    check_name: &str,
    rule_suffix: &str,
    message: &str,
    file: Option<String>,
    line: Option<u32>,
    severity: Severity,
) -> Finding {
    recipe_util::finding(
        RULE_PREFIX,
        check_name,
        rule_suffix,
        message,
        file,
        line,
        severity,
    )
}

fn note(check_name: &str, message: &str, severity: Severity) -> Finding {
    recipe_util::note(RULE_PREFIX, check_name, message, severity)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use klasp_core::{CheckConfig, CheckSourceConfig};

    use super::*;

    fn fallow_check() -> CheckConfig {
        CheckConfig {
            name: "audit".into(),
            triggers: vec![],
            source: CheckSourceConfig::Fallow {
                config_path: None,
                base: None,
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
    fn supports_config_only_for_fallow() {
        let source = FallowSource::new();
        assert!(source.supports_config(&fallow_check()));
        assert!(!source.supports_config(&shell_check()));
    }

    #[test]
    fn build_command_commit_trigger_omits_base() {
        // Commit trigger must NOT include --base; that scopes to committed
        // history and misses the staged index entirely (issue #64).
        let cmd = build_command(GitEvent::Commit, None, None);
        assert_eq!(cmd, "fallow audit --format json --quiet");
        assert!(
            !cmd.contains("--base"),
            "commit trigger must not contain --base: {cmd}"
        );
    }

    #[test]
    fn build_command_push_trigger_uses_klasp_base_ref_by_default() {
        let cmd = build_command(GitEvent::Push, None, None);
        assert_eq!(
            cmd,
            "fallow audit --format json --quiet --base ${KLASP_BASE_REF}"
        );
    }

    #[test]
    fn build_command_push_trigger_uses_explicit_base_when_set() {
        let cmd = build_command(GitEvent::Push, Some("origin/main"), None);
        assert!(cmd.contains("--base 'origin/main'"));
    }

    #[test]
    fn build_command_push_trigger_passes_config_path() {
        let cmd = build_command(
            GitEvent::Push,
            None,
            Some(Path::new("tools/.fallowrc.json")),
        );
        assert!(cmd.ends_with("-c 'tools/.fallowrc.json'"));
    }

    #[test]
    fn build_command_commit_trigger_passes_config_path() {
        let cmd = build_command(
            GitEvent::Commit,
            None,
            Some(Path::new("tools/.fallowrc.json")),
        );
        assert_eq!(
            cmd,
            "fallow audit --format json --quiet -c 'tools/.fallowrc.json'"
        );
    }

    #[test]
    fn shell_quote_handles_embedded_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn pass_verdict_with_version_warning_is_warn() {
        let json = r#"{"verdict":"pass","summary":{}}"#;
        let v = outcome_to_verdict("audit", &outcome(Some(0), json, ""), Some("too new"));
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
    fn fail_verdict_demotes_to_generic_finding_when_no_severity_rows() {
        // A `verdict = "fail"` payload with no per-row finding is unusual
        // but legal — the recipe must still surface a blocking finding
        // so the agent has something actionable.
        let json = r#"{"verdict":"fail","summary":{}}"#;
        let v = outcome_to_verdict("audit", &outcome(Some(1), json, ""), None);
        match v {
            Verdict::Fail { findings, .. } => {
                assert!(!findings.is_empty());
                assert!(findings
                    .iter()
                    .any(|f| matches!(f.severity, Severity::Error)));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn unparseable_stdout_is_fail_with_generic_finding() {
        let v = outcome_to_verdict("audit", &outcome(Some(1), "not json", "boom"), None);
        match v {
            Verdict::Fail { findings, message } => {
                assert_eq!(findings.len(), 1);
                assert!(message.contains("unparseable"));
                assert!(findings[0].message.contains("boom"));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn fail_with_version_warning_prepends_warn_finding() {
        let json = r#"{
            "verdict":"fail",
            "summary":{},
            "complexity":{"findings":[
                {"path":"src/x.ts","name":"f","line":1,"severity":"high"}
            ]},
            "dead_code":{},"duplication":{}
        }"#;
        let v = outcome_to_verdict("audit", &outcome(Some(1), json, ""), Some("old fallow"));
        match v {
            Verdict::Fail { findings, .. } => {
                assert!(findings.len() >= 2);
                assert_eq!(findings[0].severity, Severity::Warn);
                assert!(findings[0].message.contains("old fallow"));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn unknown_verdict_falls_through_to_fail() {
        let json = r#"{"verdict":"unknown","summary":{}}"#;
        let v = outcome_to_verdict("audit", &outcome(Some(1), json, ""), None);
        match v {
            Verdict::Fail { message, .. } => {
                assert!(message.contains("unexpected verdict"));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn parse_version_extracts_major_minor() {
        // Version parsing now lives in `recipe_util`; this guards the
        // fallow-shaped banners against a future parser regression.
        assert_eq!(recipe_util::parse_version("fallow 2.62.0"), Some((2, 62)));
        assert_eq!(recipe_util::parse_version("fallow 3.0.1\n"), Some((3, 0)));
        assert_eq!(recipe_util::parse_version(""), None);
        assert_eq!(recipe_util::parse_version("not a version"), None);
    }
}
