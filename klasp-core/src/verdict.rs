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

    /// Aggregate per-check verdicts into a single final verdict using the
    /// supplied [`VerdictPolicy`]. See the policy doc-comment for full
    /// semantics; the summary is:
    /// - `AnyFail` — blocks if any check returned `Fail`.
    /// - `AllFail` — blocks only when every non-`Warn` check returned `Fail`.
    /// - `MajorityFail` — blocks when strictly more than half the non-`Warn`
    ///   checks returned `Fail`.
    pub fn merge(verdicts: Vec<Verdict>, policy: VerdictPolicy) -> Verdict {
        match policy {
            VerdictPolicy::AnyFail => merge_any_fail(verdicts),
            VerdictPolicy::AllFail => merge_all_fail(verdicts),
            VerdictPolicy::MajorityFail => merge_majority_fail(verdicts),
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

/// Partition `verdicts` into three buckets and return the counts + collected
/// warn findings/messages for use in the non-blocking fallback path shared by
/// `merge_all_fail` and `merge_majority_fail`.
struct Partition {
    fail_count: usize,
    pass_count: usize,
    fail_findings: Vec<Finding>,
    fail_messages: Vec<String>,
    warn_findings: Vec<Finding>,
    warn_messages: Vec<String>,
}

fn partition(verdicts: Vec<Verdict>) -> Partition {
    let mut p = Partition {
        fail_count: 0,
        pass_count: 0,
        fail_findings: Vec::new(),
        fail_messages: Vec::new(),
        warn_findings: Vec::new(),
        warn_messages: Vec::new(),
    };
    for v in verdicts {
        match v {
            Verdict::Pass => p.pass_count += 1,
            Verdict::Warn { findings, message } => {
                p.warn_findings.extend(findings);
                if let Some(m) = message {
                    p.warn_messages.push(m);
                }
            }
            Verdict::Fail { findings, message } => {
                p.fail_count += 1;
                p.fail_findings.extend(findings);
                p.fail_messages.push(message);
            }
        }
    }
    p
}

/// Non-blocking fallback used by `AllFail` and `MajorityFail` when the
/// threshold is not met. If any `Fail` verdicts were present (but insufficient
/// to trigger the policy), they are downgraded to a `Warn` so the agent is
/// still informed. Otherwise, any collected `Warn` findings propagate; if
/// nothing at all, returns `Pass`.
fn downgrade_to_warn_or_pass(p: Partition) -> Verdict {
    if !p.fail_messages.is_empty() {
        // Failing checks exist but didn't hit the policy threshold — downgrade.
        let mut findings = p.fail_findings;
        findings.extend(p.warn_findings);
        let mut messages = p.fail_messages;
        messages.extend(p.warn_messages);
        Verdict::Warn {
            findings,
            message: Some(messages.join("\n")),
        }
    } else if !p.warn_findings.is_empty() || !p.warn_messages.is_empty() {
        Verdict::Warn {
            findings: p.warn_findings,
            message: if p.warn_messages.is_empty() {
                None
            } else {
                Some(p.warn_messages.join("\n"))
            },
        }
    } else {
        Verdict::Pass
    }
}

/// `AllFail`: blocks only when at least one check returned `Fail` AND no check
/// returned `Pass` (strict unanimity among the non-`Warn` participants).
/// Mixed `Pass`+`Fail` → `Warn`; empty or all-`Pass` → `Pass`.
fn merge_all_fail(verdicts: Vec<Verdict>) -> Verdict {
    let p = partition(verdicts);
    if p.fail_count > 0 && p.pass_count == 0 {
        Verdict::Fail {
            findings: p.fail_findings,
            message: p.fail_messages.join("\n"),
        }
    } else {
        downgrade_to_warn_or_pass(p)
    }
}

/// `MajorityFail`: blocks when strictly more than half of the non-`Warn`
/// checks returned `Fail`. Ties (equal pass and fail counts) are **not**
/// a majority — they follow the same downgrade path as `AllFail`. Empty → `Pass`.
fn merge_majority_fail(verdicts: Vec<Verdict>) -> Verdict {
    let p = partition(verdicts);
    let total_decisive = p.fail_count + p.pass_count;
    if total_decisive > 0 && p.fail_count * 2 > total_decisive {
        Verdict::Fail {
            findings: p.fail_findings,
            message: p.fail_messages.join("\n"),
        }
    } else {
        downgrade_to_warn_or_pass(p)
    }
}

/// Policy for combining multiple per-check verdicts.
///
/// | Policy | When `Fail` is returned |
/// |---|---|
/// | `AnyFail` | At least one check returned `Fail` (strict, v0.1 default). |
/// | `AllFail` | Every participating check returned `Fail` AND no check returned `Pass` (strict unanimity among non-`Warn` checks). Mixed `Pass`+`Fail` → `Warn`; all `Pass` → `Pass`; empty → `Pass`. |
/// | `MajorityFail` | Strictly more than half of the non-`Warn` checks returned `Fail`. Ties (equal pass/fail counts) are not majority — same downgrade shape as `AllFail`. Empty → `Pass`. |
///
/// `AllFail` edge-case rules (consistently applied by `MajorityFail` for non-blocking outcomes):
/// - Any `Fail` verdict present but not unanimous (or not majority) → `Warn` (the agent is
///   informed but not blocked).
/// - No `Fail` verdicts, some `Warn` → `Warn`.
/// - No `Fail`, no `Warn` (or empty input) → `Pass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerdictPolicy {
    #[default]
    AnyFail,
    /// Gate blocks only when every non-`Warn` check failed (no `Pass` votes).
    AllFail,
    /// Gate blocks only when strictly more than half the non-`Warn` checks failed.
    MajorityFail,
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

    // ── AllFail ──────────────────────────────────────────────────────────────

    fn fail(msg: &str) -> Verdict {
        Verdict::Fail {
            findings: vec![finding(msg, Severity::Error)],
            message: msg.into(),
        }
    }

    fn warn_v() -> Verdict {
        Verdict::Warn {
            findings: vec![finding("w", Severity::Warn)],
            message: Some("warn".into()),
        }
    }

    #[test]
    fn all_fail_empty_is_pass() {
        assert!(matches!(
            Verdict::merge(vec![], VerdictPolicy::AllFail),
            Verdict::Pass
        ));
    }

    #[test]
    fn all_fail_all_pass_is_pass() {
        let v = Verdict::merge(
            vec![Verdict::Pass, Verdict::Pass, Verdict::Pass],
            VerdictPolicy::AllFail,
        );
        assert!(matches!(v, Verdict::Pass));
    }

    #[test]
    fn all_fail_all_fail_is_fail() {
        // 0 pass + 3 fail — unanimous failure → Fail.
        let v = Verdict::merge(
            vec![fail("a"), fail("b"), fail("c")],
            VerdictPolicy::AllFail,
        );
        assert!(v.is_blocking(), "expected Fail (blocking), got {v:?}");
        match v {
            Verdict::Fail { findings, .. } => assert_eq!(findings.len(), 3),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn all_fail_mixed_pass_and_fail_is_warn() {
        // 1 pass + 2 fail — not unanimous → downgrade to Warn.
        let v = Verdict::merge(
            vec![Verdict::Pass, fail("x"), fail("y")],
            VerdictPolicy::AllFail,
        );
        assert!(!v.is_blocking(), "expected non-blocking, got {v:?}");
        assert!(matches!(v, Verdict::Warn { .. }));
    }

    #[test]
    fn all_fail_warn_only_is_warn() {
        // Warns don't count in the decisive set; no fails → Pass through Warn.
        let v = Verdict::merge(vec![warn_v(), Verdict::Pass], VerdictPolicy::AllFail);
        assert!(!v.is_blocking());
        assert!(matches!(v, Verdict::Warn { .. }));
    }

    #[test]
    fn all_fail_only_warns_no_pass_no_fail_is_warn() {
        // Warn-only input: no decisive votes → Warn (not Fail).
        let v = Verdict::merge(vec![warn_v(), warn_v()], VerdictPolicy::AllFail);
        assert!(!v.is_blocking());
        assert!(matches!(v, Verdict::Warn { .. }));
    }

    #[test]
    fn all_fail_fail_with_warn_no_pass_is_fail() {
        // fail_count=2, pass_count=0 → unanimous among decisive votes → Fail.
        // Warn findings are present but don't veto the unanimous Fail.
        let v = Verdict::merge(vec![fail("a"), fail("b"), warn_v()], VerdictPolicy::AllFail);
        assert!(v.is_blocking(), "expected Fail, got {v:?}");
    }

    #[test]
    fn all_fail_single_fail_single_pass_is_warn() {
        // 1 fail + 1 pass → not unanimous → Warn.
        let v = Verdict::merge(vec![fail("f"), Verdict::Pass], VerdictPolicy::AllFail);
        assert!(!v.is_blocking());
        assert!(matches!(v, Verdict::Warn { .. }));
    }

    // ── MajorityFail ─────────────────────────────────────────────────────────

    #[test]
    fn majority_fail_empty_is_pass() {
        assert!(matches!(
            Verdict::merge(vec![], VerdictPolicy::MajorityFail),
            Verdict::Pass
        ));
    }

    #[test]
    fn majority_fail_all_pass_is_pass() {
        let v = Verdict::merge(
            vec![Verdict::Pass, Verdict::Pass, Verdict::Pass],
            VerdictPolicy::MajorityFail,
        );
        assert!(matches!(v, Verdict::Pass));
    }

    #[test]
    fn majority_fail_all_fail_is_fail() {
        // 0 pass + 3 fail → 100% fail → Fail.
        let v = Verdict::merge(
            vec![fail("a"), fail("b"), fail("c")],
            VerdictPolicy::MajorityFail,
        );
        assert!(v.is_blocking(), "expected Fail, got {v:?}");
    }

    #[test]
    fn majority_fail_strict_majority_3p_0f() {
        // 3 pass + 0 fail → no majority → Pass.
        let v = Verdict::merge(
            vec![Verdict::Pass, Verdict::Pass, Verdict::Pass],
            VerdictPolicy::MajorityFail,
        );
        assert!(matches!(v, Verdict::Pass));
    }

    #[test]
    fn majority_fail_strict_majority_1p_2f() {
        // 1 pass + 2 fail → 2/3 → majority → Fail.
        let v = Verdict::merge(
            vec![Verdict::Pass, fail("x"), fail("y")],
            VerdictPolicy::MajorityFail,
        );
        assert!(v.is_blocking(), "expected Fail, got {v:?}");
    }

    #[test]
    fn majority_fail_tie_2p_2f_is_warn() {
        // 2 pass + 2 fail → tie → NOT majority → downgrade to Warn.
        let v = Verdict::merge(
            vec![Verdict::Pass, Verdict::Pass, fail("a"), fail("b")],
            VerdictPolicy::MajorityFail,
        );
        assert!(!v.is_blocking(), "expected non-blocking on tie, got {v:?}");
        assert!(matches!(v, Verdict::Warn { .. }));
    }

    #[test]
    fn majority_fail_2p_1f_is_warn() {
        // 2 pass + 1 fail → not majority → downgrade to Warn.
        let v = Verdict::merge(
            vec![Verdict::Pass, Verdict::Pass, fail("f")],
            VerdictPolicy::MajorityFail,
        );
        assert!(!v.is_blocking());
        assert!(matches!(v, Verdict::Warn { .. }));
    }

    #[test]
    fn majority_fail_warns_ignored_in_count() {
        // Warns don't participate in the decisive count. 1 pass + 1 fail + 2 warns → tie → Warn.
        let v = Verdict::merge(
            vec![Verdict::Pass, fail("f"), warn_v(), warn_v()],
            VerdictPolicy::MajorityFail,
        );
        assert!(!v.is_blocking(), "expected non-blocking, got {v:?}");
        assert!(matches!(v, Verdict::Warn { .. }));
    }

    #[test]
    fn majority_fail_0p_3f_with_warns_is_fail() {
        // 0 pass + 3 fail (+ warns) → 100% of decisive = fail → Fail.
        let v = Verdict::merge(
            vec![fail("a"), fail("b"), fail("c"), warn_v()],
            VerdictPolicy::MajorityFail,
        );
        assert!(v.is_blocking(), "expected Fail, got {v:?}");
    }

    // ── Unknown policy round-trip via config ──────────────────────────────────

    #[test]
    fn unknown_policy_rejected_at_config_parse() {
        use crate::config::ConfigV1;
        let toml = r#"
            version = 1
            [gate]
            agents = ["claude_code"]
            policy = "made_up"
        "#;
        let err = ConfigV1::parse(toml).expect_err("unknown policy should fail at parse");
        // The error wraps a toml parse error — just confirm it didn't parse OK.
        use crate::error::KlaspError;
        assert!(
            matches!(err, KlaspError::ConfigParse(_)),
            "expected ConfigParse, got {err:?}"
        );
    }

    #[test]
    fn all_three_policy_values_parse_in_config() {
        use crate::config::ConfigV1;
        for policy_str in &["any_fail", "all_fail", "majority_fail"] {
            let toml = format!(
                r#"
                version = 1
                [gate]
                agents = ["claude_code"]
                policy = "{policy_str}"
                "#
            );
            let cfg = ConfigV1::parse(&toml)
                .unwrap_or_else(|e| panic!("policy '{policy_str}' should parse, got: {e:?}"));
            let expected = match *policy_str {
                "any_fail" => VerdictPolicy::AnyFail,
                "all_fail" => VerdictPolicy::AllFail,
                "majority_fail" => VerdictPolicy::MajorityFail,
                _ => unreachable!(),
            };
            assert_eq!(cfg.gate.policy, expected, "policy '{policy_str}' mismatch");
        }
    }
}
