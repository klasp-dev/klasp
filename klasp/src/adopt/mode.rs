//! Helpers for adoption modes, specifically for the `chain` mode which is
//! not supported in v1 of this feature.
//!
//! See klasp-dev/klasp#97.

use crate::adopt::plan::AdoptionPlan;

/// Returns a multi-line message explaining that `--mode chain` is not yet
/// supported in v1, along with per-gate manual integration instructions.
///
/// The output is suitable for printing to stderr followed by `exit(2)`.
pub fn chain_unsupported_message(plan: &AdoptionPlan) -> String {
    let mut out = String::from(
        "chain mode is not supported in v1; use --mode mirror to write klasp.toml that \
         runs alongside existing gates, or follow the manual integration instructions below.\n",
    );

    let instructions: Vec<&str> = plan
        .findings
        .iter()
        .filter_map(|g| g.manual_chain_instructions.as_deref())
        .collect();

    if !instructions.is_empty() {
        out.push('\n');
        out.push_str("Manual integration instructions:\n");
        for instruction in instructions {
            out.push_str("  ");
            out.push_str(instruction);
            out.push('\n');
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::adopt::plan::{
        AdoptionPlan, ChainSupport, DetectedGate, GateType, HookStage, ProposedCheck,
        ProposedCheckSource, TriggerKind,
    };

    fn gate_with_instructions(instructions: &str) -> DetectedGate {
        DetectedGate {
            gate_type: GateType::Husky {
                hook: HookStage::PreCommit,
            },
            source_path: PathBuf::from(".husky/pre-commit"),
            proposed_checks: vec![ProposedCheck {
                name: "husky-pre-commit".to_string(),
                triggers: vec![TriggerKind::Commit],
                timeout_secs: 120,
                source: ProposedCheckSource::Shell {
                    command: "pnpm exec lint-staged".to_string(),
                },
            }],
            chain_support: ChainSupport::ManualOnly,
            manual_chain_instructions: Some(instructions.to_string()),
            warnings: vec![],
        }
    }

    fn gate_without_instructions() -> DetectedGate {
        DetectedGate {
            gate_type: GateType::PreCommitFramework,
            source_path: PathBuf::from(".pre-commit-config.yaml"),
            proposed_checks: vec![ProposedCheck {
                name: "pre-commit".to_string(),
                triggers: vec![TriggerKind::Commit],
                timeout_secs: 120,
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

    #[test]
    fn contains_rejection_sentence() {
        let plan = AdoptionPlan::default();
        let msg = chain_unsupported_message(&plan);
        assert!(msg.contains("chain mode is not supported in v1"));
        assert!(msg.contains("--mode mirror"));
    }

    #[test]
    fn empty_plan_has_no_manual_section() {
        let plan = AdoptionPlan::default();
        let msg = chain_unsupported_message(&plan);
        assert!(!msg.contains("Manual integration instructions:"));
    }

    #[test]
    fn gates_without_instructions_skipped() {
        let plan = AdoptionPlan {
            findings: vec![gate_without_instructions()],
        };
        let msg = chain_unsupported_message(&plan);
        assert!(!msg.contains("Manual integration instructions:"));
    }

    #[test]
    fn gates_with_instructions_shown() {
        let plan = AdoptionPlan {
            findings: vec![gate_with_instructions(
                "Append `klasp gate --agent codex --trigger commit \"$@\"` to .husky/pre-commit",
            )],
        };
        let msg = chain_unsupported_message(&plan);
        assert!(msg.contains("Manual integration instructions:"));
        assert!(msg.contains("Append `klasp gate"));
    }

    #[test]
    fn none_instructions_skipped_mixed() {
        let plan = AdoptionPlan {
            findings: vec![
                gate_without_instructions(),
                gate_with_instructions("Do X manually"),
                gate_without_instructions(),
            ],
        };
        let msg = chain_unsupported_message(&plan);
        assert!(msg.contains("Manual integration instructions:"));
        assert!(msg.contains("Do X manually"));
        // Exactly one instructions section
        assert_eq!(msg.matches("Manual integration instructions:").count(), 1);
    }
}
