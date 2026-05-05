//! SARIF 2.1.0 JSON formatter for `klasp gate`.

use klasp_core::{Finding, Severity, Verdict, VerdictPolicy};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn render(verdict: &Verdict, _policy: VerdictPolicy) -> String {
    let findings: &[Finding] = match verdict {
        Verdict::Pass => &[],
        Verdict::Warn { findings, .. } => findings.as_slice(),
        Verdict::Fail { findings, .. } => findings.as_slice(),
    };
    let rules = build_rules(findings);
    let results = build_results(findings);
    let sarif = json!({
        "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "klasp",
                    "version": VERSION,
                    "informationUri": "https://klasp.dev",
                    "rules": rules,
                }
            },
            "results": results,
        }]
    });
    let mut output = serde_json::to_string_pretty(&sarif).expect("serde_json serialisation failed");
    output.push('\n');
    output
}

fn build_rules(findings: &[Finding]) -> Value {
    let mut seen: BTreeMap<&str, &Finding> = BTreeMap::new();
    for f in findings {
        seen.entry(f.rule.as_str()).or_insert(f);
    }
    let rules: Vec<Value> = seen
        .values()
        .map(|f| {
            json!({
                "id": f.rule, "name": f.rule, "shortDescription": { "text": f.rule },
            })
        })
        .collect();
    json!(rules)
}

fn build_results(findings: &[Finding]) -> Value {
    json!(findings.iter().map(finding_to_result).collect::<Vec<_>>())
}

fn finding_to_result(f: &Finding) -> Value {
    let mut result = Map::new();
    result.insert("ruleId".into(), json!(f.rule));
    result.insert("level".into(), json!(severity_to_level(f.severity)));
    result.insert("message".into(), json!({"text": f.message}));
    // SARIF 2.1.0 §3.27.12: if `result.locations` is present it MUST be
    // non-empty. Omit the field when the finding has no physical location.
    if let Some(physical) = physical_location(f) {
        result.insert("locations".into(), json!([{"physicalLocation": physical}]));
    }
    Value::Object(result)
}

fn physical_location(f: &Finding) -> Option<Value> {
    match (f.file.as_deref(), f.line) {
        (Some(file), Some(line)) => Some(json!({
            "artifactLocation": {"uri": file},
            "region": {"startLine": line},
        })),
        (Some(file), None) => Some(json!({
            "artifactLocation": {"uri": file},
        })),
        _ => None,
    }
}

fn severity_to_level(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warn => "warning",
        Severity::Info => "note",
    }
}
