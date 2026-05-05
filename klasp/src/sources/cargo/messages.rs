//! Cargo `--message-format=json` walkers — diagnostic extraction and
//! summary helpers.
//!
//! Lifted out of the sibling `super` module to keep `cargo.rs` under
//! the project's 500-line cap, mirroring the W5 split between
//! `fallow.rs` and `fallow/json.rs`. Everything in here walks NDJSON
//! `serde_json::Value`s or stdout text and returns
//! [`klasp_core::Finding`]s or summary strings; nothing here spawns
//! subprocesses or touches the filesystem.
//!
//! ## Cargo's compiler-message shape
//!
//! `cargo check`, `cargo clippy`, and `cargo build` with
//! `--message-format=json` emit a stream of newline-delimited JSON
//! values. Each line is one message; the ones we care about have
//! `reason = "compiler-message"` and the diagnostic in a nested
//! `message` object whose shape mirrors `rustc`'s own JSON
//! diagnostic format:
//!
//! ```json
//! {
//!   "reason": "compiler-message",
//!   "package_id": "foo 0.1.0",
//!   "message": {
//!     "message": "cannot find value `x`",
//!     "code": { "code": "E0425" },
//!     "level": "error",
//!     "spans": [
//!       { "file_name": "src/lib.rs", "line_start": 3, "is_primary": true }
//!     ]
//!   }
//! }
//! ```

use klasp_core::{Finding, Severity};
use serde_json::Value;

use super::verdict::finding;
use super::MAX_FINDINGS;

/// Walk cargo's NDJSON output and emit one structured finding per
/// `compiler-message` whose level is `error` or `warning`. Capped at
/// [`MAX_FINDINGS`] so a fresh-checkout build doesn't drown the
/// agent's stderr.
pub(super) fn collect_compiler_diagnostics(check_name: &str, stdout: &str) -> Vec<Finding> {
    let mut out: Vec<Finding> = Vec::new();
    for line in stdout.lines() {
        if out.len() >= MAX_FINDINGS {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        if value.get("reason").and_then(Value::as_str) != Some("compiler-message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(finding) = build_diagnostic_finding(check_name, message) else {
            continue;
        };
        out.push(finding);
    }
    out
}

fn build_diagnostic_finding(check_name: &str, message: &Value) -> Option<Finding> {
    let level = message.get("level").and_then(Value::as_str)?;
    let severity = severity_from_level(level)?;
    let text = message.get("message").and_then(Value::as_str)?.to_string();
    let code = message
        .get("code")
        .and_then(|c| c.get("code"))
        .and_then(Value::as_str);
    // Pick the primary span if present; else the first span.
    let primary_span = message
        .get("spans")
        .and_then(Value::as_array)
        .and_then(|spans| {
            spans
                .iter()
                .find(|s| s.get("is_primary").and_then(Value::as_bool) == Some(true))
                .or_else(|| spans.first())
        });
    let file = primary_span
        .and_then(|s| s.get("file_name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let line = primary_span
        .and_then(|s| s.get("line_start"))
        .and_then(Value::as_u64)
        .map(|n| n as u32);

    let suffix = code.unwrap_or(level);
    let detail = match code {
        Some(c) => format!("{level}[{c}]: {text}"),
        None => format!("{level}: {text}"),
    };
    Some(finding(check_name, suffix, &detail, file, line, severity))
}

fn severity_from_level(level: &str) -> Option<Severity> {
    match level {
        "error" | "error: internal compiler error" => Some(Severity::Error),
        "warning" => Some(Severity::Warn),
        "help" | "note" => Some(Severity::Info),
        _ => None,
    }
}

/// Render the verdict-level summary for non-test cargo invocations.
/// Counts diagnostics by severity so the agent's block message reads
/// `cargo clippy: 3 errors, 1 warning` rather than a bare exit code.
pub(super) fn summarise_diagnostics(subcommand: &str, findings: &[Finding]) -> String {
    let errors = findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .count();
    let warnings = findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::Warn))
        .count();
    let info = findings
        .iter()
        .filter(|f| matches!(f.severity, Severity::Info))
        .count();
    match (errors, warnings) {
        // The `(0, 0)` arm is reached when `findings` carries only
        // `Severity::Info` rows (cargo's `help` / `note` diagnostics).
        // Be honest about it — naming "info notes" rather than the
        // catch-all "diagnostics" stops the agent guessing whether
        // something error-level was missed by the parser.
        (0, 0) => format!(
            "cargo {subcommand} reported {info} info note{}",
            if info == 1 { "" } else { "s" }
        ),
        (e, 0) => format!(
            "cargo {subcommand} reported {e} error{}",
            if e == 1 { "" } else { "s" }
        ),
        (0, w) => format!(
            "cargo {subcommand} reported {w} warning{}",
            if w == 1 { "" } else { "s" }
        ),
        (e, w) => format!(
            "cargo {subcommand} reported {e} error{}, {w} warning{}",
            if e == 1 { "" } else { "s" },
            if w == 1 { "" } else { "s" }
        ),
    }
}

/// Pull the `test result: …` summary line out of a `cargo test` run.
/// Format: `test result: FAILED. 4 passed; 1 failed; 0 ignored; 0
/// measured; 0 filtered out; finished in 0.04s`. Returns the rendered
/// "cargo test failed (4 passed, 1 failed)" detail or `None` if the
/// summary is absent (test runner crashed before printing it).
pub(super) fn summarise_test_output(stdout: &str) -> Option<String> {
    let line = stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with("test result:"))?;
    let passed = parse_count(line, "passed");
    let failed = parse_count(line, "failed");
    let ignored = parse_count(line, "ignored");
    Some(match (passed, failed, ignored) {
        (Some(p), Some(f), _) if f > 0 => {
            format!("cargo test failed ({p} passed, {f} failed)")
        }
        (Some(p), Some(0), Some(i)) if i > 0 => {
            format!("cargo test passed ({p} passed, {i} ignored)")
        }
        _ => line.trim().to_string(),
    })
}

