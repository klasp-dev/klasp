//! `Fallow` — second named recipe source (v0.2 W5).
//!
//! Translates `[checks.source] type = "fallow"` into a `fallow audit`
//! invocation; parses fallow's audit JSON output and maps the top-level
//! `verdict` plus per-finding entries to a [`klasp_core::Verdict`].
//! `verdict = "pass"` → [`Verdict::Pass`], `"warn"` → [`Verdict::Warn`],
//! `"fail"` → [`Verdict::Fail`] with structured findings drawn from
//! `complexity.findings[]`, `dead_code.unused_*[]`, and
//! `duplication.clone_groups[]`. Versions outside
//! [`MIN_SUPPORTED_VERSION`] / [`MAX_SUPPORTED_VERSION`] surface a
//! non-blocking `Severity::Warn` note alongside the verdict.

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

/// Lowest fallow release whose `fallow audit --format json` schema matches
/// the parser below. Schema version 3 (the `schema_version` field on the
/// audit document) shipped in fallow 2.x; klasp 0.2 was developed
/// against fallow 2.62.0. 1.x is out of scope.
const MIN_SUPPORTED_VERSION: (u32, u32) = (2, 0);

/// Upper inclusive bound. fallow's audit JSON has been stable through
/// the 2.x line; bumping this to 3.x is a deliberate decision once the
/// 3.x format has been audited. Until then the recipe runs but emits a
/// non-blocking warning above 2.x so operators see they're outside the
/// tested range.
const MAX_SUPPORTED_VERSION: (u32, u32) = (2, u32::MAX);

/// Cap the number of findings emitted into the verdict to keep block
/// messages tractable on huge audits. fallow already truncates its own
/// rendered output; mirroring that bound here means a wall of dead-code
/// findings from a fresh repo scan doesn't drown the agent's stderr.
const MAX_FINDINGS: usize = 50;

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
    let summary = summarise(&json, &findings);

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
            let message = if summary.is_empty() {
                None
            } else {
                Some(summary)
            };
            Verdict::Warn { findings, message }
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
                message: summary,
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

/// Walk fallow's audit JSON and emit a structured finding per actionable
/// row. Capped at [`MAX_FINDINGS`] to keep block messages bounded; the
/// caller's summary still carries the un-truncated counts.
///
/// Categories drained in order: complexity → dead-code (across every
/// `unused_*` / structural-issue array) → duplication clones. The dead-
/// code key list is the union of categories fallow reports as
/// per-finding rows; categories not present at the JSON level are
/// silently skipped.
const DEAD_CODE_KEYS: &[&str] = &[
    "unused_files",
    "unused_exports",
    "unused_types",
    "unused_dependencies",
    "unused_dev_dependencies",
    "unresolved_imports",
    "unlisted_dependencies",
    "duplicate_exports",
    "circular_dependencies",
    "boundary_violations",
];

fn collect_findings(check_name: &str, json: &Value) -> Vec<Finding> {
    let mut out: Vec<Finding> = Vec::new();
    let mut push_from = |arr: Option<&Vec<Value>>, build: &dyn Fn(&Value) -> Option<Finding>| {
        if let Some(arr) = arr {
            for entry in arr {
                if out.len() >= MAX_FINDINGS {
                    return;
                }
                if let Some(f) = build(entry) {
                    out.push(f);
                }
            }
        }
    };

    push_from(
        json.get("complexity")
            .and_then(|c| c.get("findings"))
            .and_then(Value::as_array),
        &|e| complexity_finding(check_name, e),
    );

    if let Some(dead) = json.get("dead_code") {
        for key in DEAD_CODE_KEYS {
            push_from(dead.get(key).and_then(Value::as_array), &|e| {
                dead_code_finding(check_name, key, e)
            });
        }
    }

    push_from(
        json.get("duplication")
            .and_then(|d| d.get("clone_groups"))
            .and_then(Value::as_array),
        &|e| duplication_finding(check_name, e),
    );

    out
}

