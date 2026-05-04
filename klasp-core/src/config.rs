//! `klasp.toml` config — `version = 1` schema.
//!
//! Design: [docs/design.md §3.5]. The `version` field is enforced at parse
//! time so v2 configs reject loudly with an upgrade message rather than
//! silently dropping unknown sections. `CheckSourceConfig` is
//! `#[serde(tag = "type")]`-tagged so unknown source types also fail at
//! parse time.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{KlaspError, Result};
use crate::verdict::VerdictPolicy;

/// Config schema version. Bumps only when the TOML syntax breaks; new
/// optional fields do not bump it.
pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigV1 {
    /// Schema version. Must equal [`CONFIG_VERSION`]; mismatches fail with
    /// [`KlaspError::ConfigVersion`].
    pub version: u32,

    pub gate: GateConfig,

    #[serde(default)]
    pub checks: Vec<CheckConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GateConfig {
    #[serde(default)]
    pub agents: Vec<String>,

    #[serde(default)]
    pub policy: VerdictPolicy,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckConfig {
    pub name: String,

    #[serde(default)]
    pub triggers: Vec<TriggerConfig>,

    pub source: CheckSourceConfig,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerConfig {
    pub on: Vec<String>,
}

/// Tagged enum: TOML `type = "shell"` selects the `Shell` variant,
/// `type = "pre_commit"` selects the v0.2 `PreCommit` named recipe.
/// Unknown `type` values fail at parse time — that's the v0.1 contract
/// for additive forwards-incompatibility, preserved as new recipes land.
///
/// **Adding new variants is the v0.2 named-recipe extension point** —
/// each new recipe (`fallow`, `pytest`, `cargo`, …) is a sibling
/// variant here plus a paired `CheckSource` impl in the binary crate.
/// Field shape is per-recipe: `Shell` carries a free-form `command`,
/// while `PreCommit` carries optional `hook_stage` / `config_path`
/// fields that map to pre-commit's own CLI flags. `verdict_path` is
/// deferred — see [docs/design.md §14] for the explicit scope note.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckSourceConfig {
    Shell {
        command: String,
    },
    PreCommit {
        /// Maps to `pre-commit run --hook-stage <stage>`. `None` defaults
        /// to `"pre-commit"` at run time, matching pre-commit's own
        /// default when invoked from a `.git/hooks/pre-commit` shim.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hook_stage: Option<String>,

        /// Maps to `pre-commit run -c <config_path>`. `None` lets
        /// pre-commit fall back to its own default discovery
        /// (`.pre-commit-config.yaml` at the repo root).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_path: Option<PathBuf>,
    },
}

impl ConfigV1 {
    /// Resolve and load `klasp.toml`. The lookup order matches design §14:
    /// `$CLAUDE_PROJECT_DIR` first (set by Claude Code), then the supplied
    /// `repo_root`. The first existing file wins; any parse error
    /// short-circuits.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let mut searched = Vec::new();

        if let Ok(claude_dir) = std::env::var("CLAUDE_PROJECT_DIR") {
            let candidate = PathBuf::from(claude_dir).join("klasp.toml");
            if candidate.is_file() {
                return Self::from_file(&candidate);
            }
            searched.push(candidate);
        }

        let candidate = repo_root.join("klasp.toml");
        if candidate.is_file() {
            return Self::from_file(&candidate);
        }
        searched.push(candidate);

        Err(KlaspError::ConfigNotFound { searched })
    }

    /// Read and parse a specific TOML file. Public so tests and callers
    /// that already know the path can skip the lookup logic.
    pub fn from_file(path: &Path) -> Result<Self> {
        let bytes = std::fs::read_to_string(path).map_err(|source| KlaspError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&bytes)
    }

    /// Parse from raw TOML. Validates the `version` field as part of the
    /// parse step so caller code never sees a malformed `ConfigV1`.
    pub fn parse(s: &str) -> Result<Self> {
        let config: ConfigV1 = toml::from_str(s)?;
        if config.version != CONFIG_VERSION {
            return Err(KlaspError::ConfigVersion {
                found: config.version,
                supported: CONFIG_VERSION,
            });
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let toml = r#"
            version = 1

            [gate]
            agents = ["claude_code"]
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        assert_eq!(config.version, 1);
        assert_eq!(config.gate.agents, vec!["claude_code"]);
        assert_eq!(config.gate.policy, VerdictPolicy::AnyFail);
        assert!(config.checks.is_empty());
    }

    #[test]
    fn parses_full_config() {
        let toml = r#"
            version = 1

            [gate]
            agents = ["claude_code"]
            policy = "any_fail"

            [[checks]]
            name = "ruff"
            triggers = [{ on = ["commit"] }]
            timeout_secs = 60
            [checks.source]
            type = "shell"
            command = "ruff check ."

            [[checks]]
            name = "pytest"
            triggers = [{ on = ["push"] }]
            [checks.source]
            type = "shell"
            command = "pytest -q"
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        assert_eq!(config.checks.len(), 2);
        assert_eq!(config.checks[0].name, "ruff");
        assert_eq!(config.checks[0].timeout_secs, Some(60));
        assert!(matches!(
            &config.checks[0].source,
            CheckSourceConfig::Shell { command } if command == "ruff check ."
        ));
        assert_eq!(config.checks[0].triggers[0].on, vec!["commit"]);
        assert!(config.checks[1].timeout_secs.is_none());
    }

    #[test]
    fn rejects_wrong_version() {
        let toml = r#"
            version = 2
            [gate]
        "#;
        let err = ConfigV1::parse(toml).expect_err("should reject");
        match err {
            KlaspError::ConfigVersion { found, supported } => {
                assert_eq!(found, 2);
                assert_eq!(supported, CONFIG_VERSION);
            }
            other => panic!("expected ConfigVersion, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_version() {
        let toml = r#"
            [gate]
            agents = []
        "#;
        let err = ConfigV1::parse(toml).expect_err("should reject");
        assert!(matches!(err, KlaspError::ConfigParse(_)));
    }

    #[test]
    fn rejects_missing_gate() {
        let toml = "version = 1";
        let err = ConfigV1::parse(toml).expect_err("should reject");
        assert!(matches!(err, KlaspError::ConfigParse(_)));
    }

    #[test]
    fn rejects_unknown_source_type() {
        // `pre_commit` was an unknown recipe in v0.1; now in v0.2 W4 it's a
        // first-class variant. This test pivots to a recipe that hasn't
        // landed yet (`fallow`, ETA W5) so the additive-forwards-incompat
        // contract keeps its regression coverage.
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "future-recipe"
            [checks.source]
            type = "fallow"
            base = "main"
        "#;
        let err = ConfigV1::parse(toml).expect_err("should reject");
        assert!(matches!(err, KlaspError::ConfigParse(_)));
    }

    #[test]
    fn rejects_unknown_field_on_pre_commit_variant() {
        // A typo like `hook_stages` (plural) on the `pre_commit` variant
        // would silently parse as the default `None` without
        // `#[serde(deny_unknown_fields)]` on the tagged enum — defaulting
        // to `--hook-stage pre-commit` regardless of the user's intent.
        // Locks in the variant-level footgun closure so a future serde
        // refactor (e.g. `untagged`) doesn't silently regress it.
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "typo-test"
            [checks.source]
            type = "pre_commit"
            hook_stages = "pre-push"
        "#;
        let err = ConfigV1::parse(toml).expect_err("should reject");
        assert!(matches!(err, KlaspError::ConfigParse(_)));
    }

    #[test]
    fn parses_pre_commit_recipe_minimal() {
        // Bare `type = "pre_commit"` with no extra fields: both optional
        // fields default to `None` and the recipe applies its own
        // run-time defaults (`hook_stage = "pre-commit"`,
        // `config_path = ".pre-commit-config.yaml"`).
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "lint"
            [checks.source]
            type = "pre_commit"
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        assert_eq!(config.checks.len(), 1);
        match &config.checks[0].source {
            CheckSourceConfig::PreCommit {
                hook_stage,
                config_path,
            } => {
                assert!(hook_stage.is_none());
                assert!(config_path.is_none());
            }
            other => panic!("expected PreCommit, got {other:?}"),
        }
    }

    #[test]
    fn parses_pre_commit_recipe_with_fields() {
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "lint"
            [checks.source]
            type = "pre_commit"
            hook_stage = "pre-push"
            config_path = "tools/pre-commit.yaml"
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        match &config.checks[0].source {
            CheckSourceConfig::PreCommit {
                hook_stage,
                config_path,
            } => {
                assert_eq!(hook_stage.as_deref(), Some("pre-push"));
                assert_eq!(
                    config_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    Some("tools/pre-commit.yaml".to_string())
                );
            }
            other => panic!("expected PreCommit, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_check_name() {
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            [checks.source]
            type = "shell"
            command = "echo"
        "#;
        let err = ConfigV1::parse(toml).expect_err("should reject");
        assert!(matches!(err, KlaspError::ConfigParse(_)));
    }
}
