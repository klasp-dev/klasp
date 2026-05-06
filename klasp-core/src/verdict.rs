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
#[path = "verdict_tests.rs"]
mod tests;