fn complexity_finding(check_name: &str, entry: &Value) -> Option<Finding> {
    let str_field = |k: &str| entry.get(k).and_then(Value::as_str);
    let u64_field = |k: &str| entry.get(k).and_then(Value::as_u64);
    let name = str_field("name").unwrap_or("?");
    let line = u64_field("line").map(|n| n as u32);
    let mut detail = format!("complexity: function `{name}`");
    match (u64_field("cyclomatic"), u64_field("cognitive")) {
        (Some(c), Some(g)) => detail.push_str(&format!(" (cyclomatic={c}, cognitive={g})")),
        (Some(c), None) => detail.push_str(&format!(" (cyclomatic={c})")),
        _ => {}
    }
    Some(finding(
        check_name,
        "complexity",
        &detail,
        str_field("path").map(str::to_string),
        line,
        severity_from(str_field("severity").unwrap_or(""), Severity::Warn),
    ))
}

fn dead_code_finding(check_name: &str, kind: &str, entry: &Value) -> Option<Finding> {
    let str_field = |k: &str| entry.get(k).and_then(Value::as_str);
    let path = str_field("path");
    let line = entry.get("line").and_then(Value::as_u64).map(|n| n as u32);
    // fallow uses different label keys per dead-code category; check
    // each in priority order so the rendered detail says "unused
    // export `foo`" rather than the bare category.
    let label = ["export_name", "type_name", "dependency", "name"]
        .iter()
        .find_map(|k| str_field(k));
    let kind_pretty = kind.replace('_', " ");
    let detail = match (label, path) {
        (Some(name), _) => format!("{kind_pretty}: `{name}`"),
        (None, Some(p)) => format!("{kind_pretty}: {p}"),
        (None, None) => kind_pretty.clone(),
    };
    Some(finding(
        check_name,
        kind,
        &detail,
        path.map(str::to_string),
        line,
        Severity::Error,
    ))
}

fn duplication_finding(check_name: &str, entry: &Value) -> Option<Finding> {
    // Clone groups carry an array of sites; surface the first as the
    // finding's location and report the per-group counts in the detail.
    let sites = entry.get("sites").and_then(Value::as_array)?;
    let first = sites.first()?;
    let path = first.get("path").and_then(Value::as_str);
    let line = first
        .get("start_line")
        .and_then(Value::as_u64)
        .map(|n| n as u32);
    let lines = entry
        .get("lines")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let detail = format!("duplication: {} clones across {lines} lines", sites.len());
    Some(finding(
        check_name,
        "duplication",
        &detail,
        path.map(str::to_string),
        line,
        Severity::Error,
    ))
}

fn severity_from(raw: &str, fallback: Severity) -> Severity {
    match raw {
        "critical" | "high" | "error" => Severity::Error,
        "moderate" | "medium" | "warn" | "warning" => Severity::Warn,
        "low" | "info" => Severity::Info,
        _ => fallback,
    }
}

/// Build a one-line summary used as the verdict's `message`. Pulls the
/// canonical counts from `summary.*` fields rather than recomputing
/// from `findings.len()` so a truncated finding list still reports the
/// real totals.
fn summarise(json: &Value, findings: &[Finding]) -> String {
    let count = |key: &str| -> u64 {
        json.get("summary")
            .and_then(|s| s.get(key))
            .and_then(Value::as_u64)
            .unwrap_or_default()
    };
    let dead = count("dead_code_issues");
    let complexity = count("complexity_findings");
    let dupes = count("duplication_clone_groups");
    if dead + complexity + dupes == 0 {
        return format!("fallow audit failed ({} findings)", findings.len());
    }
    format!("fallow audit failed ({dead} dead-code, {complexity} complexity, {dupes} duplication)")
}

