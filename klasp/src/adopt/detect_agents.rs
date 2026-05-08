//! Machine-level agent detection: sniff well-known per-user directories to
//! determine which AI coding agents are installed on this machine.
//!
//! This is intentionally separate from the per-repo [`super::detect`] module
//! (which finds existing gate infrastructure). The two detection passes serve
//! different purposes:
//!
//! - Per-repo detection: "what gates already exist in this repo?"
//! - Per-machine detection: "which agent surfaces should `[gate].agents` cover?"
//!
//! Called from `klasp init --adopt --mode mirror` to narrow the default
//! agents list from the three-agent fallback to just what the user has
//! installed. Also called by `klasp setup` for the same purpose.
//!
//! See klasp-dev/klasp#103.

use std::path::{Path, PathBuf};

/// The agents that klasp's surface registry supports, in canonical order.
pub const ALL_AGENTS: &[&str] = &["claude_code", "codex", "aider"];

/// Probe the machine to determine which agent surfaces are installed.
///
/// Returns a non-empty `Vec<String>` of agent IDs. If no known agent is found,
/// falls back to [`ALL_AGENTS`] with a note that the user should edit the
/// config — this matches today's existing default behaviour so no user is
/// left with an invalid empty agents list.
///
/// `home_dir` is the user's home directory (pass `dirs::home_dir()` in
/// production; supply a tempdir in tests).
pub fn detect_installed_agents(home_dir: Option<&Path>) -> Vec<String> {
    let Some(home) = home_dir else {
        return all_agents_fallback();
    };

    let mut found = Vec::new();

    if probe_claude_code(home) {
        found.push("claude_code".to_string());
    }
    if probe_codex(home) {
        found.push("codex".to_string());
    }
    if probe_aider(home) {
        found.push("aider".to_string());
    }

    if found.is_empty() {
        all_agents_fallback()
    } else {
        found
    }
}

/// Detect Claude Code: `~/.claude/` directory must exist.
fn probe_claude_code(home: &Path) -> bool {
    home.join(".claude").is_dir()
}

/// Detect Codex: `~/.codex/` directory must exist.
fn probe_codex(home: &Path) -> bool {
    home.join(".codex").is_dir()
}

/// Detect Aider: any of `~/.aider`, `~/.aider.conf.yml`, `~/.aiderignore`.
/// Aider's global config can land in several places.
fn probe_aider(home: &Path) -> bool {
    aider_probe_paths(home).into_iter().any(|p| p.exists())
}

fn aider_probe_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".aider"),
        home.join(".aider.conf.yml"),
        home.join(".aiderignore"),
    ]
}

/// Fall back to today's three-agent default when we can't determine which
/// agents the user runs. The caller should add an "edit-me" comment.
fn all_agents_fallback() -> Vec<String> {
    ALL_AGENTS.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_home_dir_returns_all_three() {
        let agents = detect_installed_agents(None);
        assert_eq!(agents, vec!["claude_code", "codex", "aider"]);
    }

    #[test]
    fn empty_home_returns_all_three_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        // No agent dirs created — should fall through to all-three fallback.
        let agents = detect_installed_agents(Some(tmp.path()));
        assert_eq!(agents, vec!["claude_code", "codex", "aider"]);
    }

    #[test]
    fn claude_only_home_returns_claude_code() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".claude")).unwrap();
        let agents = detect_installed_agents(Some(tmp.path()));
        assert_eq!(agents, vec!["claude_code"]);
    }

    #[test]
    fn codex_only_home_returns_codex() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".codex")).unwrap();
        let agents = detect_installed_agents(Some(tmp.path()));
        assert_eq!(agents, vec!["codex"]);
    }

    #[test]
    fn aider_conf_yml_detected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".aider.conf.yml"), "commit: true\n").unwrap();
        let agents = detect_installed_agents(Some(tmp.path()));
        assert_eq!(agents, vec!["aider"]);
    }

    #[test]
    fn all_three_detected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".claude")).unwrap();
        std::fs::create_dir(tmp.path().join(".codex")).unwrap();
        std::fs::write(tmp.path().join(".aider.conf.yml"), "commit: true\n").unwrap();
        let agents = detect_installed_agents(Some(tmp.path()));
        assert_eq!(agents, vec!["claude_code", "codex", "aider"]);
    }

    #[test]
    fn claude_and_codex_detected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".claude")).unwrap();
        std::fs::create_dir(tmp.path().join(".codex")).unwrap();
        let agents = detect_installed_agents(Some(tmp.path()));
        assert_eq!(agents, vec!["claude_code", "codex"]);
    }
}
