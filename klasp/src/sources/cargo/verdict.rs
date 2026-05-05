//! Verdict-shaping helpers for the cargo recipe — exit-code mapping,
//! finding builders, and version sniffing.
//!
//! Lifted out of the sibling `super` module to keep `cargo.rs` under
//! the project's 500-line cap. Mirrors the same split pattern used by
//! `cargo/messages.rs`: pure helpers with no [`super::CheckSource`]
//! traffic, exposed via `pub(super)` so the parent module can compose
//! them into its `CheckSource::run` impl.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use klasp_core::{Finding, Severity, Verdict};

use super::messages::{collect_compiler_diagnostics, summarise_diagnostics, summarise_test_output};
use super::ShellOutcome;

/// Lowest cargo release the recipe was developed against. Cargo's
/// `--message-format=json` shape has been stable since 1.0 and the
/// `test result:` summary line has been stable since cargo's own
/// inception; the version sniff is informational only.
const MIN_SUPPORTED_VERSION: (u32, u32) = (1, 0);

pub(super) fn outcome_to_verdict(
    check_name: &str,
    subcommand: &str,
    outcome: &ShellOutcome,
    version_warning: Option<&str>,
) -> Verdict {
    match outcome.status_code {
        Some(0) => match version_warning {
            None => Verdict::Pass,
            Some(warning) => Verdict::Warn {
                findings: vec![note(check_name, warning, Severity::Warn)],
                message: Some(warning.to_string()),
            },
        },
        Some(code) => {
            let mut findings = if subcommand == "test" {
                Vec::new()
            } else {
                collect_compiler_diagnostics(check_name, &outcome.stdout)
            };
            let message = if subcommand == "test" {
                let detail = summarise_test_output(&outcome.stdout).unwrap_or_else(|| {
                    let trimmed = outcome.stderr.trim();
                    if trimmed.is_empty() {
                        format!("cargo test `{check_name}` failed (exit {code})")
                    } else {
                        format!("cargo test `{check_name}` failed (exit {code}): {trimmed}")
                    }
                });
                findings.push(note(check_name, &detail, Severity::Error));
                detail
            } else if findings.is_empty() {
                let trimmed = outcome.stderr.trim();
                let detail = if trimmed.is_empty() {
                    format!("cargo {subcommand} `{check_name}` failed (exit {code})")
                } else {
                    format!("cargo {subcommand} `{check_name}` failed (exit {code}): {trimmed}")
                };
                findings.push(note(check_name, &detail, Severity::Error));
                detail
            } else {
                summarise_diagnostics(subcommand, &findings)
            };
            if let Some(warning) = version_warning {
                findings.insert(0, note(check_name, warning, Severity::Warn));
            }
            Verdict::Fail { findings, message }
        }
        None => fail_with_optional_warning(
            check_name,
            format!("cargo `{check_name}` was terminated before producing an exit code"),
            version_warning,
        ),
    }
}

pub(super) fn fail_with_optional_warning(
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

/// Centralised `Finding` builder. `rule_suffix = ""` produces a top-
/// level rule (`cargo:<check>`); a non-empty suffix nests it.
pub(super) fn finding(
    check_name: &str,
    rule_suffix: &str,
    message: &str,
    file: Option<String>,
    line: Option<u32>,
    severity: Severity,
) -> Finding {
    let rule = if rule_suffix.is_empty() {
        format!("cargo:{check_name}")
    } else {
        format!("cargo:{check_name}:{rule_suffix}")
    };
    Finding {
        rule,
        message: message.to_string(),
        file,
        line,
        severity,
    }
}

pub(super) fn note(check_name: &str, message: &str, severity: Severity) -> Finding {
    finding(check_name, "", message, None, None, severity)
}

/// Cached `cargo --version` probe. Same shape as fallow / pre_commit /
/// pytest's sniffs — see `fallow.rs` for the rationale on
/// cwd-insensitive caching.
pub(super) fn sniff_version_warning(cwd: &Path) -> Option<String> {
    static CACHED: OnceLock<Option<String>> = OnceLock::new();
    CACHED
        .get_or_init(|| sniff_version_warning_uncached(cwd))
        .clone()
}

fn sniff_version_warning_uncached(cwd: &Path) -> Option<String> {
    let output = Command::new("cargo")
        .arg("--version")
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).into_owned();
    let (major, minor) = parse_version(&raw)?;
    if (major, minor) < MIN_SUPPORTED_VERSION {
        let (rmaj, rmin) = MIN_SUPPORTED_VERSION;
        return Some(format!(
            "cargo {major}.{minor} is older than the minimum tested version \
             {rmaj}.{rmin}; output parsing may be incomplete"
        ));
    }
    None
}

