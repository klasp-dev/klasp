//! Verdict-shaping helpers for the pytest recipe — exit-code mapping,
//! finding builders, and version sniffing.
//!
//! Lifted out of the sibling `super` module to keep `pytest.rs` under
//! the project's 500-line cap, mirroring the W5 split between
//! `fallow.rs` and `fallow/json.rs`. Pure helpers; no
//! [`super::CheckSource`] traffic, exposed via `pub(super)` so the
//! parent module can compose them into its `CheckSource::run` impl.

use std::path::Path;

use klasp_core::{Finding, Severity, Verdict};

use super::junit::{collect_failures, summarise_failures};
use super::ShellOutcome;
use crate::sources::recipe_util;

/// Lowest pytest release whose JUnit XML schema and exit-code semantics
/// match the parser in `junit::collect_failures`. klasp 0.2 was
/// developed against pytest 7.x and 8.x; 6.x and earlier are out of
/// scope. There is deliberately no upper bound — pytest's JUnit
/// emission has been stable since 5.0 and emitting a "newer than
/// tested" notice on every release would be toil-without-signal.
const MIN_SUPPORTED_VERSION: (u32, u32) = (7, 0);

pub(super) fn outcome_to_verdict(
    check_name: &str,
    outcome: &ShellOutcome,
    junit_xml: Option<&str>,
    version_warning: Option<&str>,
) -> Verdict {
    match outcome.status_code {
        // Exit 0 = tests ran clean. Exit 5 = pytest collected no tests; in a
        // diff-scoped commit gate this is the common benign case of a commit
        // that staged no Python (e.g. a Rust-only change in a polyglot repo),
        // so it is treated as a no-op pass rather than a block. A genuinely
        // vanished test suite is better caught by CI running the full suite
        // than by the commit gate. See docs/recipes.md and the klasp.toml note.
        Some(0) | Some(5) => match version_warning {
            None => Verdict::Pass,
            Some(warning) => Verdict::Warn {
                findings: vec![note(check_name, warning, Severity::Warn)],
                message: Some(warning.to_string()),
            },
        },
        Some(1) => {
            let mut findings = if let Some(xml) = junit_xml {
                collect_failures(check_name, xml)
            } else {
                Vec::new()
            };
            let message = if findings.is_empty() {
                let trimmed = outcome.stderr.trim();
                let detail = if trimmed.is_empty() {
                    format!("pytest `{check_name}` reported test failures")
                } else {
                    format!("pytest `{check_name}` reported test failures: {trimmed}")
                };
                findings.push(note(check_name, &detail, Severity::Error));
                detail
            } else {
                summarise_failures(&findings)
            };
            if let Some(warning) = version_warning {
                findings.insert(0, note(check_name, warning, Severity::Warn));
            }
            Verdict::Fail { findings, message }
        }
        Some(other) => {
            let detail = exit_code_detail(check_name, other, outcome.stderr.trim());
            fail_with_optional_warning(check_name, detail, version_warning)
        }
        None => fail_with_optional_warning(
            check_name,
            format!("pytest `{check_name}` was terminated before producing an exit code"),
            version_warning,
        ),
    }
}

/// Map pytest's documented exit codes to a human-readable detail.
/// Source: <https://docs.pytest.org/en/stable/reference/exit-codes.html>.
fn exit_code_detail(check_name: &str, code: i32, stderr_trimmed: &str) -> String {
    let cause = match code {
        2 => "test run was interrupted by the user (KeyboardInterrupt)",
        3 => "internal error happened while executing tests",
        4 => "pytest command line usage error",
        // exit 5 ("no tests collected") is handled as a no-op pass in
        // `outcome_to_verdict` and never reaches here.
        _ => "pytest exited with an unexpected status",
    };
    if stderr_trimmed.is_empty() {
        format!("pytest `{check_name}` exit {code}: {cause}")
    } else {
        format!("pytest `{check_name}` exit {code}: {cause}: {stderr_trimmed}")
    }
}

pub(super) fn fail_with_optional_warning(
    check_name: &str,
    detail: String,
    version_warning: Option<&str>,
) -> Verdict {
    recipe_util::fail_with_optional_warning(super::RULE_PREFIX, check_name, detail, version_warning)
}

/// Centralised `Finding` builder. `rule_suffix = ""` produces a top-
/// level rule (`pytest:<check>`); a non-empty suffix nests it. Thin
/// wrapper over [`recipe_util::finding`] pinning the `pytest:` prefix;
/// re-exported to [`super::junit`] via `use super::verdict::finding`.
pub(super) fn finding(
    check_name: &str,
    rule_suffix: &str,
    message: &str,
    file: Option<String>,
    line: Option<u32>,
    severity: Severity,
) -> Finding {
    recipe_util::finding(
        super::RULE_PREFIX,
        check_name,
        rule_suffix,
        message,
        file,
        line,
        severity,
    )
}

