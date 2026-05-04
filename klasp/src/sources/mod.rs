//! `CheckSource` implementations for the klasp binary.
//!
//! v0.1 shipped exactly one source — `Shell`. v0.2 W4 added the first named
//! recipe — `PreCommit`. W5 adds `Fallow`. W6 will add `pytest` and `cargo`
//! along the same shape. v0.3 will add the subprocess plugin model. Per
//! [docs/design.md §3.2], every new source is an additive change.
//!
//! A `SourceRegistry` is the dispatch table the gate runtime uses to find
//! the right source for a `CheckConfig`. The registry is a fixed `Vec`
//! pre-populated with the built-in sources; the v0.3 plugin model
//! will append discovered subprocess plugins to the same vec.

pub mod fallow;
pub mod pre_commit;
pub mod shell;

use klasp_core::{CheckConfig, CheckSource};

/// Registry of known `CheckSource` impls. Cheap to construct, cheap to
/// query — v0.1's `Vec` linear scan is O(n) over a single-digit number of
/// sources, no need for a `HashMap` until v0.3.
pub struct SourceRegistry {
    sources: Vec<Box<dyn CheckSource>>,
}

impl SourceRegistry {
    /// Build the default registry. Order doesn't affect correctness
    /// (each source's `supports_config` claims a disjoint subset of
    /// `CheckSourceConfig` variants), but more-specific recipes go
    /// before more-general ones so future variants that overlap with
    /// `Shell` (none today, but the v0.3 plugin model could add them)
    /// don't get short-circuited by the catch-all.
    pub fn default_v1() -> Self {
        let sources: Vec<Box<dyn CheckSource>> = vec![
            Box::new(pre_commit::PreCommitSource::new()),
            Box::new(fallow::FallowSource::new()),
            Box::new(shell::ShellSource::new()),
        ];
        Self { sources }
    }

    /// Find the first source that claims to support the given check config.
    /// Returns `None` if no source matches — the gate runtime treats that as
    /// a fail-open skip with a stderr notice (see
    /// [docs/design.md §6] step 6).
    pub fn find_for(&self, check: &CheckConfig) -> Option<&dyn CheckSource> {
        self.sources
            .iter()
            .find(|s| s.supports_config(check))
            .map(|b| b.as_ref())
    }
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self::default_v1()
    }
}

#[cfg(test)]
mod tests {
    use klasp_core::{CheckConfig, CheckSourceConfig};

    use super::*;

    fn shell_check() -> CheckConfig {
        CheckConfig {
            name: "demo".into(),
            triggers: vec![],
            source: CheckSourceConfig::Shell {
                command: "true".into(),
            },
            timeout_secs: None,
        }
    }

    fn pre_commit_check() -> CheckConfig {
        CheckConfig {
            name: "lint".into(),
            triggers: vec![],
            source: CheckSourceConfig::PreCommit {
                hook_stage: None,
                config_path: None,
            },
            timeout_secs: None,
        }
    }

    fn fallow_check() -> CheckConfig {
        CheckConfig {
            name: "audit".into(),
            triggers: vec![],
            source: CheckSourceConfig::Fallow {
                config_path: None,
                base: None,
            },
            timeout_secs: None,
        }
    }

    #[test]
    fn registry_dispatches_shell_check_to_shell_source() {
        let registry = SourceRegistry::default_v1();
        let source = registry
            .find_for(&shell_check())
            .expect("shell source must claim shell config");
        assert_eq!(source.source_id(), "shell");
    }

    #[test]
    fn registry_dispatches_pre_commit_check_to_pre_commit_source() {
        let registry = SourceRegistry::default_v1();
        let source = registry
            .find_for(&pre_commit_check())
            .expect("pre_commit source must claim pre_commit config");
        assert_eq!(source.source_id(), "pre_commit");
    }

    #[test]
    fn registry_dispatches_fallow_check_to_fallow_source() {
        let registry = SourceRegistry::default_v1();
        let source = registry
            .find_for(&fallow_check())
            .expect("fallow source must claim fallow config");
        assert_eq!(source.source_id(), "fallow");
    }
}
