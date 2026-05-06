//! `klasp doctor` — diagnose the local install.
//!
//! Runs four sequential check groups, prints `OK`/`WARN`/`FAIL`/`INFO`
//! lines on stdout (6-char prefix gutter), prints an aggregate summary
//! to stderr, and exits 0 iff zero `FAIL`. `WARN` is non-fatal. References
//! [docs/design.md §5] and [`klasp_core::AgentSurface`].
//!
//! Check order:
//!   1. **Config** — `klasp.toml` exists and parses as `version = 1`.
//!   2. **Hook script** — for each surface declared in `[gate].agents`,
//!      the file at `surface.hook_path()` is byte-equal to a fresh
//!      `surface.render_hook_script()` at the binary's current
//!      `GATE_SCHEMA_VERSION`. Catches schema drift between binary and
//!      installed hook (the exact case the gate runtime fail-opens on).
//!   3. **Settings** — for each surface declared in `[gate].agents`,
//!      `surface.settings_path()` exists, parses as JSON, and contains
//!      klasp's `PreToolUse[Bash]` hook entry.
//!   4. **PATH** — for each `config.checks[*].source.Shell { command }`,
//!      the leading executable resolves via `which::which`. WARN-only —
//!      missing dev tools shouldn't fail doctor.
//!
//! Surfaces NOT listed in `[gate].agents` are skipped with an INFO line.
//! If `AGENTS.md` is present but `"codex"` is not in `[gate].agents`, an
//! INFO suggestion is emitted (not a FAIL) so the user knows they can
//! opt in with `klasp install --agent codex`.

use std::path::Path;
use std::process::ExitCode;

use klasp_core::{
    AgentSurface, CheckSourceConfig, ConfigV1, InstallContext, KlaspError, GATE_SCHEMA_VERSION,
};
use serde_json::Value;

use crate::cli::DoctorArgs;
use crate::cmd::install::resolve_repo_root;
use crate::registry::SurfaceRegistry;

/// FAIL/WARN counters for the aggregate summary. `INFO` lines do not count.
struct Counters {
    fails: usize,
    warns: usize,
}

impl Counters {
    fn new() -> Self {
        Self { fails: 0, warns: 0 }
    }

    fn ok(&self, msg: &str) {
        println!("OK    {msg}");
    }

    fn warn(&mut self, msg: &str) {
        self.warns += 1;
        println!("WARN  {msg}");
    }

    fn fail(&mut self, msg: &str) {
        self.fails += 1;
        println!("FAIL  {msg}");
    }

    fn info(msg: &str) {
        println!("INFO  {msg}");
    }
}

pub fn run(_args: &DoctorArgs) -> ExitCode {
    let repo_root = match resolve_repo_root(None) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("klasp doctor: {e:#}");
            return ExitCode::FAILURE;
        }
    };

    let mut c = Counters::new();

    let config = check_config(&repo_root, &mut c);
    check_surfaces(&repo_root, config.as_ref(), &mut c);
    if let Some(cfg) = config {
        check_paths(&cfg, &mut c);
    }

    if c.fails > 0 || c.warns > 0 {
        eprintln!("doctor: {} FAIL, {} WARN", c.fails, c.warns);
    } else {
        eprintln!("doctor: all checks passed");
    }

    if c.fails > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Check 1 — load `klasp.toml`. Returns the parsed config so check 4 can
/// iterate `config.checks`. `None` on any load failure (the corresponding
/// FAIL line has already been emitted).
fn check_config(repo_root: &Path, c: &mut Counters) -> Option<ConfigV1> {
    match ConfigV1::load(repo_root) {
        Ok(cfg) => {
            c.ok("config: klasp.toml loaded OK");
            Some(cfg)
        }
        Err(KlaspError::ConfigNotFound { searched }) => {
            let paths: Vec<String> = searched.iter().map(|p| p.display().to_string()).collect();
            c.fail(&format!(
                "config: klasp.toml not found (searched: {})",
                paths.join(", ")
            ));
            None
        }
        Err(KlaspError::ConfigVersion { found, supported }) => {
            c.fail(&format!(
                "config: version mismatch — file declares version = {found}, but this klasp understands version = {supported}"
            ));
            None
        }
        Err(KlaspError::ConfigParse(e)) => {
            c.fail(&format!("config: klasp.toml parse error: {e}"));
            None
        }
        Err(KlaspError::Io { path, source }) => {
            c.fail(&format!(
                "config: I/O error reading {}: {source}",
                path.display()
            ));
            None
        }
        Err(
            e @ (KlaspError::Protocol(_) | KlaspError::Install(_) | KlaspError::CheckSource(_)),
        ) => {
            c.fail(&format!("config: unexpected error: {e}"));
            None
        }
    }
}

