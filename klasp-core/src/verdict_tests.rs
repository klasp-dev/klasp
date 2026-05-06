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
