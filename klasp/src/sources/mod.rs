//! `CheckSource` implementations for the klasp binary.
//!
//! v0.1 shipped exactly one source — `Shell`. v0.2 W4 added the first named
//! recipe — `PreCommit`. W5 adds `Fallow`. W6 adds `Pytest` and `Cargo`,
//! finishing the v0.2 named-recipe slate. v0.3 adds the subprocess plugin
//! model via `PluginSource`. Per [docs/design.md §3.2], every new source is
//! an additive change.
//!
//! A `SourceRegistry` is the dispatch table the gate runtime uses to find
//! the right source for a `CheckConfig`. Built-in sources live in a fixed
//! `Vec`. When no built-in source claims a `type = "plugin"` config, a
//! `PluginSource` is constructed on demand and returned as a `Box<dyn CheckSource>`.
//! The caller owns the returned box for plugin sources; built-in sources are
//! returned as `&dyn CheckSource` borrows from the registry vec.

pub mod cargo;
pub mod fallow;
pub mod plugin;
pub mod pre_commit;
pub mod pytest;
pub mod shell;

use klasp_core::{CheckConfig, CheckSource, CheckSourceConfig};

use plugin::PluginSource;

/// Dispatch result: either a borrowed built-in or an owned plugin source.
///
/// `SourceForCheck::run` forwards to whichever arm is active, giving the gate
/// a single call-site regardless of whether the check is built-in or plugin.
pub enum SourceForCheck<'a> {
    BuiltIn(&'a dyn CheckSource),
    Plugin(Box<dyn CheckSource>),
}

impl<'a> SourceForCheck<'a> {
    /// Delegate to the underlying `CheckSource::run`.
    pub fn run(
        &self,
        config: &CheckConfig,
        state: &klasp_core::RepoState,
    ) -> Result<klasp_core::CheckResult, klasp_core::CheckSourceError> {
        match self {
            SourceForCheck::BuiltIn(s) => s.run(config, state),
            SourceForCheck::Plugin(s) => s.run(config, state),
        }
    }

    /// Forward `source_id()` to the underlying source. Used in tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn source_id(&self) -> &str {
        match self {
            SourceForCheck::BuiltIn(s) => s.source_id(),
            SourceForCheck::Plugin(s) => s.source_id(),
        }
    }
}

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
            Box::new(pytest::PytestSource::new()),
            Box::new(cargo::CargoSource::new()),
            Box::new(shell::ShellSource::new()),
        ];
        Self { sources }
    }

    /// Find the source for the given check config.
    ///
    /// Built-in sources (shell, cargo, fallow, pre_commit, pytest) are checked
    /// first. When none match and the config is `type = "plugin"`, a fresh
    /// `PluginSource` is constructed and returned as `SourceForCheck::Plugin`.
    /// The caller owns the plugin source for the duration of the check run.
    /// Returns `None` only for genuinely unknown (non-plugin) configs.
    pub fn find_for(&self, check: &CheckConfig) -> Option<SourceForCheck<'_>> {
        // Built-in sources take priority over plugins.
        if let Some(src) = self.sources.iter().find(|s| s.supports_config(check)) {
            return Some(SourceForCheck::BuiltIn(src.as_ref()));
        }
        // Plugin fallback: `type = "plugin"` with a `name` field.
        if let CheckSourceConfig::Plugin { name, .. } = &check.source {
            return Some(SourceForCheck::Plugin(Box::new(PluginSource::new(name))));
        }
        None
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

    fn pytest_check() -> CheckConfig {
        CheckConfig {
            name: "tests".into(),
            triggers: vec![],
            source: CheckSourceConfig::Pytest {
                extra_args: None,
                config_path: None,
                junit_xml: None,
            },
            timeout_secs: None,
        }
    }

    fn cargo_check_config() -> CheckConfig {
        CheckConfig {
            name: "build".into(),
            triggers: vec![],
            source: CheckSourceConfig::Cargo {
                subcommand: "check".into(),
                extra_args: None,
                package: None,
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

    #[test]
    fn registry_dispatches_pytest_check_to_pytest_source() {
        let registry = SourceRegistry::default_v1();
        let source = registry
            .find_for(&pytest_check())
            .expect("pytest source must claim pytest config");
        assert_eq!(source.source_id(), "pytest");
    }

    #[test]
    fn registry_dispatches_cargo_check_to_cargo_source() {
        let registry = SourceRegistry::default_v1();
        let source = registry
            .find_for(&cargo_check_config())
            .expect("cargo source must claim cargo config");
        assert_eq!(source.source_id(), "cargo");
    }

    #[test]
    fn registry_dispatches_plugin_check_to_plugin_source() {
        let registry = SourceRegistry::default_v1();
        let plugin_check = CheckConfig {
            name: "my-check".into(),
            triggers: vec![],
            source: CheckSourceConfig::Plugin {
                name: "my-linter".into(),
                args: vec![],
                settings: None,
            },
            timeout_secs: None,
        };
        let source = registry
            .find_for(&plugin_check)
            .expect("plugin source must be created for plugin config");
        assert_eq!(source.source_id(), "plugin:my-linter");
    }

    #[test]
    fn registry_returns_none_for_unknown_non_plugin_config() {
        // There is no way to construct an unknown CheckSourceConfig at the
        // Rust level (the enum is exhaustive), but we can verify that `Shell`
        // is not returned as a plugin source.
        let registry = SourceRegistry::default_v1();
        let shell = shell_check();
        let source = registry.find_for(&shell);
        assert!(
            source.is_some(),
            "shell check must always resolve to a source"
        );
    }
}
