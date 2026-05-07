//! Stable JSON output formatter for `klasp gate --format json`.
//!
//! Implements `KLASP_OUTPUT_SCHEMA = 1` as documented in `docs/output-schema.md`.
//! The schema is stable from v0.3 forward: additions are allowed within v0.3.x,
//! but removals and renames require a schema version bump.

use klasp_core::{
    CheckResult, Finding, Severity, Verdict, VerdictPolicy, GATE_SCHEMA_VERSION,
    KLASP_OUTPUT_SCHEMA,
};
use serde_json::{json, Map, Value};

/// Render the gate output as a stable JSON document (KLASP_OUTPUT_SCHEMA = 1).
///
/// `check_results` carries the per-check structured data. When empty (e.g. when
/// no checks ran), the `checks` array is empty and `stats` counts are all zero.
pub fn render(verdict: &Verdict, _policy: VerdictPolicy, check_results: &[CheckResult]) -> String {
    let verdict_str = verdict_to_str(verdict);
    let checks_arr = build_checks(check_results);
    let stats = build_stats(check_results);

    // Field order is load-bearing for the golden-fixture test — keep stable.
    let mut doc = Map::new();
    doc.insert("output_schema_version".into(), json!(KLASP_OUTPUT_SCHEMA));
    doc.insert("gate_schema_version".into(), json!(GATE_SCHEMA_VERSION));
    doc.insert("verdict".into(), json!(verdict_str));
    doc.insert("checks".into(), Value::Array(checks_arr));
    doc.insert("stats".into(), stats);

    let mut out =
        serde_json::to_string_pretty(&Value::Object(doc)).expect("serde_json serialisation failed");
    out.push('\n');
    out
}

fn verdict_to_str(v: &Verdict) -> &'static str {
    match v {
        Verdict::Pass => "pass",
        Verdict::Warn { .. } => "warn",
        Verdict::Fail { .. } => "fail",
    }
}

fn build_checks(results: &[CheckResult]) -> Vec<Value> {
    results.iter().map(check_result_to_value).collect()
}

fn check_result_to_value(r: &CheckResult) -> Value {
    let verdict_str = verdict_to_str(&r.verdict);
    let findings_arr = match &r.verdict {
        Verdict::Pass => vec![],
        Verdict::Warn { findings, .. } => findings.iter().map(finding_to_value).collect(),
        Verdict::Fail { findings, .. } => findings.iter().map(finding_to_value).collect(),
    };

    // Field order matches docs/output-schema.md — keep stable.
    let mut obj = Map::new();
    obj.insert("name".into(), json!(r.check_name));
    obj.insert("source".into(), json!(r.source_id));
    obj.insert("verdict".into(), json!(verdict_str));
    obj.insert("findings".into(), Value::Array(findings_arr));
    Value::Object(obj)
}

fn finding_to_value(f: &Finding) -> Value {
    // Field order matches docs/output-schema.md — keep stable.
    let mut obj = Map::new();
    obj.insert("severity".into(), json!(severity_to_str(f.severity)));
    obj.insert("rule".into(), json!(f.rule));
    obj.insert(
        "file".into(),
        match &f.file {
            Some(p) => json!(p),
            None => Value::Null,
        },
    );
    obj.insert(
        "line".into(),
        match f.line {
            Some(n) => json!(n),
            None => Value::Null,
        },
    );
    obj.insert("message".into(), json!(f.message));
    Value::Object(obj)
}

fn severity_to_str(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warn => "warn",
        Severity::Info => "info",
    }
}

fn build_stats(results: &[CheckResult]) -> Value {
    let total = results.len();
    let pass = results
        .iter()
        .filter(|r| matches!(r.verdict, Verdict::Pass))
        .count();
    let warn = results
        .iter()
        .filter(|r| matches!(r.verdict, Verdict::Warn { .. }))
        .count();
    let fail = results
        .iter()
        .filter(|r| matches!(r.verdict, Verdict::Fail { .. }))
        .count();

    // Field order matches docs/output-schema.md — keep stable.
    let mut obj = Map::new();
    obj.insert("total_checks".into(), json!(total));
    obj.insert("pass".into(), json!(pass));
    obj.insert("warn".into(), json!(warn));
    obj.insert("fail".into(), json!(fail));
    Value::Object(obj)
}
