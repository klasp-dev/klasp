//! Shared data types for the adoption plan produced by detectors and consumed
//! by render, writer, and mode modules.
//!
//! These types form the public API surface that all concurrent agents code
//! against. They must remain stable across the lifetime of this feature.
//! See klasp-dev/klasp#97.

use std::path::PathBuf;

/// The aggregated result of running all gate detectors against a repo.
///
/// Collected by [`super::detect::detect_all`] and then passed to the render
/// and writer modules depending on the chosen [`AdoptMode`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdoptionPlan {
    /// All gate findings produced by the detectors. May be empty when the
    /// repo has no recognised existing gate infrastructure.
    pub findings: Vec<DetectedGate>,
}

/// A single existing gate detected in the repository.
///
/// Each detector returns zero or more `DetectedGate`s. The aggregator
/// collects them all into [`AdoptionPlan::findings`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedGate {
    /// Which kind of hook infrastructure was found.
    pub gate_type: GateType,
    /// The file that triggered detection (config file, hook script, etc.).
    pub source_path: PathBuf,
    /// Proposed `klasp.toml` checks that mirror what the existing gate does.
    pub proposed_checks: Vec<ProposedCheck>,
    /// Whether klasp can safely chain into this gate automatically.
    pub chain_support: ChainSupport,
    /// Human-readable chaining instructions when automatic chaining is not
    /// safe.
    pub manual_chain_instructions: Option<String>,
    /// Non-fatal warnings the user should be aware of (e.g. duplicate
    /// execution risk when both the existing hook and klasp would run the
    /// same tool at commit time).
    pub warnings: Vec<String>,
}

/// The git hook stage (e.g. pre-commit, pre-push).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookStage {
    /// The `pre-commit` git hook.
    PreCommit,
    /// The `pre-push` git hook.
    PrePush,
}

impl HookStage {
    /// Return the canonical git hook name for this stage.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreCommit => "pre-commit",
            Self::PrePush => "pre-push",
        }
    }
}

/// The kind of trigger a proposed check fires on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    /// Fire at commit time (`on = ["commit"]`).
    Commit,
    /// Fire at push time (`on = ["push"]`).
    Push,
}

impl TriggerKind {
    /// Return the TOML-serialisable trigger name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Push => "push",
        }
    }
}

/// The kind of existing gate infrastructure that was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateType {
    /// `.pre-commit-config.yaml` / `.pre-commit-config.yml` present.
    PreCommitFramework,
    /// A Husky hook file (`.husky/<hook>`) was found.
    Husky {
        /// The git hook stage that was detected.
        hook: HookStage,
    },
    /// `lefthook.yml` or `lefthook.yaml` was found.
    Lefthook,
    /// A plain user-owned `.git/hooks/<hook>` script was found that is not
    /// attributed to any other recognised hook manager.
    PlainGitHook {
        /// The git hook stage that was detected.
        hook: HookStage,
    },
    /// `lint-staged` config was found (key in `package.json` or standalone
    /// `.lintstagedrc*` file).
    LintStaged,
    /// Catch-all for tool-specific config hints (e.g. `pyproject.toml` with
    /// ruff config, `Makefile` with a `lint` target). Reserved for future
    /// stack-detection work; no detector emits this variant in v1.
    #[allow(dead_code)]
    Tooling(String),
}

// AutoSafe is reserved for chain-mode v2; not yet constructed
/// Whether klasp can automatically chain into the existing gate at
/// `--mode chain` time, or whether manual steps are required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainSupport {
    /// Chaining requires manual steps; klasp prints instructions.
    ManualOnly,
    /// Chaining is explicitly blocked (would overwrite user content without
    /// a safe uninstall path). `--mode chain` emits an error for this gate.
    Unsafe,
}

/// A single check proposed for inclusion in `klasp.toml`.
///
/// The render module converts these into TOML text; the writer module
/// merges them into an existing or new `klasp.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedCheck {
    /// Value for `[[checks]] name = …`.
    pub name: String,
    /// Values for `triggers = [{ on = […] }]` — list of trigger kinds
    /// such as `Commit` or `Push`.
    pub triggers: Vec<TriggerKind>,
    /// Value for `timeout_secs = …`. Required; the writer never silently
    /// defaults this.
    pub timeout_secs: u64,
    /// The source block (`[checks.source]`) for this check.
    pub source: ProposedCheckSource,
}

/// The source block of a proposed check, mirroring [`klasp_core::CheckSourceConfig`]
/// variants relevant to adoption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposedCheckSource {
    /// `type = "pre_commit"` — uses the built-in pre-commit recipe.
    PreCommit {
        /// Optional `--hook-stage` override.
        hook_stage: Option<String>,
        /// Optional `-c <path>` override.
        config_path: Option<PathBuf>,
    },
    /// `type = "shell"` — free-form shell command.
    Shell {
        /// The shell command to run.
        command: String,
    },
}

/// The adoption mode selected by the user via `--mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptMode {
    /// Print the plan without writing anything.
    Inspect,
    /// Write or update `klasp.toml` to mirror detected gates. Never modifies
    /// existing hook files.
    Mirror,
    /// Where safe, integrate klasp into the existing hook manager. Requires
    /// explicit opt-in; unsupported gates error out.
    Chain,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adoption_plan_default_is_empty() {
        let plan = AdoptionPlan::default();
        assert!(plan.findings.is_empty());
    }

    #[test]
    fn detected_gate_clone_round_trips() {
        let gate = DetectedGate {
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
            manual_chain_instructions: Some("Follow the manual steps.".to_string()),
            warnings: vec!["duplicate execution risk".to_string()],
        };
        let cloned = gate.clone();
        assert_eq!(gate, cloned);
    }

    #[test]
    fn adopt_mode_variants_are_distinct() {
        assert_ne!(AdoptMode::Inspect, AdoptMode::Mirror);
        assert_ne!(AdoptMode::Mirror, AdoptMode::Chain);
        assert_ne!(AdoptMode::Inspect, AdoptMode::Chain);
    }

    #[test]
    fn chain_support_copy_semantics() {
        let cs = ChainSupport::ManualOnly;
        let cs2 = cs;
        assert_eq!(cs, cs2);
    }

    #[test]
    fn trigger_kind_as_str() {
        assert_eq!(TriggerKind::Commit.as_str(), "commit");
        assert_eq!(TriggerKind::Push.as_str(), "push");
    }

    #[test]
    fn hook_stage_as_str() {
        assert_eq!(HookStage::PreCommit.as_str(), "pre-commit");
        assert_eq!(HookStage::PrePush.as_str(), "pre-push");
    }
}