pub(super) fn note(check_name: &str, message: &str, severity: Severity) -> Finding {
    recipe_util::note(super::RULE_PREFIX, check_name, message, severity)
}

/// Cached `pytest --version` probe. Same shape as fallow / pre_commit /
/// cargo's sniffs, except `check_stderr = true`: pytest 7.x prints its
/// banner on stderr while 8.x uses stdout, so the probe concatenates both
/// channels before parsing.
pub(super) fn sniff_version_warning(cwd: &Path) -> Option<String> {
    recipe_util::sniff_version_warning("pytest", MIN_SUPPORTED_VERSION, cwd, true)
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
        let v = outcome_to_verdict("tests", &outcome(Some(0), "", ""), None, Some("too old"));
        assert!(matches!(v, Verdict::Warn { .. }));
    }

    #[test]
    fn fail_without_junit_uses_generic_finding() {
        let v = outcome_to_verdict(
            "tests",
            &outcome(Some(1), "", "FAILED tests/test_x.py"),
            None,
            None,
        );
        match v {
            Verdict::Fail { findings, message } => {
                assert_eq!(findings.len(), 1);
                assert!(findings[0].message.contains("FAILED"));
                assert!(message.contains("test failures"));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn fail_with_junit_xml_yields_per_failure_findings() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
            <testsuites>
              <testsuite name="pytest" tests="2" failures="2">
                <testcase classname="t.x" name="test_alpha" file="tests/test_x.py" line="3">
                  <failure message="assert 1 == 2">stack</failure>
                </testcase>
                <testcase classname="t.x" name="test_beta" file="tests/test_x.py" line="9">
                  <failure message="assert 'a' == 'b'">stack</failure>
                </testcase>
              </testsuite>
            </testsuites>"#;
        let v = outcome_to_verdict("tests", &outcome(Some(1), "", ""), Some(xml), None);
        match v {
            Verdict::Fail { findings, message } => {
                assert_eq!(findings.len(), 2);
                assert!(findings[0].message.contains("test_alpha"));
                assert!(findings[1].message.contains("test_beta"));
                assert!(message.contains("2"));
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn collection_error_exit_5_is_pass() {
        // pytest exit 5 = "no tests collected". In a diff-scoped commit gate
        // this is the benign no-Python-staged case, so it is a no-op pass
        // rather than a block. See `outcome_to_verdict` and docs/recipes.md.
        let v = outcome_to_verdict("tests", &outcome(Some(5), "", ""), None, None);
        assert!(matches!(v, Verdict::Pass), "expected Pass, got {v:?}");
    }

    #[test]
    fn collection_error_exit_5_with_version_warning_is_warn() {
        // The no-tests-collected pass still surfaces a version warning the
        // same way the exit-0 path does — non-blocking, but noted.
        let v = outcome_to_verdict("tests", &outcome(Some(5), "", ""), None, Some("old pytest"));
        match v {
            Verdict::Warn { findings, message } => {
                assert_eq!(findings.len(), 1);
                assert!(message.unwrap().contains("old pytest"));
            }
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn keyboard_interrupt_exit_2_is_fail() {
        let v = outcome_to_verdict("tests", &outcome(Some(2), "", ""), None, None);
        match v {
            Verdict::Fail { message, .. } => assert!(message.contains("interrupted")),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn fail_with_version_warning_prepends_warn_finding() {
        let v = outcome_to_verdict(
            "tests",
            &outcome(Some(1), "", "boom"),
            None,
            Some("old pytest"),
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
        let v = outcome_to_verdict("tests", &outcome(None, "", ""), None, None);
        assert!(matches!(v, Verdict::Fail { .. }));
    }

    #[test]
    fn parse_version_extracts_major_minor() {
        // Version parsing now lives in `recipe_util`; this guards the
        // pytest-shaped banners (including the "This is pytest version …"
        // prefix where the version isn't the last token) against regression.
        assert_eq!(recipe_util::parse_version("pytest 7.4.0"), Some((7, 4)));
        assert_eq!(recipe_util::parse_version("pytest 8.3.2\n"), Some((8, 3)));
        assert_eq!(
            recipe_util::parse_version("This is pytest version 8.0.1, imported from …"),
            Some((8, 0))
        );
        assert_eq!(recipe_util::parse_version(""), None);
        assert_eq!(recipe_util::parse_version("not a version"), None);
    }
}
