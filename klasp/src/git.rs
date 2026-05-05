//! Tiny git helpers used by the gate runtime.
//!
//! Just enough to answer "what's the working repo root?" and "what's the
//! diff-aware base ref?" without pulling `git2` into the dependency closure.
//! The gate path runs once per tool-call, so a single `git rev-parse`
//! subprocess is cheap; the failure path falls back to `CLAUDE_PROJECT_DIR`
//! so klasp gates work even outside a git checkout (worktrees, sparse
//! clones, the test harness).

use std::path::{Path, PathBuf};
use std::process::Command;

use klasp_core::CLAUDE_PROJECT_DIR_ENV;

/// Resolve a working repo root from the current working directory.
///
/// Lookup order, mirroring [docs/design.md §6, §14]:
///
/// 1. `CLAUDE_PROJECT_DIR` — Claude Code sets this when invoking hooks, and
///    it's the strongest signal because it doesn't depend on git being
///    initialised.
/// 2. `git rev-parse --show-toplevel` from the current dir.
/// 3. The current dir itself, last-resort fallback.
///
/// Returns `None` only when every attempt fails (no env var, no git, no
/// readable cwd). The gate runtime treats `None` as a fail-open signal.
pub fn find_repo_root_from_cwd() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var(CLAUDE_PROJECT_DIR_ENV) {
        let candidate = PathBuf::from(dir);
        if candidate.is_dir() {
            return Some(candidate);
        }
    }

    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
    {
        if output.status.success() {
            let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !raw.is_empty() {
                let candidate = PathBuf::from(raw);
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
        }
    }

    std::env::current_dir().ok()
}

/// Compute the diff-aware base ref klasp exports as `KLASP_BASE_REF`.
///
/// Lookup order, mirroring [docs/recipes.md §`${KLASP_BASE_REF}`] and
/// [docs/design.md §3.5]:
///
/// 1. `git merge-base @{upstream} HEAD` — the canonical "where did this
///    branch diverge from the tracking branch?" lookup. Set when the user
///    configured an upstream (`git push -u`, `git branch --set-upstream-to`).
/// 2. `git merge-base origin/main HEAD` — the convention klasp expects when
///    the user hasn't set an upstream but does follow the modern default
///    branch name.
/// 3. `git merge-base origin/master HEAD` — same fallback for legacy repos.
/// 4. `HEAD~1` — last-resort fallback for fresh repos with no remote.
///
/// Always returns a `String`, never an error: a missing remote is the common
/// case for a fresh `git init`, not a failure mode the gate should fail-open
/// over. Diff-aware tools that don't recognise the fallback ref will lint
/// the whole tree — the same behaviour they'd have without klasp.
///
/// **`cwd`** is the resolved repo root (from [`find_repo_root_from_cwd`]) so
/// the `git merge-base` invocation runs against the right repo even when the
/// caller's `current_dir` is a subdirectory or a worktree.
pub fn compute_base_ref(cwd: &Path) -> String {
    const CANDIDATES: &[&str] = &["@{upstream}", "origin/main", "origin/master"];

    for candidate in CANDIDATES {
        if let Some(sha) = git_merge_base(cwd, candidate, "HEAD") {
            return sha;
        }
    }

    "HEAD~1".to_string()
}

/// Run `git merge-base <a> <b>` in `cwd`. Returns the resolved commit SHA on
/// success, `None` if either ref is unknown or the subprocess fails.
fn git_merge_base(cwd: &Path, a: &str, b: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["merge-base", a, b])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        None
    } else {
        Some(raw)
    }
}

/// Return staged file paths (absolute) from `git diff --staged --name-only`.
///
/// Returns an empty `Vec` on failure (no staging area, not a git repo, etc.).
/// Callers treat an empty list as "fall back to single-config mode".
pub fn staged_files(repo_root: &Path) -> Vec<PathBuf> {
    let output = match Command::new("git")
        .args(["diff", "--staged", "--name-only", "--diff-filter=ACMRT"])
        .current_dir(repo_root)
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    if !output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| repo_root.join(l))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    /// Initialise a fresh git repo in `dir` and create a single commit. Used
    /// by the merge-base tests to give `git merge-base @{upstream} HEAD`
    /// something to fail against and `HEAD~1` something to resolve.
    fn init_repo_with_commits(dir: &Path, commits: usize) {
        run(dir, &["init", "--initial-branch=main"]);
        run(dir, &["config", "user.email", "klasp-test@example.com"]);
        run(dir, &["config", "user.name", "klasp-test"]);
        run(dir, &["config", "commit.gpgsign", "false"]);
        for i in 0..commits {
            std::fs::write(dir.join(format!("f{i}.txt")), format!("commit {i}"))
                .expect("write fixture file");
            run(dir, &["add", "."]);
            run(dir, &["commit", "-m", &format!("c{i}")]);
        }
    }

    fn run(cwd: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .status()
            .expect("spawn git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn compute_base_ref_falls_back_to_head_tilde_one_without_remote() {
        let tmp = TempDir::new().unwrap();
        init_repo_with_commits(tmp.path(), 2);
        // No upstream, no `origin/*` — the canonical fallback path.
        assert_eq!(compute_base_ref(tmp.path()), "HEAD~1");
    }

    #[test]
    fn compute_base_ref_uses_upstream_when_available() {
        // Two repos: `upstream` (the "remote") and `local` (a clone). We
        // commit on the local clone past `origin/main`; the merge-base should
        // resolve to the commit `origin/main` points at.
        let upstream_tmp = TempDir::new().unwrap();
        let local_tmp = TempDir::new().unwrap();
        init_repo_with_commits(upstream_tmp.path(), 1);

        run(
            local_tmp.path(),
            &[
                "clone",
                upstream_tmp.path().to_str().unwrap(),
                local_tmp.path().to_str().unwrap(),
            ],
        );
        run(
            local_tmp.path(),
            &["config", "user.email", "klasp-test@example.com"],
        );
        run(local_tmp.path(), &["config", "user.name", "klasp-test"]);
        run(local_tmp.path(), &["config", "commit.gpgsign", "false"]);

        // Capture the SHA `origin/main` points at — that's what merge-base
        // should return after we add a divergent commit on the local branch.
        let expected = String::from_utf8_lossy(
            &Command::new("git")
                .args(["rev-parse", "origin/main"])
                .current_dir(local_tmp.path())
                .output()
                .expect("rev-parse origin/main")
                .stdout,
        )
        .trim()
        .to_string();

        std::fs::write(local_tmp.path().join("local.txt"), "local").unwrap();
        run(local_tmp.path(), &["add", "."]);
        run(local_tmp.path(), &["commit", "-m", "local divergence"]);

        let got = compute_base_ref(local_tmp.path());
        assert_eq!(got, expected, "merge-base @{{u}} should match origin/main");
    }
}
