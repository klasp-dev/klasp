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
use crate::trigger_config::{validate_user_triggers, UserTrigger, UserTriggerConfig};
use crate::verdict::VerdictPolicy;

/// Config schema version. Bumps only when the TOML syntax breaks; new
/// optional fields do not bump it.
pub const CONFIG_VERSION: u32 = 1;

/// Env var Claude Code sets to the project root it was launched from.
/// Centralised so [`ConfigV1::load`] and the runtime's repo-root resolver
/// can't drift on the spelling.
pub const CLAUDE_PROJECT_DIR_ENV: &str = "CLAUDE_PROJECT_DIR";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigV1 {
    /// Schema version. Must equal [`CONFIG_VERSION`]; mismatches fail with
    /// [`KlaspError::ConfigVersion`].
    pub version: u32,

    pub gate: GateConfig,

    #[serde(default)]
    pub checks: Vec<CheckConfig>,

    /// User-defined `[[trigger]]` blocks. These extend (not replace) the
    /// built-in commit/push regex. Validated eagerly on parse via
    /// [`UserTriggerConfig`] → [`UserTrigger`] compilation.
    #[serde(default, rename = "trigger")]
    pub triggers: Vec<UserTriggerConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GateConfig {
    #[serde(default)]
    pub agents: Vec<String>,

    #[serde(default)]
    pub policy: VerdictPolicy,

    /// Run checks in parallel via rayon's work-stealing scheduler. v0.2.5+
    /// behaviour. Default `false` for back-compat. Per [docs/design.md §6.1],
    /// checks MUST be stateless when this is enabled — anything writing to a
    /// shared temp file or process-global state will race.
    #[serde(default)]
    pub parallel: bool,
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
/// `type = "pre_commit"` selects the v0.2 W4 `PreCommit` named recipe,
/// `type = "fallow"` selects the v0.2 W5 `Fallow` named recipe,
/// `type = "pytest"` selects the v0.2 W6 `Pytest` named recipe,
/// `type = "cargo"` selects the v0.2 W6 `Cargo` named recipe.
/// `type = "plugin"` selects the v0.3 subprocess plugin model — the plugin
/// binary is identified by the required `name` field (e.g. `name = "my-linter"`
/// maps to the `klasp-plugin-my-linter` binary on `$PATH`).
///
/// Unknown `type` values (other than the above) fail at parse time — that's
/// the v0.1 contract for additive forwards-incompatibility, preserved as
/// new recipes land.
///
/// **Adding new variants is the v0.2 named-recipe extension point** —
/// each new recipe is a sibling variant here plus a paired `CheckSource`
/// impl in the binary crate. Field shape is per-recipe: `Shell` carries
/// a free-form `command`, `PreCommit` carries optional `hook_stage` /
/// `config_path` fields, `Fallow` carries optional `config_path` /
/// `base` fields, `Pytest` carries optional `extra_args`, `config_path`,
/// and `junit_xml` toggle, `Cargo` requires a `subcommand` plus optional
/// `extra_args` / `package`. `verdict_path` is deferred — see
/// [docs/design.md §14] for the explicit scope note.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckSourceConfig {
    Shell {
        command: String,
    },
    /// v0.3 subprocess plugin. The plugin binary `klasp-plugin-<name>` is
    /// discovered lazily on `$PATH` when the gate encounters this config.
    /// `args` is an optional list of extra arguments passed to the plugin on
    /// every `--gate` invocation; `settings` is an optional opaque JSON object
    /// forwarded verbatim inside `PluginGateInput.config.settings` so the
    /// plugin can consume arbitrary config without klasp knowing its schema.
    Plugin {
        /// Name of the plugin binary to invoke. `name = "my-linter"` resolves
        /// to `klasp-plugin-my-linter` on `$PATH`.
        name: String,
        /// Optional extra arguments forwarded to the plugin on every `--gate`
        /// invocation. Plugins receive these inside `PluginGateInput`.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        /// Optional opaque config block forwarded verbatim to the plugin inside
        /// `PluginGateInput.config.settings`. Plugins may define any schema
        /// here; klasp treats it as a JSON blob.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        settings: Option<serde_json::Value>,
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
    Fallow {
        /// Maps to `fallow audit -c <config_path>`. `None` lets fallow
        /// fall back to its own discovery (`.fallowrc.json`,
        /// `.fallowrc.jsonc`, or `fallow.toml` at the repo root).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_path: Option<PathBuf>,

        /// Maps to `fallow audit --base <ref>`. `None` falls back to
        /// `${KLASP_BASE_REF}` at run time, which the gate runtime
        /// resolves to the merge-base of `HEAD` against the upstream
        /// tracking branch. Set this only when the diff-base for the
        /// audit should diverge from the gate's resolved base ref —
        /// e.g. auditing against a fixed mainline for a long-lived
        /// release branch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
    },
    Pytest {
        /// Free-form extra args appended after pytest's own flags.
        /// e.g. `"-x -q tests/integration"`. `None` runs pytest with
        /// its own defaults.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra_args: Option<String>,

        /// Maps to `pytest -c <config_path>`. `None` lets pytest fall
        /// back to its own discovery (`pytest.ini`, `pyproject.toml`,
        /// `tox.ini`, …).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_path: Option<PathBuf>,

        /// When `true`, the recipe asks pytest to write a JUnit XML
        /// report and parses it for per-failure findings. When `false`
        /// (default), the recipe falls back to a generic count-based
        /// finding from pytest's exit code alone.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        junit_xml: Option<bool>,
    },
    Cargo {
        /// Required: which `cargo <subcommand>` to dispatch. Accepted
        /// values are `"check"`, `"clippy"`, `"test"`, `"build"`. Any
        /// other value fails at run time with an unparseable detail
        /// (the schema doesn't enum-restrict this so a future cargo
        /// subcommand can be tried by an adventurous user without a
        /// klasp release).
        subcommand: String,

        /// Free-form extra args appended after cargo's own flags
        /// (e.g. `"--all-features"`). `None` runs cargo with its
        /// own defaults.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extra_args: Option<String>,

        /// Maps to `cargo <sub> -p <package>`. `None` runs across
        /// the workspace via `--workspace`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        package: Option<String>,
    },
}

/// Walk up from `start` to `repo_root` looking for `klasp.toml`.
///
/// Lookup order:
/// 1. Canonicalize both `start` and `repo_root` (resolves macOS symlinks).
/// 2. If `start` is a file, begin from its parent directory.
/// 3. Walk upward, checking for `klasp.toml` at each level, stopping at
///    `repo_root` inclusive.
/// 4. Return `None` if no config found or `start` is outside `repo_root`.
pub fn discover_config_for_path(start: &Path, repo_root: &Path) -> Option<PathBuf> {
    let root = repo_root.canonicalize().ok()?;
    let start_dir = if start.is_file() {
        start.parent().map(Path::to_path_buf)?
    } else {
        start.to_path_buf()
    };
    let start_canon = start_dir.canonicalize().ok()?;
    if !start_canon.starts_with(&root) {
        return None;
    }
    let mut current = start_canon;
    loop {
        let candidate = current.join("klasp.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if current == root {
            break;
        }
        match current.parent() {
            Some(p) => current = p.to_path_buf(),
            None => break,
        }
    }
    None
}

/// Convenience wrapper: discover nearest `klasp.toml` then load it.
///
/// Returns `None` if no config file is found in the walk-up chain.
/// Returns `Some(Err(_))` if a config file exists but fails to parse.
pub fn load_config_for_path(start: &Path, repo_root: &Path) -> Option<Result<(PathBuf, ConfigV1)>> {
    let config_path = discover_config_for_path(start, repo_root)?;
    Some(ConfigV1::from_file(&config_path).map(|cfg| (config_path, cfg)))
}

/// True when the process cwd resolves under `root`. Both paths are
/// canonicalised so symlinked layouts (`/var` → `/private/var` on macOS,
/// worktrees, etc.) compare correctly. Any failure to canonicalise either
/// side is treated as "not inside" — the caller falls back to its
/// alternative resolution path.
fn cwd_inside(root: &Path) -> bool {
    let cwd = match std::env::current_dir().and_then(|c| c.canonicalize()) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let root = match root.canonicalize() {
        Ok(r) => r,
        Err(_) => return false,
    };
    cwd.starts_with(root)
}

impl ConfigV1 {
    /// Resolve and load `klasp.toml`. Lookup order per design §14:
    /// `$CLAUDE_PROJECT_DIR` first (set by Claude Code), then the supplied
    /// `repo_root`. The first existing file wins; any parse error
    /// short-circuits.
    ///
    /// The `$CLAUDE_PROJECT_DIR` candidate is only honoured when the process
    /// cwd is inside that directory — otherwise a session bound to repo A
    /// would run A's gate against an unrelated sibling repo B. On mismatch
    /// the env candidate is skipped and resolution falls through to
    /// `repo_root`; if neither exists, [`KlaspError::ConfigNotFound`] is
    /// returned and the gate fails open.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let mut searched = Vec::new();

        if let Ok(claude_dir) = std::env::var(CLAUDE_PROJECT_DIR_ENV) {
            let env_root = PathBuf::from(claude_dir);
            let candidate = env_root.join("klasp.toml");
            match (candidate.is_file(), cwd_inside(&env_root)) {
                (true, true) => return Self::from_file(&candidate),
                // env candidate exists but cwd is elsewhere — skip silently;
                // the file isn't missing, it's just not ours to load.
                (true, false) => {}
                (false, _) => searched.push(candidate),
            }
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

    /// Parse from raw TOML. Validates the `version` field and eagerly compiles
    /// all `[[trigger]]` regexes so caller code never sees a malformed `ConfigV1`.
    pub fn parse(s: &str) -> Result<Self> {
        let config: ConfigV1 = toml::from_str(s)?;
        if config.version != CONFIG_VERSION {
            return Err(KlaspError::ConfigVersion {
                found: config.version,
                supported: CONFIG_VERSION,
            });
        }
        // Eagerly validate all user triggers — bad regexes are config errors.
        validate_user_triggers(&config.triggers)?;
        Ok(config)
    }

    /// Compile and return user triggers as validated [`UserTrigger`] objects.
    ///
    /// This is infallible post-parse because [`Self::parse`] already validated
    /// them. Callers that hold a parsed `ConfigV1` may call this freely.
    pub fn compiled_triggers(&self) -> Vec<UserTrigger> {
        validate_user_triggers(&self.triggers).expect("triggers already validated at parse time")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_TOML: &str = r#"
        version = 1
        [gate]
        agents = ["claude_code"]
    "#;

    fn write_klasp_toml(dir: &std::path::Path) {
        std::fs::write(dir.join("klasp.toml"), MINIMAL_TOML).expect("write klasp.toml");
    }

    /// All `load()` cases live in one `#[test]` because both `CLAUDE_PROJECT_DIR`
    /// and cwd are process-global; running them in parallel under cargo's
    /// default test harness would clobber each other.
    #[test]
    fn load_cwd_guard_cases() {
        struct Guard {
            cwd: std::path::PathBuf,
            env: Option<String>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                match &self.env {
                    Some(v) => std::env::set_var(CLAUDE_PROJECT_DIR_ENV, v),
                    None => std::env::remove_var(CLAUDE_PROJECT_DIR_ENV),
                }
                let _ = std::env::set_current_dir(&self.cwd);
            }
        }
        let _guard = Guard {
            cwd: std::env::current_dir().expect("current_dir"),
            env: std::env::var(CLAUDE_PROJECT_DIR_ENV).ok(),
        };

        // Case 1: cwd inside env_root with env candidate present → uses env candidate.
        {
            let env_root = tempfile::tempdir().expect("tempdir env_root");
            let sub = env_root.path().join("sub");
            std::fs::create_dir_all(&sub).expect("mkdir sub");
            write_klasp_toml(env_root.path());

            std::env::set_var(CLAUDE_PROJECT_DIR_ENV, env_root.path());
            std::env::set_current_dir(&sub).expect("cd sub");

            let cfg = ConfigV1::load(env_root.path()).expect("case 1: should load");
            assert_eq!(cfg.version, 1, "case 1: version mismatch");
        }

        // Case 2: cwd outside env_root, cwd_root candidate present → uses cwd_root.
        {
            let env_root = tempfile::tempdir().expect("tempdir env_root");
            let cwd_root = tempfile::tempdir().expect("tempdir cwd_root");
            write_klasp_toml(env_root.path());
            write_klasp_toml(cwd_root.path());

            std::env::set_var(CLAUDE_PROJECT_DIR_ENV, env_root.path());
            std::env::set_current_dir(cwd_root.path()).expect("cd cwd_root");

            let cfg = ConfigV1::load(cwd_root.path()).expect("case 2: should load");
            assert_eq!(cfg.version, 1, "case 2: version mismatch");
        }

        // Case 3: cwd outside env_root, no cwd_root candidate → ConfigNotFound.
        {
            let env_root = tempfile::tempdir().expect("tempdir env_root");
            let cwd_root = tempfile::tempdir().expect("tempdir cwd_root");
            write_klasp_toml(env_root.path());

            std::env::set_var(CLAUDE_PROJECT_DIR_ENV, env_root.path());
            std::env::set_current_dir(cwd_root.path()).expect("cd cwd_root");

            let err =
                ConfigV1::load(cwd_root.path()).expect_err("case 3: should be ConfigNotFound");
            assert!(
                matches!(err, KlaspError::ConfigNotFound { .. }),
                "case 3: expected ConfigNotFound, got {err:?}"
            );
        }

        // Case 4: env var points at a non-existent path → falls through to cwd_root.
        {
            let cwd_root = tempfile::tempdir().expect("tempdir cwd_root");
            write_klasp_toml(cwd_root.path());
            let bogus = cwd_root.path().join("does-not-exist");

            std::env::set_var(CLAUDE_PROJECT_DIR_ENV, &bogus);
            std::env::set_current_dir(cwd_root.path()).expect("cd cwd_root");

            let cfg = ConfigV1::load(cwd_root.path()).expect("case 4: should load cwd candidate");
            assert_eq!(cfg.version, 1, "case 4: version mismatch");
        }

        // Case 5: env var unset → uses cwd_root candidate (regression check on the
        // pre-#65 happy path; distinct branch from case 2).
        {
            let cwd_root = tempfile::tempdir().expect("tempdir cwd_root");
            write_klasp_toml(cwd_root.path());

            std::env::remove_var(CLAUDE_PROJECT_DIR_ENV);
            std::env::set_current_dir(cwd_root.path()).expect("cd cwd_root");

            let cfg = ConfigV1::load(cwd_root.path()).expect("case 5: should load cwd candidate");
            assert_eq!(cfg.version, 1, "case 5: version mismatch");
        }
    }

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
        // `pre_commit` was an unknown recipe in v0.1, `fallow` was unknown
        // in v0.2 W4, `pytest` / `cargo` were unknown in W5; all are
        // first-class variants now. Pivot to a recipe that hasn't landed
        // yet (placeholder for whichever recipe lands next post-W6) so
        // the additive-forwards-incompat contract keeps its regression
        // coverage.
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "future-recipe"
            [checks.source]
            type = "future_recipe_not_yet_landed"
            command = "noop"
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
    fn parses_fallow_recipe_minimal() {
        // Bare `type = "fallow"` with no extra fields: both optional
        // fields default to `None` and the recipe applies its own
        // run-time defaults (`base = "${KLASP_BASE_REF}"`,
        // `config_path = <fallow's own discovery>`).
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "audit"
            [checks.source]
            type = "fallow"
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        assert_eq!(config.checks.len(), 1);
        match &config.checks[0].source {
            CheckSourceConfig::Fallow { config_path, base } => {
                assert!(config_path.is_none());
                assert!(base.is_none());
            }
            other => panic!("expected Fallow, got {other:?}"),
        }
    }

    #[test]
    fn parses_fallow_recipe_with_fields() {
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "audit"
            [checks.source]
            type = "fallow"
            config_path = "tools/.fallowrc.json"
            base = "origin/main"
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        match &config.checks[0].source {
            CheckSourceConfig::Fallow { config_path, base } => {
                assert_eq!(
                    config_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    Some("tools/.fallowrc.json".to_string())
                );
                assert_eq!(base.as_deref(), Some("origin/main"));
            }
            other => panic!("expected Fallow, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_field_on_fallow_variant() {
        // Same footgun closure as the pre_commit variant: a typo like
        // `bases` (plural) on the `fallow` variant must fail at parse
        // time rather than silently default to `None`.
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "audit"
            [checks.source]
            type = "fallow"
            bases = "main"
        "#;
        let err = ConfigV1::parse(toml).expect_err("should reject");
        assert!(matches!(err, KlaspError::ConfigParse(_)));
    }

    #[test]
    fn parses_pytest_recipe_minimal() {
        // Bare `type = "pytest"` with no extra fields: every optional
        // field defaults to `None` and the recipe applies its own
        // run-time defaults.
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "tests"
            [checks.source]
            type = "pytest"
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        assert_eq!(config.checks.len(), 1);
        match &config.checks[0].source {
            CheckSourceConfig::Pytest {
                extra_args,
                config_path,
                junit_xml,
            } => {
                assert!(extra_args.is_none());
                assert!(config_path.is_none());
                assert!(junit_xml.is_none());
            }
            other => panic!("expected Pytest, got {other:?}"),
        }
    }

    #[test]
    fn parses_pytest_recipe_with_fields() {
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "tests"
            [checks.source]
            type = "pytest"
            extra_args = "-x -q tests/"
            config_path = "pytest.ini"
            junit_xml = true
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        match &config.checks[0].source {
            CheckSourceConfig::Pytest {
                extra_args,
                config_path,
                junit_xml,
            } => {
                assert_eq!(extra_args.as_deref(), Some("-x -q tests/"));
                assert_eq!(
                    config_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    Some("pytest.ini".to_string())
                );
                assert_eq!(*junit_xml, Some(true));
            }
            other => panic!("expected Pytest, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_field_on_pytest_variant() {
        // Same footgun closure as the pre_commit / fallow variants: a
        // typo on the `pytest` variant must fail at parse time.
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "tests"
            [checks.source]
            type = "pytest"
            extra_arg = "-x"
        "#;
        let err = ConfigV1::parse(toml).expect_err("should reject");
        assert!(matches!(err, KlaspError::ConfigParse(_)));
    }

    #[test]
    fn parses_cargo_recipe_minimal() {
        // `type = "cargo"` requires `subcommand`; the other fields
        // default to `None` and the recipe runs across the workspace.
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "build"
            [checks.source]
            type = "cargo"
            subcommand = "check"
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        assert_eq!(config.checks.len(), 1);
        match &config.checks[0].source {
            CheckSourceConfig::Cargo {
                subcommand,
                extra_args,
                package,
            } => {
                assert_eq!(subcommand, "check");
                assert!(extra_args.is_none());
                assert!(package.is_none());
            }
            other => panic!("expected Cargo, got {other:?}"),
        }
    }

    #[test]
    fn parses_cargo_recipe_with_fields() {
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "lint"
            [checks.source]
            type = "cargo"
            subcommand = "clippy"
            extra_args = "--all-features -- -D warnings"
            package = "klasp-core"
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        match &config.checks[0].source {
            CheckSourceConfig::Cargo {
                subcommand,
                extra_args,
                package,
            } => {
                assert_eq!(subcommand, "clippy");
                assert_eq!(extra_args.as_deref(), Some("--all-features -- -D warnings"));
                assert_eq!(package.as_deref(), Some("klasp-core"));
            }
            other => panic!("expected Cargo, got {other:?}"),
        }
    }

    #[test]
    fn rejects_cargo_recipe_missing_subcommand() {
        // `subcommand` is required (no `#[serde(default)]`), so a
        // bare `type = "cargo"` must fail at parse time.
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "build"
            [checks.source]
            type = "cargo"
        "#;
        let err = ConfigV1::parse(toml).expect_err("should reject");
        assert!(matches!(err, KlaspError::ConfigParse(_)));
    }

    #[test]
    fn rejects_unknown_field_on_cargo_variant() {
        let toml = r#"
            version = 1
            [gate]

            [[checks]]
            name = "build"
            [checks.source]
            type = "cargo"
            subcommand = "check"
            packages = "klasp-core"
        "#;
        let err = ConfigV1::parse(toml).expect_err("should reject");
        assert!(matches!(err, KlaspError::ConfigParse(_)));
    }

    // ── GateConfig.parallel field tests ─────────────────────────────────────

    #[test]
    fn parallel_field_defaults_to_false_when_omitted() {
        let toml = r#"
            version = 1
            [gate]
            agents = ["claude_code"]
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        assert!(!config.gate.parallel, "parallel should default to false");
    }

    #[test]
    fn parallel_field_parses_true() {
        let toml = r#"
            version = 1
            [gate]
            agents = ["claude_code"]
            parallel = true
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        assert!(config.gate.parallel, "parallel = true should parse");
    }

    #[test]
    fn parallel_field_parses_explicit_false() {
        let toml = r#"
            version = 1
            [gate]
            agents = ["claude_code"]
            parallel = false
        "#;
        let config = ConfigV1::parse(toml).expect("should parse");
        assert!(!config.gate.parallel, "parallel = false should parse");
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

    // ── discover_config_for_path unit tests ─────────────────────────────────

    #[test]
    fn discover_returns_none_for_path_outside_repo_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("klasp.toml"), MINIMAL_TOML).unwrap();

        let outside = tmp.path().join("other");
        std::fs::create_dir_all(&outside).unwrap();
        assert!(discover_config_for_path(&outside, &repo).is_none());
    }

    #[test]
    fn discover_finds_config_at_repo_root_for_deep_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        let deep = repo.join("a").join("b").join("c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(repo.join("klasp.toml"), MINIMAL_TOML).unwrap();

        let found = discover_config_for_path(&deep, &repo).unwrap();
        assert_eq!(found, repo.canonicalize().unwrap().join("klasp.toml"));
    }

    #[test]
    fn discover_prefers_nearest_config_over_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        let pkg = repo.join("packages").join("web");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(repo.join("klasp.toml"), MINIMAL_TOML).unwrap();
        std::fs::write(pkg.join("klasp.toml"), MINIMAL_TOML).unwrap();

        let found = discover_config_for_path(&pkg, &repo).unwrap();
        assert_eq!(found, pkg.canonicalize().unwrap().join("klasp.toml"));
    }

    #[test]
    fn discover_starts_from_parent_when_given_a_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        let pkg = repo.join("packages").join("web");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(repo.join("klasp.toml"), MINIMAL_TOML).unwrap();
        std::fs::write(pkg.join("klasp.toml"), MINIMAL_TOML).unwrap();
        std::fs::write(pkg.join("index.ts"), "").unwrap();

        let found = discover_config_for_path(&pkg.join("index.ts"), &repo).unwrap();
        assert_eq!(found, pkg.canonicalize().unwrap().join("klasp.toml"));
    }

    #[test]
    fn discover_returns_none_when_no_config_in_chain() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        let deep = repo.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        // No klasp.toml anywhere.

        assert!(discover_config_for_path(&deep, &repo).is_none());
    }
}