/// Walk a `test result:` line for `<N> <kind>;` patterns. Returns the
/// number associated with `kind` (`"passed"`, `"failed"`, …) or
/// `None` when the kind isn't present.
fn parse_count(line: &str, kind: &str) -> Option<u64> {
    // Pattern: digits followed by whitespace, then the kind word.
    let mut tokens = line.split_whitespace().peekable();
    while let Some(tok) = tokens.next() {
        if let Ok(n) = tok.parse::<u64>() {
            // peek the next token — strip a trailing semicolon if present
            if let Some(next) = tokens.peek() {
                let stripped = next.trim_end_matches(';').trim_end_matches(',');
                if stripped == kind {
                    return Some(n);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stdout_yields_no_findings() {
        assert!(collect_compiler_diagnostics("build", "").is_empty());
    }

    #[test]
    fn non_compiler_message_lines_skipped() {
        let stdout = concat!(
            r#"{"reason":"compiler-artifact","package_id":"foo","target":{}}"#,
            "\n",
            r#"{"reason":"build-finished","success":true}"#,
            "\n"
        );
        assert!(collect_compiler_diagnostics("build", stdout).is_empty());
    }

    #[test]
    fn compiler_message_with_error_yields_finding() {
        let stdout = concat!(
            r#"{"reason":"compiler-message","message":{"message":"cannot find value `x`","code":{"code":"E0425"},"level":"error","spans":[{"file_name":"src/lib.rs","line_start":7,"is_primary":true}]}}"#,
            "\n"
        );
        let findings = collect_compiler_diagnostics("build", stdout);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].message.contains("E0425"));
        assert_eq!(findings[0].file.as_deref(), Some("src/lib.rs"));
        assert_eq!(findings[0].line, Some(7));
        assert!(findings[0].rule.contains("E0425"));
    }

    #[test]
    fn compiler_message_with_warning_uses_warn_severity() {
        let stdout = concat!(
            r#"{"reason":"compiler-message","message":{"message":"unused variable","level":"warning","spans":[{"file_name":"a.rs","line_start":1,"is_primary":true}]}}"#,
            "\n"
        );
        let findings = collect_compiler_diagnostics("build", stdout);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Warn);
    }

    #[test]
    fn compiler_message_skipped_when_no_level() {
        let stdout = r#"{"reason":"compiler-message","message":{"message":"odd","spans":[]}}"#;
        assert!(collect_compiler_diagnostics("build", stdout).is_empty());
    }

    #[test]
    fn malformed_json_lines_silently_skipped() {
        let stdout = concat!(
            "garbage\n",
            r#"{"reason":"compiler-message","message":{"message":"a","level":"error","spans":[]}}"#,
            "\n",
            "more garbage\n",
        );
        let findings = collect_compiler_diagnostics("build", stdout);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn first_span_used_when_no_primary_marker() {
        let stdout = concat!(
            r#"{"reason":"compiler-message","message":{"message":"x","level":"error","spans":[{"file_name":"a.rs","line_start":2,"is_primary":false},{"file_name":"b.rs","line_start":9,"is_primary":false}]}}"#,
            "\n"
        );
        let findings = collect_compiler_diagnostics("b", stdout);
        assert_eq!(findings[0].file.as_deref(), Some("a.rs"));
        assert_eq!(findings[0].line, Some(2));
    }

    #[test]
    fn cap_limits_diagnostic_count() {
        let mut stdout = String::new();
        for _ in 0..70 {
            stdout.push_str(
                r#"{"reason":"compiler-message","message":{"message":"x","level":"error","spans":[]}}"#,
            );
            stdout.push('\n');
        }
        let findings = collect_compiler_diagnostics("b", &stdout);
        assert_eq!(findings.len(), MAX_FINDINGS);
    }

    #[test]
    fn summarise_diagnostics_pluralisation() {
        let one_err = vec![mk_finding(Severity::Error)];
        assert_eq!(
            summarise_diagnostics("clippy", &one_err),
            "cargo clippy reported 1 error"
        );
        let many = vec![
            mk_finding(Severity::Error),
            mk_finding(Severity::Error),
            mk_finding(Severity::Warn),
        ];
        let s = summarise_diagnostics("clippy", &many);
        assert!(s.contains("2 errors"));
        assert!(s.contains("1 warning"));
    }

    #[test]
    fn summarise_test_output_extracts_failed_count() {
        let stdout = concat!(
            "running 5 tests\n",
            "test foo ... ok\n",
            "test bar ... FAILED\n",
            "test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s\n",
        );
        let summary = summarise_test_output(stdout).expect("summary line should parse");
        assert!(summary.contains("4 passed"));
        assert!(summary.contains("1 failed"));
    }

    #[test]
    fn summarise_test_output_returns_none_when_summary_missing() {
        // A panic before the summary line means the runner crashed —
        // the recipe falls back to the generic exit-code message.
        assert!(summarise_test_output("running 5 tests\nthread 'main' panicked").is_none());
    }

    fn mk_finding(severity: Severity) -> Finding {
        Finding {
            rule: "r".into(),
            message: "m".into(),
            file: None,
            line: None,
            severity,
        }
    }
}
