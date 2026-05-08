//! Renders an [`AdoptionPlan`] to a human-readable string for `--mode inspect`
//! and `--mode mirror` output.
//!
//! See klasp-dev/klasp#97.

use crate::adopt::plan::{
    AdoptionPlan, ChainSupport, DetectedGate, GateType, ProposedCheckSource,
};

/// Render `plan` to a human-readable string.
///
/// The string always ends with a newline. It is suitable for direct `print!`
/// (not `println!`) to stdout.
pub fn render_plan(plan: &AdoptionPlan) -> String {
    if plan.findings.is_empty() {
        return "No existing gates detected. Run `klasp init` for a fresh klasp.toml.\n"
            .to_string();
    }

    let mut out = String::from("Detected existing gates:\n");
    for gate in &plan.findings {
        out.push('\n');
        out.push_str(&render_gate(gate));
    }
    out.push_str("\nNext:\n");
    out.push_str("  klasp init --adopt --mode mirror\n");
    out.push_str("  klasp install --agent all\n");
    out.push_str("  klasp doctor\n");
    out
}

fn render_gate(gate: &DetectedGate) -> String {
    let label = gate_label(gate);
    let name = gate_human_name(&gate.gate_type);
    let mut out = format!("{label}  {name}\n");
    out.push_str(&format!("    {}\n", gate.source_path.display()));
    out.push_str(&format!("    {}\n", summarise_checks(gate)));
    for warning in &gate.warnings {
        out.push_str(&format!("    {warning}\n"));
    }
    out
}

/// Returns `"WARN"` when the gate has `ChainSupport::Unsafe` or any warnings;
/// otherwise `"OK"`.
fn gate_label(gate: &DetectedGate) -> &'static str {
    if gate.chain_support == ChainSupport::Unsafe || !gate.warnings.is_empty() {
        "WARN"
    } else {
        "OK"
    }
}

fn gate_human_name(gt: &GateType) -> String {
    match gt {
        GateType::PreCommitFramework => "pre-commit framework".to_string(),
        GateType::Husky { hook } => format!("husky {hook}"),
        GateType::Lefthook => "lefthook".to_string(),
        GateType::PlainGitHook { hook } => format!("plain git hook ({hook})"),
        GateType::LintStaged => "lint-staged".to_string(),
        GateType::Tooling(name) => name.clone(),
    }
}