/// Lazily run `fallow --version`, parse the major.minor, and return a
/// warning when it falls outside the supported range. `None` means the
/// version is fine *or* we couldn't probe fallow; both cases swallow
/// the warning. Cached for the lifetime of the process — a klasp gate
/// invocation typically resolves `fallow` from the same `$PATH` entry
/// for every check, so re-running the probe per check would multiply
/// subprocess overhead by N for no signal.
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
    if (major, minor) > MAX_SUPPORTED_VERSION {
        let (rmaj, rmin) = MAX_SUPPORTED_VERSION;
        return Some(format!(
            "fallow {major}.{minor} is newer than the maximum tested version \
             {rmaj}.{rmin}; output parsing may have drifted"
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
    use std::path::{Path, PathBuf};

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
    fn pass_verdict_yields_pass() {
        let json = r#"{"schema_version":3,"version":"2.62.0","verdict":"pass","summary":{}}"#;
        let v = outcome_to_verdict("audit", &outcome(Some(0), json, ""), None);
        assert!(matches!(v, Verdict::Pass));
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
    fn fail_verdict_with_complexity_finding_yields_fail() {
        let json = r#"{
            "verdict":"fail",
            "summary":{"dead_code_issues":0,"complexity_findings":1,"duplication_clone_groups":0},
            "complexity":{"findings":[
                {"path":"src/index.ts","name":"tooComplex","line":7,
                 "cyclomatic":7,"cognitive":21,"severity":"high"}
            ]},
            "dead_code":{},"duplication":{}
        }"#;
        let v = outcome_to_verdict("audit", &outcome(Some(1), json, ""), None);
        match v {
            Verdict::Fail { findings, message } => {
                assert_eq!(findings.len(), 1);
                assert_eq!(findings[0].severity, Severity::Error);
                assert_eq!(findings[0].file.as_deref(), Some("src/index.ts"));
                assert_eq!(findings[0].line, Some(7));
                assert!(findings[0].message.contains("tooComplex"));
                assert!(message.contains("1 complexity"));
            }
            other => panic!("expected Fail, got {other:?}"),
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
    fn warn_verdict_demotes_error_findings_to_warn() {
        let json = r#"{
            "verdict":"warn",
            "summary":{},
            "complexity":{"findings":[
                {"path":"src/x.ts","name":"medium","line":3,"severity":"high"}
            ]},
            "dead_code":{},"duplication":{}
        }"#;
        let v = outcome_to_verdict("audit", &outcome(Some(0), json, ""), None);
        match v {
            Verdict::Warn { findings, .. } => {
                assert_eq!(findings.len(), 1);
                assert_eq!(findings[0].severity, Severity::Warn);
            }
            other => panic!("expected Warn, got {other:?}"),
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
    fn dead_code_findings_extracted_with_locations() {
        let json = r#"{
            "verdict":"fail",
            "summary":{"dead_code_issues":1,"complexity_findings":0,"duplication_clone_groups":0},
            "complexity":{"findings":[]},
            "duplication":{},
            "dead_code":{"unused_exports":[
                {"path":"src/foo.ts","export_name":"unused","line":1,"col":13}
            ]}
        }"#;
        let v = outcome_to_verdict("audit", &outcome(Some(1), json, ""), None);
        match v {
            Verdict::Fail { findings, .. } => {
                let dead = findings
                    .iter()
                    .find(|f| f.rule.contains("unused_exports"))
                    .expect("expected unused_exports finding");
                assert_eq!(dead.file.as_deref(), Some("src/foo.ts"));
                assert_eq!(dead.line, Some(1));
                assert!(dead.message.contains("unused"));
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

    #[test]
    fn fallow_config_round_trips_path_buf() {
        // `config_path` is a `PathBuf` rather than a `String`; this guards
        // against an accidental flip back to `String` (which would lose
        // the typed-path API on the surface).
        let c = CheckConfig {
            name: "audit".into(),
            triggers: vec![],
            source: CheckSourceConfig::Fallow {
                config_path: Some(PathBuf::from("tools/.fallowrc.json")),
                base: Some("origin/main".into()),
            },
            timeout_secs: None,
        };
        match c.source {
            CheckSourceConfig::Fallow { config_path, base } => {
                assert_eq!(
                    config_path.as_deref(),
                    Some(Path::new("tools/.fallowrc.json"))
                );
                assert_eq!(base.as_deref(), Some("origin/main"));
            }
            _ => unreachable!(),
        }
    }
}