/// Checks 2 & 3 — for each surface declared in `[gate].agents`, run hook +
/// settings checks. Surfaces not in the declared list are skipped with an
/// INFO line. If `config` is `None` (config load failed), falls back to the
/// detect-based path so surfaces still produce actionable output.
///
/// Additionally, if `AGENTS.md` is present but `"codex"` is not declared in
/// `[gate].agents`, emit an INFO suggestion (non-fatal) so users learn they
/// can enable codex gate coverage.
fn check_surfaces(repo_root: &Path, config: Option<&ConfigV1>, c: &mut Counters) {
    let registry = SurfaceRegistry::default();
    let mut active = 0usize;

    let declared: Option<&[String]> = config.map(|cfg| cfg.gate.agents.as_slice());

    for surface in registry.iter() {
        let agent_id = surface.agent_id();
        let is_declared = match declared {
            // Config loaded: honour [gate].agents exclusively.
            Some(agents) => agents.iter().any(|a| a == agent_id),
            // Config failed to load: fall back to filesystem detection so
            // the user still gets actionable output for installed surfaces.
            None => surface.detect(repo_root),
        };

        if !is_declared {
            Counters::info(&format!("{agent_id}: not in [gate].agents, skipping"));
            continue;
        }
        active += 1;
        check_hook(repo_root, surface, c);
        check_settings(repo_root, surface, c);
    }

    if active == 0 {
        c.warn(
            "no agent surfaces declared in [gate].agents; run `klasp install --force` if needed",
        );
    }

    // Advisory: AGENTS.md present but codex not declared → INFO suggestion.
    let codex_declared = declared
        .map(|agents| agents.iter().any(|a| a == "codex"))
        .unwrap_or(false);
    if !codex_declared && repo_root.join("AGENTS.md").is_file() {
        Counters::info(
            "AGENTS.md present but \"codex\" not in [gate].agents \
             — run `klasp install --agent codex` to enable codex gate coverage",
        );
    }
}

/// Check 2 — byte-equality of the on-disk hook against a fresh re-render at
/// the binary's `GATE_SCHEMA_VERSION`. A mismatch means the binary was
/// upgraded since the last `klasp install` (the gate runtime would
/// fail-open in this state).
fn check_hook(repo_root: &Path, surface: &dyn AgentSurface, c: &mut Counters) {
    let agent_id = surface.agent_id();
    let hook_path = surface.hook_path(repo_root);

    let actual = match std::fs::read_to_string(&hook_path) {
        Ok(s) => s,
        Err(_) => {
            c.fail(&format!(
                "hook[{agent_id}]: {} not found; re-run `klasp install`",
                hook_path.display()
            ));
            return;
        }
    };

    let ctx = InstallContext {
        repo_root: repo_root.to_path_buf(),
        dry_run: false,
        force: false,
        schema_version: GATE_SCHEMA_VERSION,
    };
    let expected = surface.render_hook_script(&ctx);

    if actual == expected {
        c.ok(&format!(
            "hook[{agent_id}]: current (schema v{GATE_SCHEMA_VERSION})"
        ));
    } else {
        c.fail(&format!(
            "hook[{agent_id}]: schema drift detected (re-run `klasp install`)"
        ));
    }
}

/// Check 3 — settings JSON exists, parses, and contains klasp's
/// `PreToolUse[Bash]` entry.
///
/// JSON-shaped only — the Codex surface's `settings_path` points at an
/// `AGENTS.md` markdown file with no JSON inside. Doctor's W3 contract is
/// "don't FAIL on a healthy Codex install"; v0.3 will add a typed
/// per-surface health check on the trait so this special-case can go away.
fn check_settings(repo_root: &Path, surface: &dyn AgentSurface, c: &mut Counters) {
    let agent_id = surface.agent_id();
    if agent_id != klasp_agents_claude::ClaudeCodeSurface::AGENT_ID {
        // Non-Claude surfaces have their own format (e.g. AGENTS.md
        // managed-block for Codex). The hook-script byte-equality check
        // run by `check_hook` is the surface-agnostic health signal; the
        // settings-parse logic below is Claude-specific.
        return;
    }
    let settings_path = surface.settings_path(repo_root);

    let raw = match std::fs::read_to_string(&settings_path) {
        Ok(s) => s,
        Err(_) => {
            c.fail(&format!(
                "settings[{agent_id}]: {} not found; re-run `klasp install`",
                settings_path.display()
            ));
            return;
        }
    };

    let root: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            c.fail(&format!(
                "settings[{agent_id}]: failed to parse {} as JSON: {e}",
                settings_path.display()
            ));
            return;
        }
    };

    let hook_command = klasp_agents_claude::ClaudeCodeSurface::HOOK_COMMAND;
    let has_entry = root
        .get("hooks")
        .and_then(|h| h.get("PreToolUse"))
        .and_then(Value::as_array)
        .is_some_and(|arr| {
            arr.iter().any(|matcher_entry| {
                matcher_entry.get("matcher").and_then(Value::as_str) == Some("Bash")
                    && matcher_entry
                        .get("hooks")
                        .and_then(Value::as_array)
                        .is_some_and(|inner| {
                            inner.iter().any(|hook| {
                                hook.get("command").and_then(Value::as_str) == Some(hook_command)
                            })
                        })
            })
        });

    if has_entry {
        c.ok(&format!("settings[{agent_id}]: hook entry present"));
    } else {
        c.fail(&format!(
            "settings[{agent_id}]: klasp hook entry missing; re-run `klasp install`"
        ));
    }
}

