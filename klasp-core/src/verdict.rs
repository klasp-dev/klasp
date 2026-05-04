//! `Verdict` — three-tier outcome of a check run, plus aggregation policies.
//!
//! Design: [docs/design.md §3.4]. The Warn tier is the staged-rollout gradient
//! that lets new checks land without immediately blocking. `Finding` derives
//! `Clone` because verdicts are merged across multiple `CheckResult`s — the
//! aggregation in `Verdict::merge` requires copies of the input findings.

use std::process::ExitCode;

use serde::{Deserialize, Serialize};

/// Severity attached to an individual `Finding`. Verdict tier is decided by
/// the runtime aggregating findings; severity here is informational metadata
/// for the rendered block message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warn,
    Error,
}

/// A single actionable item produced by a check (a lint hit, a failed test,
/// a rule violation). Renders into the agent-visible block message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub rule: String,
    pub message: String,
    /// File path the finding refers to, formatted via `Path::to_string_lossy()`
    /// at construction time. Stored as `String` (not `PathBuf`) so JSON
    /// serialisation on Windows can't fail on non-UTF-8 paths — findings are
    /// human-rendered anyway, lossy display is acceptable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub severity: Severity,
}

/// Three-tier verdict. `Pass` and `Warn` are non-blocking (exit 0); `Fail` is
/// blocking (exit 2 — the Claude Code convention for "deny the tool call").
#[derive(Debug, Clone)]
pub enum Verdict {
    Pass,
    Warn {
        findings: Vec<Finding>,
        message: Option<String>,
    },
    Fail {
        findings: Vec<Finding>,
        message: String,
    },
}

impl Verdict {
    /// Only `Fail` blocks the tool call. `Warn` renders a notice but allows
    /// the agent to proceed.
    pub fn is_blocking(&self) -> bool {
        matches!(self, Verdict::Fail { .. })
    }

    /// Map a verdict to the `klasp gate` process exit code.
    /// Pass / Warn → 0, Fail → 2.
    pub fn exit_code(&self) -> ExitCode {
        if self.is_blocking() {
            ExitCode::from(2)
        } else {
            ExitCode::SUCCESS
        }
    }

    /// Aggregate per-check verdicts into a single final verdict.
    /// v0.1 only ships [`VerdictPolicy::AnyFail`].
    pub fn merge(verdicts: Vec<Verdict>, policy: VerdictPolicy) -> Verdict {
        match policy {
            VerdictPolicy::AnyFail => merge_any_fail(verdicts),
        }
    }
}

fn merge_any_fail(verdicts: Vec<Verdict>) -> Verdict {
    let mut fail_findings: Vec<Finding> = Vec::new();
    let mut fail_messages: Vec<String> = Vec::new();
    let mut warn_findings: Vec<Finding> = Vec::new();
    let mut warn_messages: Vec<String> = Vec::new();

    for verdict in verdicts {
        match verdict {
            Verdict::Pass => {}
            Verdict::Warn { findings, message } => {
                warn_findings.extend(findings);
                if let Some(m) = message {
                    warn_messages.push(m);
                }
            }
            Verdict::Fail { findings, message } => {
                fail_findings.extend(findings);
                fail_messages.push(message);
            }
        }
    }

    if !fail_messages.is_empty() {
        Verdict::Fail {
            findings: fail_findings,
            message: fail_messages.join("\n"),
        }
    } else if !warn_findings.is_empty() || !warn_messages.is_empty() {
        Verdict::Warn {
            findings: warn_findings,
            message: if warn_messages.is_empty() {
                None
            } else {
                Some(warn_messages.join("\n"))
            },
        }
    } else {
        Verdict::Pass
    }
}

/// Policy for combining multiple per-check verdicts.
///
/// v0.1 ships only `AnyFail` (any failing check fails the gate). v0.2.5 adds
/// `AllFail` and `MajorityFail` per [docs/roadmap.md].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerdictPolicy {
    #[default]
    AnyFail,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(rule: &str, severity: Severity) -> Finding {
        Finding {
            rule: rule.into(),
            message: "msg".into(),
            file: None,
            line: None,
            severity,
        }
    }

    #[test]
    fn pass_is_not_blocking() {
        assert!(!Verdict::Pass.is_blocking());
    }

    #[test]
    fn warn_is_not_blocking() {
        let v = Verdict::Warn {
            findings: vec![finding("r", Severity::Warn)],
            message: None,
        };
        assert!(!v.is_blocking());
    }

    #[test]
    fn fail_is_blocking() {
        let v = Verdict::Fail {
            findings: vec![],
            message: "boom".into(),
        };
        assert!(v.is_blocking());
    }

    #[test]
    fn merge_empty_is_pass() {
        let v = Verdict::merge(vec![], VerdictPolicy::AnyFail);
        assert!(matches!(v, Verdict::Pass));
    }

    #[test]
    fn merge_all_pass_is_pass() {
        let v = Verdict::merge(
            vec![Verdict::Pass, Verdict::Pass, Verdict::Pass],
            VerdictPolicy::AnyFail,
        );
        assert!(matches!(v, Verdict::Pass));
    }

    #[test]
    fn merge_warn_among_pass_is_warn() {
        let v = Verdict::merge(
            vec![
                Verdict::Pass,
                Verdict::Warn {
                    findings: vec![finding("a", Severity::Warn)],
                    message: Some("notice".into()),
                },
                Verdict::Pass,
            ],
            VerdictPolicy::AnyFail,
        );
        match v {
            Verdict::Warn { findings, message } => {
                assert_eq!(findings.len(), 1);
                assert_eq!(message.as_deref(), Some("notice"));
            }
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn merge_any_fail_is_fail() {
        let v = Verdict::merge(
            vec![
                Verdict::Pass,
                Verdict::Warn {
                    findings: vec![finding("w", Severity::Warn)],
                    message: None,
                },
                Verdict::Fail {
                    findings: vec![finding("f", Severity::Error)],
                    message: "broken".into(),
                },
            ],
            VerdictPolicy::AnyFail,
        );
        match v {
            Verdict::Fail { findings, message } => {
                assert_eq!(findings.len(), 1);
                assert_eq!(message, "broken");
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }
}