/// Return a one-line summary of the first proposed check, or a fallback.
fn summarise_checks(gate: &DetectedGate) -> String {
    let Some(first) = gate.proposed_checks.first() else {
        return "mirror: (no checks proposed; inspect only)".to_string();
    };
    match &first.source {
        ProposedCheckSource::PreCommit { .. } => {
            format!("mirror: type = \"pre_commit\"")
        }
        ProposedCheckSource::Shell { command } => {
            format!("mirror: command = \"{command}\"")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::adopt::plan::{
        AdoptionPlan, ChainSupport, Confidence, DetectedGate, GateType, ProposedCheck,
        ProposedCheckSource,
    };

    fn pre_commit_gate() -> DetectedGate {
        DetectedGate {
            gate_type: GateType::PreCommitFramework,
            source_path: PathBuf::from(".pre-commit-config.yaml"),
            confidence: Confidence::High,
            proposed_checks: vec![ProposedCheck {
                name: "pre-commit".to_string(),
                triggers: vec!["commit".to_string()],
                timeout_secs: Some(120),
                source: ProposedCheckSource::PreCommit {
                    hook_stage: None,
                    config_path: None,
                },
            }],
            chain_support: ChainSupport::ManualOnly,
            manual_chain_instructions: None,
            warnings: vec![],
        }
    }

    fn plain_hook_gate() -> DetectedGate {
        DetectedGate {
            gate_type: GateType::PlainGitHook {
                hook: "pre-push".to_string(),
            },
            source_path: PathBuf::from(".git/hooks/pre-push"),
            confidence: Confidence::Medium,
            proposed_checks: vec![],
            chain_support: ChainSupport::Unsafe,
            manual_chain_instructions: Some(
                "Append `klasp gate` calls to .git/hooks/pre-push manually.".to_string(),
            ),
            warnings: vec![
                "klasp will not overwrite this hook".to_string(),
                "run with --mode chain to append a managed block, or mirror the command manually"
                    .to_string(),
            ],
        }
    }

    #[test]
    fn empty_plan_renders_no_gates_message() {
        let plan = AdoptionPlan::default();
        let rendered = render_plan(&plan);
        assert!(rendered.contains("No existing gates detected"));
        assert!(!rendered.contains("Next:"));
    }

    #[test]
    fn pre_commit_gate_renders_ok_label() {
        let plan = AdoptionPlan {
            findings: vec![pre_commit_gate()],
        };
        let rendered = render_plan(&plan);
        assert!(rendered.contains("OK  pre-commit framework"));
        assert!(rendered.contains(".pre-commit-config.yaml"));
        assert!(rendered.contains("mirror: type = \"pre_commit\""));
    }

    #[test]
    fn unsafe_chain_gate_renders_warn_label() {
        let plan = AdoptionPlan {
            findings: vec![plain_hook_gate()],
        };
        let rendered = render_plan(&plan);
        assert!(rendered.contains("WARN  plain git hook (pre-push)"));
        assert!(rendered.contains("mirror: (no checks proposed; inspect only)"));
        assert!(rendered.contains("klasp will not overwrite this hook"));
    }

    #[test]
    fn next_block_present_for_non_empty_plan() {
        let plan = AdoptionPlan {
            findings: vec![pre_commit_gate()],
        };
        let rendered = render_plan(&plan);
        assert!(rendered.contains("Next:"));
        assert!(rendered.contains("klasp init --adopt --mode mirror"));
        assert!(rendered.contains("klasp install --agent all"));
        assert!(rendered.contains("klasp doctor"));
    }

    #[test]
    fn shell_source_renders_command() {
        let gate = DetectedGate {
            gate_type: GateType::LintStaged,
            source_path: PathBuf::from("package.json"),
            confidence: Confidence::High,
            proposed_checks: vec![ProposedCheck {
                name: "lint-staged".to_string(),
                triggers: vec!["commit".to_string()],
                timeout_secs: Some(120),
                source: ProposedCheckSource::Shell {
                    command: "pnpm exec lint-staged".to_string(),
                },
            }],
            chain_support: ChainSupport::ManualOnly,
            manual_chain_instructions: None,
            warnings: vec![],
        };
        let plan = AdoptionPlan {
            findings: vec![gate],
        };
        let rendered = render_plan(&plan);
        assert!(rendered.contains("mirror: command = \"pnpm exec lint-staged\""));
    }

    #[test]
    fn multiple_gates_all_appear() {
        let plan = AdoptionPlan {
            findings: vec![pre_commit_gate(), plain_hook_gate()],
        };
        let rendered = render_plan(&plan);
        assert!(rendered.contains("pre-commit framework"));
        assert!(rendered.contains("plain git hook (pre-push)"));
        // Exactly one Next: block
        assert_eq!(rendered.matches("Next:").count(), 1);
    }

    #[test]
    fn warning_on_gate_forces_warn_label() {
        let mut gate = pre_commit_gate();
        gate.warnings.push("duplicate execution risk".to_string());
        // chain_support is ManualOnly (not Unsafe), but warnings push to WARN
        assert_eq!(gate.chain_support, ChainSupport::ManualOnly);
        let plan = AdoptionPlan {
            findings: vec![gate],
        };
        let rendered = render_plan(&plan);
        assert!(rendered.contains("WARN  pre-commit framework"));
        assert!(rendered.contains("duplicate execution risk"));
    }
}
