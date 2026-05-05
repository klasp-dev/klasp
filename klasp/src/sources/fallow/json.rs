//! JSON helpers for the fallow recipe — finding extraction and summary.
//!
//! Lifted out of the sibling `super` module to keep `fallow.rs` under
//! the project's 500-line cap. The split is along the natural seam:
//! everything in here walks a `serde_json::Value` and returns
//! [`klasp_core::Finding`]s or summary strings; nothing here spawns
//! subprocesses or touches the filesystem.

use klasp_core::{Finding, Severity};
use serde_json::Value;

use super::{finding, MAX_FINDINGS};

/// Categories fallow reports as per-finding rows under its `dead_code`
/// block. Keys not present at the JSON level are silently skipped.
const DEAD_CODE_KEYS: &[&str] = &[
    "unused_files",
    "unused_exports",
    "unused_types",
    "unused_dependencies",
    "unused_dev_dependencies",
    "unresolved_imports",
    "unlisted_dependencies",
    "duplicate_exports",
    "circular_dependencies",
    "boundary_violations",
];

/// Walk fallow's audit JSON and emit a structured finding per actionable
/// row. Order: complexity → dead-code → duplication. Capped at
/// [`MAX_FINDINGS`] so a wall of dead-code rows from a fresh repo scan
/// doesn't drown the agent's stderr.
pub(super) fn collect_findings(check_name: &str, json: &Value) -> Vec<Finding> {
    let mut out: Vec<Finding> = Vec::new();
    let mut push_from = |arr: Option<&Vec<Value>>, build: &dyn Fn(&Value) -> Option<Finding>| {
        if let Some(arr) = arr {
            for entry in arr {
                if out.len() >= MAX_FINDINGS {
                    return;
                }
                if let Some(f) = build(entry) {
                    out.push(f);
                }
            }
        }
    };

    push_from(
        json.get("complexity")
            .and_then(|c| c.get("findings"))
            .and_then(Value::as_array),
        &|e| complexity_finding(check_name, e),
    );

    if let Some(dead) = json.get("dead_code") {
        for key in DEAD_CODE_KEYS {
            push_from(dead.get(key).and_then(Value::as_array), &|e| {
                dead_code_finding(check_name, key, e)
            });
        }
    }

    push_from(
        json.get("duplication")
            .and_then(|d| d.get("clone_groups"))
            .and_then(Value::as_array),
        &|e| duplication_finding(check_name, e),
    );

    out
}

fn complexity_finding(check_name: &str, entry: &Value) -> Option<Finding> {
    let str_field = |k: &str| entry.get(k).and_then(Value::as_str);
    let u64_field = |k: &str| entry.get(k).and_then(Value::as_u64);
    let name = str_field("name").unwrap_or("?");
    let line = u64_field("line").map(|n| n as u32);
    let mut detail = format!("complexity: function `{name}`");
    match (u64_field("cyclomatic"), u64_field("cognitive")) {
        (Some(c), Some(g)) => detail.push_str(&format!(" (cyclomatic={c}, cognitive={g})")),
        (Some(c), None) => detail.push_str(&format!(" (cyclomatic={c})")),
        _ => {}
    }
    Some(finding(
        check_name,
        "complexity",
        &detail,
        str_field("path").map(str::to_string),
        line,
        severity_from(str_field("severity").unwrap_or(""), Severity::Warn),
    ))
}

fn dead_code_finding(check_name: &str, kind: &str, entry: &Value) -> Option<Finding> {
    let str_field = |k: &str| entry.get(k).and_then(Value::as_str);
    let path = str_field("path");
    let line = entry.get("line").and_then(Value::as_u64).map(|n| n as u32);
    // fallow uses different label keys per dead-code category; check
    // each in priority order so the rendered detail says "unused
    // export `foo`" rather than the bare category.
    let label = ["export_name", "type_name", "dependency", "name"]
        .iter()
        .find_map(|k| str_field(k));
    let kind_pretty = kind.replace('_', " ");
    let detail = match (label, path) {
        (Some(name), _) => format!("{kind_pretty}: `{name}`"),
        (None, Some(p)) => format!("{kind_pretty}: {p}"),
        (None, None) => kind_pretty.clone(),
    };
    Some(finding(
        check_name,
        kind,
        &detail,
        path.map(str::to_string),
        line,
        Severity::Error,
    ))
}

fn duplication_finding(check_name: &str, entry: &Value) -> Option<Finding> {
    // Clone groups carry an array of sites; surface the first as the
    // finding's location and report the per-group counts in the detail.
    let sites = entry.get("sites").and_then(Value::as_array)?;
    let first = sites.first()?;
    let path = first.get("path").and_then(Value::as_str);
    let line = first
        .get("start_line")
        .and_then(Value::as_u64)
        .map(|n| n as u32);
    let lines = entry
        .get("lines")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let detail = format!("duplication: {} clones across {lines} lines", sites.len());
    Some(finding(
        check_name,
        "duplication",
        &detail,
        path.map(str::to_string),
        line,
        Severity::Error,
    ))
}

fn severity_from(raw: &str, fallback: Severity) -> Severity {
    match raw {
        "critical" | "high" | "error" => Severity::Error,
        "moderate" | "medium" | "warn" | "warning" => Severity::Warn,
        "low" | "info" => Severity::Info,
        _ => fallback,
    }
}

/// One-line summary for the verdict's `message`. `None` when fallow's
/// `summary` reports zero findings everywhere. `verdict_str = "warn"`
/// reads "warned" rather than "failed" so warn-level outcomes don't
/// carry error-framed prose.
pub(super) fn summarise(json: &Value, verdict_str: &str) -> Option<String> {
    let count = |key: &str| -> u64 {
        json.get("summary")
            .and_then(|s| s.get(key))
            .and_then(Value::as_u64)
            .unwrap_or_default()
    };
    let dead = count("dead_code_issues");
    let complexity = count("complexity_findings");
    let dupes = count("duplication_clone_groups");
    if dead + complexity + dupes == 0 {
        return None;
    }
    let action = if verdict_str == "warn" {
        "warned"
    } else {
        "failed"
    };
    Some(format!(
        "fallow audit {action} ({dead} dead-code, {complexity} complexity, {dupes} duplication)"
    ))
}
