//! Invoke `pre-commit run` and translate output into plugin findings.
//!
//! This mirrors the logic in `klasp/src/sources/pre_commit.rs` but uses
//! `std::process::Command` directly (no klasp-core dependency). The key
//! behaviour differences from a built-in recipe:
//!
//! - No per-finding file/line: pre-commit doesn't emit per-line info reliably
//!   in its summary output. Callers see `file = None, line = None`.
//! - Findings are capped at `MAX_FINDINGS` to stay within klasp's 16 MiB
//!   output cap. A sentinel finding is appended when truncation occurs.
//! - Missing binary → `warn` verdict rather than an error, matching the
//!   protocol's "infrastructure error = warn, never fail" rule.

use which::which;

use crate::protocol::{
    PluginFinding, PluginGateInput, PluginGateOutput, PluginTriggerKind, PluginVerdict,
    PROTOCOL_VERSION,
};

/// Rule slug for infrastructure errors emitted by this plugin.
const BINARY_MISSING_RULE: &str = "klasp-plugin-pre-commit/binary-missing";
/// Rule slug for unknown-protocol warnings emitted by this plugin.
const PROTOCOL_WARN_RULE: &str = "klasp-plugin-pre-commit/protocol-warn";
/// Rule slug for all pre-commit hook findings.
const HOOK_RULE_PREFIX: &str = "pre-commit/";
/// Maximum number of findings to emit before truncating.
const MAX_FINDINGS: usize = 100;

/// Run the gate check and produce a `PluginGateOutput`.
///
/// Never panics; all errors are captured as warn-level findings so the plugin
/// always exits 0 with well-formed JSON.
pub fn run_gate(input: &PluginGateInput) -> PluginGateOutput {
    // Warn if the caller is on a newer protocol version but continue best-effort.
    let protocol_warn = if input.protocol_version != PROTOCOL_VERSION {
        Some(warn_finding(
            PROTOCOL_WARN_RULE,
            &format!(
                "received protocol_version={} but this plugin speaks v{}; \
                 proceeding best-effort — update the plugin when klasp v1.0 ships",
                input.protocol_version, PROTOCOL_VERSION
            ),
        ))
    } else {
        None
    };

    // Locate the pre-commit binary.
    if which("pre-commit").is_err() {
        let mut findings = protocol_warn.into_iter().collect::<Vec<_>>();
        findings.push(warn_finding(
            BINARY_MISSING_RULE,
            "pre-commit binary not found on $PATH; install via `pipx install pre-commit`",
        ));
        return PluginGateOutput {
            protocol_version: PROTOCOL_VERSION,
            verdict: PluginVerdict::Warn,
            findings,
        };
    }

    let output = invoke_pre_commit(input);
    let mut findings = build_findings(&output, &input.trigger.kind);

    // Prepend any protocol warning so it's visible at the top.
    if let Some(pw) = protocol_warn {
        findings.insert(0, pw);
    }

    let verdict = if findings.iter().any(|f| f.severity == "error") {
        PluginVerdict::Fail
    } else if findings.is_empty() {
        PluginVerdict::Pass
    } else {
        PluginVerdict::Warn
    };

    PluginGateOutput {
        protocol_version: PROTOCOL_VERSION,
        verdict,
        findings,
    }
}

/// Result of invoking the pre-commit subprocess.
struct PreCommitOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Invoke `pre-commit run` with the appropriate flags for the trigger kind.
fn invoke_pre_commit(input: &PluginGateInput) -> PreCommitOutput {
    use std::process::Command;

    let repo_root = std::path::Path::new(&input.repo_root);

    let mut cmd = Command::new("pre-commit");
    cmd.arg("run").arg("--hook-stage").arg("pre-commit");

    if input.trigger.kind == PluginTriggerKind::Push {
        cmd.arg("--from-ref")
            .arg(&input.base_ref)
            .arg("--to-ref")
            .arg("HEAD");
    }

    if !input.trigger.files.is_empty() && input.trigger.kind == PluginTriggerKind::Commit {
        cmd.arg("--files");
        for f in &input.trigger.files {
            cmd.arg(f);
        }
    }

    cmd.current_dir(repo_root);

    match cmd.output() {
        Ok(out) => PreCommitOutput {
            exit_code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        Err(e) => PreCommitOutput {
            exit_code: None,
            stdout: String::new(),
            stderr: format!("failed to spawn pre-commit: {e}"),
        },
    }
}

/// Convert pre-commit output into plugin findings, capped at `MAX_FINDINGS`.
fn build_findings(output: &PreCommitOutput, _kind: &PluginTriggerKind) -> Vec<PluginFinding> {
    match output.exit_code {
        Some(0) => vec![],
        Some(1) => {
            let mut findings = parse_failed_hooks(&output.stdout);
            if findings.is_empty() {
                let detail = if output.stderr.trim().is_empty() {
                    "pre-commit reported failures (no parseable hook output)".to_string()
                } else {
                    format!("pre-commit reported failures: {}", output.stderr.trim())
                };
                findings.push(error_finding(
                    &format!("{HOOK_RULE_PREFIX}unknown"),
                    &detail,
                ));
            }
            truncate_findings(findings)
        }
        Some(code) => {
            let detail = if output.stderr.trim().is_empty() {
                format!(
                    "pre-commit exited with unexpected status {code}; \
                     this usually means a tooling error inside pre-commit itself"
                )
            } else {
                format!(
                    "pre-commit exited with unexpected status {code}: {}",
                    output.stderr.trim()
                )
            };
            vec![error_finding(
                &format!("{HOOK_RULE_PREFIX}exit-{code}"),
                &detail,
            )]
        }
        None => vec![error_finding(
            &format!("{HOOK_RULE_PREFIX}terminated"),
            "pre-commit was terminated before producing an exit code",
        )],
    }
}

/// Parse `"<hook>....Failed"` lines from pre-commit stdout.
/// Format is stable from pre-commit 3.0 through 4.x.
fn parse_failed_hooks(stdout: &str) -> Vec<PluginFinding> {
    stdout
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            let head = line.strip_suffix("Failed")?;
            let head = head.trim_end_matches(|c: char| c == '.' || c.is_whitespace());
            if head.is_empty() {
                return None;
            }
            // Normalize hook name: lowercase, spaces → hyphens
            let hook_id = head
                .to_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join("-");
            Some(error_finding(
                &format!("{HOOK_RULE_PREFIX}{hook_id}"),
                &format!("hook `{head}` failed"),
            ))
        })
        .collect()
}

/// Cap findings at `MAX_FINDINGS` and append a sentinel if truncated.
fn truncate_findings(mut findings: Vec<PluginFinding>) -> Vec<PluginFinding> {
    if findings.len() <= MAX_FINDINGS {
        return findings;
    }
    let overflow = findings.len() - MAX_FINDINGS;
    findings.truncate(MAX_FINDINGS);
    findings.push(warn_finding(
        "klasp-plugin-pre-commit/truncated",
        &format!(
            "{overflow} additional finding{} not shown (truncated at {MAX_FINDINGS})",
            if overflow == 1 { "" } else { "s" }
        ),
    ));
    findings
}

fn error_finding(rule: &str, message: &str) -> PluginFinding {
    PluginFinding {
        severity: "error".to_string(),
        rule: rule.to_string(),
        file: None,
        line: None,
        message: message.to_string(),
    }
}

fn warn_finding(rule: &str, message: &str) -> PluginFinding {
    PluginFinding {
        severity: "warn".to_string(),
        rule: rule.to_string(),
        file: None,
        line: None,
        message: message.to_string(),
    }
}