/// Check 4 — for each shell-flavoured check, resolve its leading executable
/// on PATH. WARN-only: a missing dev tool isn't an install bug, but the user
/// should know the gate will fail at runtime if invoked.
///
/// Recipe sources (v0.2 W4: `pre_commit`, W5: `fallow`) advertise a known
/// argv0 directly — the recipe knows which binary it shells out to even
/// before the gate renders the full command.
fn check_paths(config: &ConfigV1, c: &mut Counters) {
    for check in &config.checks {
        match &check.source {
            CheckSourceConfig::Shell { command } => match extract_argv0(command) {
                Some(argv0) => match which::which(argv0) {
                    Ok(_) => c.ok(&format!("path[{}]: `{argv0}` found in PATH", check.name)),
                    Err(_) => c.warn(&format!(
                        "path[{}]: `{argv0}` not found in PATH (command: `{command}`)",
                        check.name
                    )),
                },
                None => c.warn(&format!(
                    "path[{}]: could not determine executable from command `{command}`",
                    check.name
                )),
            },
            CheckSourceConfig::PreCommit { .. } => check_recipe_argv0(c, &check.name, "pre-commit"),
            CheckSourceConfig::Fallow { .. } => check_recipe_argv0(c, &check.name, "fallow"),
            CheckSourceConfig::Pytest { .. } => check_recipe_argv0(c, &check.name, "pytest"),
            CheckSourceConfig::Cargo { .. } => check_recipe_argv0(c, &check.name, "cargo"),
            // Plugin sources: probe for `klasp-plugin-<name>` on PATH.
            CheckSourceConfig::Plugin { name, .. } => {
                let binary = format!("klasp-plugin-{name}");
                match which::which(&binary) {
                    Ok(_) => c.ok(&format!("path[{}]: `{binary}` found in PATH", check.name)),
                    Err(_) => c.warn(&format!(
                        "path[{}]: plugin binary `{binary}` not found in PATH \
                         (install it to enable this check)",
                        check.name
                    )),
                }
            }
        }
    }
}

/// Per-recipe PATH probe. Centralised so a new recipe gets PATH coverage
/// in one line and the OK/WARN message format stays consistent.
fn check_recipe_argv0(c: &mut Counters, check_name: &str, argv0: &str) {
    match which::which(argv0) {
        Ok(_) => c.ok(&format!("path[{check_name}]: `{argv0}` found in PATH")),
        Err(_) => c.warn(&format!(
            "path[{check_name}]: `{argv0}` not found in PATH (recipe: {argv0})"
        )),
    }
}

/// Return the first non-`KEY=VALUE` whitespace-separated token from
/// `command`. Shell prefixes like `PYTHONPATH=. pytest` should resolve
/// `pytest`, not `PYTHONPATH=.`.
fn extract_argv0(command: &str) -> Option<&str> {
    command
        .split_ascii_whitespace()
        .find(|token| !token.contains('='))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_argv0_simple_command() {
        assert_eq!(extract_argv0("ruff check ."), Some("ruff"));
    }

    #[test]
    fn extract_argv0_skips_env_prefix() {
        assert_eq!(extract_argv0("PYTHONPATH=. pytest -q"), Some("pytest"));
    }

    #[test]
    fn extract_argv0_skips_multiple_env_prefixes() {
        assert_eq!(
            extract_argv0("FOO=1 BAR=2 cargo test --workspace"),
            Some("cargo")
        );
    }

    #[test]
    fn extract_argv0_empty_command() {
        assert_eq!(extract_argv0(""), None);
    }

    #[test]
    fn extract_argv0_only_env_assignments() {
        assert_eq!(extract_argv0("FOO=1 BAR=2"), None);
    }
}
