//! `Fallow` — second named recipe source (v0.2 W5).
//!
//! Translates `[checks.source] type = "fallow"` into a `fallow audit
//! --format json` invocation, then maps the top-level `verdict` plus
//! per-finding entries (complexity, dead-code, duplication) to a
//! [`klasp_core::Verdict`]. Pre-2.x and post-2.x versions surface a
//! non-blocking `Severity::Warn` note. JSON parsing helpers live in
//! [`json`]; version sniffing in [`version`].

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use klasp_core::{
    CheckConfig, CheckResult, CheckSource, CheckSourceConfig, CheckSourceError, Finding, RepoState,
    Severity, Verdict,
};
use serde_json::Value;

use super::shell::{run_with_timeout, ShellOutcome, DEFAULT_TIMEOUT_SECS};

/// Stable identifier this source advertises through `CheckSource::source_id`.
const SOURCE_ID: &str = "fallow";

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

        let command = build_command(base.as_deref(), config_path.as_deref());
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

/// Render the `fallow audit …` command klasp will hand to `sh -c`.
///
/// `${KLASP_BASE_REF}` is the documented default for `--base` so users
/// who configure `[[checks]]` without a `base` field automatically pick
/// up the gate-resolved merge-base. Users who set `base` explicitly
/// (e.g. for a long-lived release branch) override that default.
/// `--quiet` suppresses fallow's progress output, and `--format json`
/// gives us the structured verdict the parser below consumes.
fn build_command(base: Option<&str>, config_path: Option<&Path>) -> String {
    let mut parts: Vec<String> = vec!["fallow".into(), "audit".into()];
    parts.push("--format".into());
    parts.push("json".into());
    parts.push("--quiet".into());
    parts.push("--base".into());
    parts.push(match base {
        Some(b) => shell_quote(b),
        None => "${KLASP_BASE_REF}".into(),
    });
    if let Some(path) = config_path {
        parts.push("-c".into());
        parts.push(shell_quote(&path.to_string_lossy()));
    }
    parts.join(" ")
}

/// Single-quote a value for inclusion in a `sh -c "<command>"` string.
/// Embedded single quotes become `'\''`, the standard POSIX trick. Used
/// only for user-supplied strings (`base`, `config_path`); the flag
/// literals are static and don't need quoting.
fn shell_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
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
/// an optional version-warning prepended at `Severity::Warn`.
fn fail_with_optional_warning(
    check_name: &str,
    detail: String,
    version_warning: Option<&str>,
) -> Verdict {
    let mut findings = vec![note(check_name, &detail, Severity::Error)];
    if let Some(warning) = version_warning {
        findings.insert(0, note(check_name, warning, Severity::Warn));
    }
    Verdict::Fail {
        findings,
        message: detail,
    }
}

/// `Finding` builder. `rule_suffix = ""` produces a top-level rule
/// (`fallow:<check>`), suitable for recipe-level notices. A non-empty
/// suffix nests the rule (`fallow:<check>:<suffix>`) for per-finding
/// rows so a future filter can target one category at a time.
fn finding(
    check_name: &str,
    rule_suffix: &str,
    message: &str,
    file: Option<String>,
    line: Option<u32>,
    severity: Severity,
) -> Finding {
    let rule = if rule_suffix.is_empty() {
        format!("fallow:{check_name}")
    } else {
        format!("fallow:{check_name}:{rule_suffix}")
    };
    Finding {
        rule,
        message: message.to_string(),
        file,
        line,
        severity,
    }
}

fn note(check_name: &str, message: &str, severity: Severity) -> Finding {
    finding(check_name, "", message, None, None, severity)
}

/// Lazily run `fallow --version`, parse the major.minor, and return a
/// warning when older than [`MIN_SUPPORTED_VERSION`]. `None` means OK
/// *or* we couldn't probe fallow; both cases swallow. Cached for the
/// lifetime of the process — a gate invocation resolves `fallow` from
/// the same `$PATH` entry for every check, so re-running the probe
/// would multiply subprocess overhead by N for no signal.
fn sniff_version_warning(cwd: &Path) -> Option<String> {
    static CACHED: OnceLock<Option<String>> = OnceLock::new();
    CACHED
        .get_or_init(|| sniff_version_warning_uncached(cwd))
        .clone()
}

fn sniff_version_warning_uncached(cwd: &Path) -> Option<String> {
    let output = Command::new("fallow")
        .arg("--version")
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).to_string();
    let (major, minor) = parse_version(&raw)?;
    if (major, minor) < MIN_SUPPORTED_VERSION {
        let (rmaj, rmin) = MIN_SUPPORTED_VERSION;
        return Some(format!(
            "fallow {major}.{minor} is older than the minimum tested version \
             {rmaj}.{rmin}; output parsing may be incomplete"
        ));
    }
    None
}

/// Parse `"fallow 2.62.0\n"` → `Some((2, 62))`. Tolerant: takes the
/// last whitespace-separated token from the first non-empty line and
/// parses its first two dot-separated segments.
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
    fn build_command_uses_klasp_base_ref_by_default() {
        let cmd = build_command(None, None);
        assert_eq!(
            cmd,
            "fallow audit --format json --quiet --base ${KLASP_BASE_REF}"
        );
    }

    #[test]
    fn build_command_uses_explicit_base_when_set() {
        let cmd = build_command(Some("origin/main"), None);
        assert!(cmd.contains("--base 'origin/main'"));
    }

    #[test]
    fn build_command_passes_config_path() {
        let cmd = build_command(None, Some(Path::new("tools/.fallowrc.json")));
        assert!(cmd.ends_with("-c 'tools/.fallowrc.json'"));
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
        assert_eq!(parse_version("fallow 2.62.0"), Some((2, 62)));
        assert_eq!(parse_version("fallow 3.0.1\n"), Some((3, 0)));
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("not a version"), None);
    }
}
