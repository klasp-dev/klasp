//! Foreign hook-manager conflict detection for `ClaudeCodeSurface`.
//!
//! Unlike Codex — which writes real `.git/hooks/pre-commit` /
//! `.git/hooks/pre-push` scripts and therefore sniffs *those files'
//! contents* for husky / lefthook / pre-commit-framework fingerprints
//! (see [`klasp_agents_codex::git_hooks::detect_conflict`]) — Claude Code
//! installs entirely through `.claude/settings.json` plus a standalone
//! `.claude/hooks/klasp-gate.sh` shim. It never touches `.git/hooks/`, so
//! there is no on-disk hook file to clobber and nothing to *skip writing*.
//!
//! The conflict that matters here is therefore a different shape: when a
//! repo *also* runs husky / lefthook / pre-commit framework, the two gates
//! run on different triggers (Claude's PreToolUse[Bash] gate vs the foreign
//! manager's git-commit hook) and share no state. That is not a failure —
//! both fire independently and correctly — but klasp surfaces it as a
//! non-fatal advisory so the user knows klasp's Claude gate does *not*
//! coordinate with their existing git-hook manager. Detection is by
//! repo-root config-marker presence, the version-stable way each tool
//! advertises itself:
//!
//! - **husky** → a `.husky/` directory (where husky stores its hooks).
//! - **lefthook** → a `lefthook.yml` or `lefthook.yaml` config file.
//! - **pre-commit framework** → a `.pre-commit-config.yaml` file.
//!
//! The [`HookConflict`] enum and its `tool()` accessor mirror the Codex
//! variant names verbatim so downstream UIs render the same canonical
//! strings (`"husky"`, `"lefthook"`, `"pre-commit"`) regardless of which
//! surface produced the warning.

use std::path::Path;

/// A foreign hook manager klasp recognises as co-resident in the repo.
///
/// Mirrors `klasp_agents_codex::git_hooks::HookConflict` (same variants,
/// same canonical `tool()` strings) so the two surfaces speak a common
/// vocabulary, even though their detection mechanisms differ (Codex sniffs
/// hook-file contents; Claude probes repo-root config markers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookConflict {
    /// husky — repo has a `.husky/` directory.
    Husky,
    /// lefthook — repo has a `lefthook.yml` / `lefthook.yaml` config.
    Lefthook,
    /// pre-commit framework — repo has a `.pre-commit-config.yaml`.
    PreCommit,
}

impl HookConflict {
    /// Short canonical tool name, suitable for log output. Matches the
    /// Codex surface's `HookConflict::tool` strings verbatim.
    pub const fn tool(self) -> &'static str {
        match self {
            HookConflict::Husky => "husky",
            HookConflict::Lefthook => "lefthook",
            HookConflict::PreCommit => "pre-commit",
        }
    }

    /// Repo-root marker path (relative to `repo_root`) that fingerprints
    /// this tool. Used by [`detect_conflicts`] and by warning messages so
    /// the user knows exactly which file/dir tripped the advisory.
    pub const fn marker(self) -> &'static str {
        match self {
            HookConflict::Husky => ".husky/",
            HookConflict::Lefthook => "lefthook.yml",
            HookConflict::PreCommit => ".pre-commit-config.yaml",
        }
    }
}

/// Detect every foreign hook manager configured at `repo_root`.
///
/// Returns the conflicts in a stable order (husky, lefthook, pre-commit).
/// A repo can run more than one manager at once, so this returns a `Vec`
/// rather than an `Option` — callers emit one advisory warning per match.
/// An empty result means klasp's Claude gate is the only gate in the repo.
///
/// Detection is by config-marker presence, deliberately conservative:
///
/// - **husky** matches a `.husky/` *directory* (husky's hook store). A
///   stray file named `.husky` does not count.
/// - **lefthook** matches either `lefthook.yml` or `lefthook.yaml`.
/// - **pre-commit framework** matches `.pre-commit-config.yaml`.
pub fn detect_conflicts(repo_root: &Path) -> Vec<HookConflict> {
    let mut conflicts = Vec::new();

    if repo_root.join(".husky").is_dir() {
        conflicts.push(HookConflict::Husky);
    }
    if repo_root.join("lefthook.yml").is_file() || repo_root.join("lefthook.yaml").is_file() {
        conflicts.push(HookConflict::Lefthook);
    }
    if repo_root.join(".pre-commit-config.yaml").is_file() {
        conflicts.push(HookConflict::PreCommit);
    }

    conflicts
}

/// Human-readable advisory for a detected co-resident hook manager.
///
/// Non-fatal: the install completes. The message explains that klasp's
/// Claude gate (PreToolUse[Bash]) runs independently of the foreign git-hook
/// manager and shares no state with it — so both gates fire, but neither
/// coordinates with the other.
pub fn conflict_message(conflict: HookConflict) -> String {
    let tool = conflict.tool();
    let marker = conflict.marker();
    format!(
        "detected {tool} ({marker}) in this repo. klasp's Claude Code gate runs via \
         `.claude/settings.json` (PreToolUse[Bash]) and does not share state with {tool}'s \
         git hooks — both gates run independently. No action needed; this is informational."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_match_codex_canonical_strings() {
        assert_eq!(HookConflict::Husky.tool(), "husky");
        assert_eq!(HookConflict::Lefthook.tool(), "lefthook");
        assert_eq!(HookConflict::PreCommit.tool(), "pre-commit");
    }

    #[test]
    fn detect_returns_empty_on_clean_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_conflicts(dir.path()).is_empty());
    }

    #[test]
    fn detect_husky_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".husky")).unwrap();
        assert_eq!(detect_conflicts(dir.path()), vec![HookConflict::Husky]);
    }

    #[test]
    fn detect_husky_ignores_plain_file_named_dot_husky() {
        // husky's marker is a *directory*; a stray file must not match.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".husky"), "not a dir").unwrap();
        assert!(detect_conflicts(dir.path()).is_empty());
    }

    #[test]
    fn detect_lefthook_yml_and_yaml() {
        for name in ["lefthook.yml", "lefthook.yaml"] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(name), "pre-commit:\n").unwrap();
            assert_eq!(
                detect_conflicts(dir.path()),
                vec![HookConflict::Lefthook],
                "marker {name} should detect lefthook"
            );
        }
    }

    #[test]
    fn detect_pre_commit_framework() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".pre-commit-config.yaml"), "repos: []\n").unwrap();
        assert_eq!(detect_conflicts(dir.path()), vec![HookConflict::PreCommit]);
    }

    #[test]
    fn detect_multiple_managers_in_stable_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".husky")).unwrap();
        std::fs::write(dir.path().join("lefthook.yml"), "pre-commit:\n").unwrap();
        std::fs::write(dir.path().join(".pre-commit-config.yaml"), "repos: []\n").unwrap();
        assert_eq!(
            detect_conflicts(dir.path()),
            vec![
                HookConflict::Husky,
                HookConflict::Lefthook,
                HookConflict::PreCommit
            ],
        );
    }

    #[test]
    fn conflict_message_names_the_tool_and_marker() {
        let msg = conflict_message(HookConflict::Husky);
        assert!(msg.contains("husky"));
        assert!(msg.contains(".husky/"));
        assert!(msg.contains(".claude/settings.json"));
    }
}
