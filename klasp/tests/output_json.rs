//! Integration tests for `klasp gate --format json` (KLASP_OUTPUT_SCHEMA = 1, issue #45).
//!
//! Golden-fixture tests compare serialised JSON to on-disk files under
//! `tests/fixtures/output_json/`. A unified diff is printed on mismatch.

use klasp::output::json;
use klasp_core::{
    CheckResult, Finding, Severity, Verdict, VerdictPolicy, GATE_SCHEMA_VERSION,
    KLASP_OUTPUT_SCHEMA,
};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_check_result(name: &str, source: &str, verdict: Verdict) -> CheckResult {
    CheckResult {
        check_name: name.into(),
        source_id: source.into(),
        verdict,
        raw_stdout: None,
        raw_stderr: None,
    }
}

/// Load a golden fixture and return its contents.
///
/// Goldens contain the placeholder `__GATE_SCHEMA_VERSION__` so the
/// `--format json` output schema (v1) is decoupled from `GATE_SCHEMA_VERSION`
/// bumps. The placeholder is substituted with the current `GATE_SCHEMA_VERSION`
/// before comparison so a future bump (e.g. 2 → 3) doesn't require re-baking
/// every golden file.
fn load_golden(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/output_json/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("golden fixture missing: {path}"))
        .replace("\r\n", "\n")
        .replace("__GATE_SCHEMA_VERSION__", &GATE_SCHEMA_VERSION.to_string())
}

/// Assert `actual` matches the golden fixture, printing a diff on mismatch.
fn assert_matches_golden(actual: &str, golden_name: &str) {
    let actual_norm = actual.replace("\r\n", "\n");
    let expected = load_golden(golden_name);
    if actual_norm != expected {
        eprintln!("--- expected ({golden_name}) ---");
        eprintln!("{expected}");
        eprintln!("--- actual ---");
        eprintln!("{actual_norm}");
        // Print a simple line-diff.
        let a_lines: Vec<&str> = expected.lines().collect();
        let b_lines: Vec<&str> = actual_norm.lines().collect();
        let max = a_lines.len().max(b_lines.len());
        for i in 0..max {
            let a = a_lines.get(i).copied().unwrap_or("<missing>");
            let b = b_lines.get(i).copied().unwrap_or("<missing>");
            if a != b {
                eprintln!("line {}: expected {:?}", i + 1, a);
                eprintln!("line {}: actual   {:?}", i + 1, b);
            }
        }
        panic!("JSON output does not match golden fixture: {golden_name}");
    }
}

// ── 1. Pass verdict with no checks matches golden ─────────────────────────────

#[test]
fn format_json_pass_no_findings_matches_golden() {
    let json = json::render(&Verdict::Pass, VerdictPolicy::AnyFail, &[]);
    assert_matches_golden(&json, "gate-pass-empty.json");
}

// ── 2. Fail verdict with findings matches golden ──────────────────────────────

#[test]
fn format_json_fail_with_findings_matches_golden() {
    let results = vec![make_check_result(
        "rustfmt",
        "shell",
        Verdict::Fail {
            findings: vec![Finding {
                rule: "fmt".into(),
                message: "not formatted".into(),
                file: Some("src/lib.rs".into()),
                line: Some(10),
                severity: Severity::Error,
            }],
            message: "1 check failed".into(),
        },
    )];
    let json = json::render(
        &Verdict::Fail {
            findings: vec![],
            message: "1 check failed".into(),
        },
        VerdictPolicy::AnyFail,
        &results,
    );
    assert_matches_golden(&json, "gate-fail-with-findings.json");
}

// ── 3. Warn mixed verdict matches golden ─────────────────────────────────────

#[test]
fn format_json_warn_mixed_matches_golden() {
    let results = vec![
        make_check_result("lint", "shell", Verdict::Pass),
        make_check_result(
            "security",
            "shell",
            Verdict::Warn {
                findings: vec![Finding {
                    rule: "dep-outdated".into(),
                    message: "dependency is outdated".into(),
                    file: None,
                    line: None,
                    severity: Severity::Warn,
                }],
                message: None,
            },
        ),
    ];
    let json = json::render(
        &Verdict::Warn {
            findings: vec![],
            message: None,
        },
        VerdictPolicy::AnyFail,
        &results,
    );
    assert_matches_golden(&json, "gate-warn-mixed.json");
}

// ── 4. Every JSON output declares output_schema_version = 1 ──────────────────

#[test]
fn format_json_includes_output_schema_version() {
    let json = json::render(&Verdict::Pass, VerdictPolicy::AnyFail, &[]);
    let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
    assert_eq!(
        v["output_schema_version"],
        serde_json::json!(KLASP_OUTPUT_SCHEMA),
        "output_schema_version must equal KLASP_OUTPUT_SCHEMA"
    );
    assert_eq!(
        v["output_schema_version"], 1,
        "KLASP_OUTPUT_SCHEMA must be 1 in v0.3"
    );
}

// ── 5. Stats totals match the checks array length ────────────────────────────

#[test]
fn format_json_includes_stats() {
    let results = vec![
        make_check_result("a", "shell", Verdict::Pass),
        make_check_result("b", "shell", Verdict::Pass),
        make_check_result(
            "c",
            "shell",
            Verdict::Fail {
                findings: vec![],
                message: "fail".into(),
            },
        ),
    ];
    let merged = Verdict::Fail {
        findings: vec![],
        message: "fail".into(),
    };
    let json = json::render(&merged, VerdictPolicy::AnyFail, &results);
    let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
    assert_eq!(v["stats"]["total_checks"], 3, "total_checks must be 3");
    assert_eq!(v["stats"]["pass"], 2, "pass must be 2");
    assert_eq!(v["stats"]["warn"], 0, "warn must be 0");
    assert_eq!(v["stats"]["fail"], 1, "fail must be 1");
}

// ── 6. Field ordering is stable (catches accidental reordering) ───────────────

#[test]
fn format_json_field_order_stable() {
    // The golden fixture is the source of truth for field order.
    // This test asserts that the raw serialised text — not just the parsed
    // value — matches the golden. assert_matches_golden uses string comparison
    // so any reordering will produce a diff.
    let json = json::render(&Verdict::Pass, VerdictPolicy::AnyFail, &[]);
    assert_matches_golden(&json, "gate-pass-empty.json");

    // Also verify that the top-level key order matches the documented spec:
    // output_schema_version, gate_schema_version, verdict, checks, stats.
    let lines: Vec<&str> = json.lines().collect();
    let key_lines: Vec<&str> = lines
        .iter()
        .filter(|l| l.trim_start().starts_with('"'))
        .copied()
        .collect();
    let keys: Vec<&str> = key_lines
        .iter()
        .filter_map(|l| {
            let trimmed = l.trim();
            if trimmed.starts_with('"') {
                trimmed.split('"').nth(1)
            } else {
                None
            }
        })
        .collect();
    // First five non-nested keys must match the documented spec order.
    let top_level_keys: Vec<&str> = keys.iter().take(5).copied().collect();
    assert_eq!(
        top_level_keys,
        vec![
            "output_schema_version",
            "gate_schema_version",
            "verdict",
            "checks",
            "stats"
        ],
        "top-level field order must match KLASP_OUTPUT_SCHEMA = 1 spec"
    );
}