/// Parse `"cargo 1.79.0 (ded6e..)\n"` → `Some((1, 79))`. Tolerant — the
/// banner sometimes carries a build-date suffix; we only inspect the
/// first dot-separated pair we find.
pub(super) fn parse_version(raw: &str) -> Option<(u32, u32)> {
    let line = raw.lines().find(|l| !l.trim().is_empty())?;
    for token in line.split_whitespace() {
        let mut parts = token.split('.');
        let Some(maj_raw) = parts.next() else {
            continue;
        };
        let Some(min_raw) = parts.next() else {
            continue;
        };
        let Ok(major) = maj_raw.parse::<u32>() else {
            continue;
        };
        let Ok(minor) = min_raw.parse::<u32>() else {
            continue;
        };
        return Some((major, minor));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(code: Option<i32>, stdout: &str, stderr: &str) -> ShellOutcome {
        ShellOutcome {
            status_code: code,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn pass_with_version_warning_is_warn() {
        let v = outcome_to_verdict("build", "check", &outcome(Some(0), "", ""), Some("old"));
        assert!(matches!(v, Verdict::Warn { .. }));
    }

    #[test]
    fn check_failure_with_diagnostic_yields_finding() {
        let stdout = concat!(
            r#"{"reason":"compiler-message","message":{"message":"cannot find value `x`","code":{"code":"E0425"},"level":"error","spans":[{"file_name":"src/lib.rs","line_start":3,"is_primary":true}]}}"#,
            "\n",
        );
        let v = outcome_to_verdict("build", "check", &outcome(Some(101), stdout, ""), None);
        match v {
            Verdict::Fail { findings, message } => {
                assert!(findings.iter().any(|f| f.message.contains("E0425")));
                assert!(message.contains("cargo check"));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn check_failure_without_parseable_stdout_falls_back_to_generic() {
        let v = outcome_to_verdict("build", "check", &outcome(Some(101), "", "boom"), None);
        match v {
            Verdict::Fail { findings, message } => {
                assert_eq!(findings.len(), 1);
                assert!(findings[0].message.contains("boom"));
                assert!(message.contains("exit 101"));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn test_failure_uses_summary_line() {
        let stdout = concat!(
            "running 5 tests\n",
            "test result: FAILED. 4 passed; 1 failed; 0 ignored\n",
        );
        let v = outcome_to_verdict("tests", "test", &outcome(Some(101), stdout, ""), None);
        match v {
            Verdict::Fail { message, .. } => assert!(message.contains("1 failed")),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn fail_with_version_warning_prepends_warn_finding() {
        let v = outcome_to_verdict(
            "build",
            "check",
            &outcome(Some(101), "", "boom"),
            Some("old cargo"),
        );
        match v {
            Verdict::Fail { findings, .. } => {
                assert!(findings.len() >= 2);
                assert_eq!(findings[0].severity, Severity::Warn);
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn no_exit_code_is_fail() {
        let v = outcome_to_verdict("build", "check", &outcome(None, "", ""), None);
        assert!(matches!(v, Verdict::Fail { .. }));
    }

    #[test]
    fn parse_version_extracts_major_minor() {
        assert_eq!(
            parse_version("cargo 1.79.0 (ded6ed5ec 2024-04-19)"),
            Some((1, 79))
        );
        assert_eq!(parse_version("cargo 1.95.0\n"), Some((1, 95)));
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("not a version"), None);
    }
}
