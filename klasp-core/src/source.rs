//! `CheckSource` — abstraction over a runnable check.
//!
//! Design: [docs/design.md §3.2]. v0.1 ships exactly one impl (`Shell`,
//! living in the `klasp` binary). The trait exists at v0.1 so v0.2's named
//! recipes (`pre_commit`, `fallow`, `pytest`, `cargo`) and v0.3's
//! subprocess plugins land as new impls without touching the trait.
//!
//! **Lifetime note**: `source_id(&self) -> &str` returns a `&str` tied to
//! `&self`, *not* `&'static str`. v0.3 subprocess plugins have IDs derived
//! from binary filenames discovered at runtime — a `'static` lifetime
//! cannot represent that. This is the v0.3 plugin commitment locked in
//! at v0.1. See issue [klasp#1] for the explicit callout.

use std::path::PathBuf;

use crate::config::CheckConfig;
use crate::trigger::GitEvent;
use crate::verdict::Verdict;

/// Typed error for `CheckSource::run`. Covers runtime-level failures only;
/// semantic failures (lint hits, test failures) ride inside `Verdict::Fail`.
/// Use `CheckSourceError::Other` for impl-specific errors that don't fit the
/// predefined variants.
#[derive(Debug, thiserror::Error)]
pub enum CheckSourceError {
    #[error("failed to spawn check process: {source}")]
    Spawn {
        #[source]
        source: std::io::Error,
    },

    #[error("check produced unparseable output: {0}")]
    Output(String),

    #[error("check exceeded {secs}s timeout")]
    Timeout { secs: u64 },

    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

/// Snapshot of repo metadata passed to every check execution.
///
/// `base_ref` is the merge-base ref between `HEAD` and the upstream tracking
/// branch — the "branch divergence point" diff-aware tools (`pre-commit
/// run --from-ref`, `fallow audit --base`) want. The gate runtime exposes it
/// to shell checks via the `KLASP_BASE_REF` env var; sources that talk to
/// other check tools (named recipes in v0.2, subprocess plugins in v0.3) read
/// it directly off this struct.
///
/// Falls back to `HEAD~1` when no upstream is configured (a fresh checkout,
/// a detached HEAD, or a branch that has never been pushed). The fallback is
/// best-effort — diff-aware tools that don't recognise the ref simply lint
/// the whole tree, which is the same behaviour they'd have without klasp.
///
/// `staged_files` carries the absolute paths of files in the current group's
/// scope when running in monorepo mode (i.e. the subset of staged files that
/// belong to the `klasp.toml` group that owns this invocation). An **empty
/// Vec means "no scoping; the check sees the whole repo"** — this is the
/// back-compat value used by the single-config fallback path and by callers
/// that do not dispatch per-group. Per-source consumption of `staged_files`
/// for fine-grained scoping is deferred to issue #34 (rayon / named recipes);
/// the field is present now so that data is available to checks without a
/// further struct-breaking change.
#[derive(Debug, Clone)]
pub struct RepoState {
    pub root: PathBuf,
    pub git_event: GitEvent,
    pub base_ref: String,
    /// Staged files scoped to this group's `klasp.toml`. Empty = whole-repo
    /// (single-config fallback or unscoped callers).
    pub staged_files: Vec<PathBuf>,
}

/// Outcome of a single `CheckSource::run` invocation.
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub source_id: String,
    pub check_name: String,
    pub verdict: Verdict,
    pub raw_stdout: Option<String>,
    pub raw_stderr: Option<String>,
}

/// Object-safe trait. Implementations are stored as `Box<dyn CheckSource>`
/// in the source registry.
pub trait CheckSource: Send + Sync {
    /// Stable identifier for this source (e.g. `"shell"`, `"pre_commit"`,
    /// `"plugin:klasp-plugin-foo"`). Tied to `&self` lifetime — see module
    /// docs for why this isn't `&'static str`.
    fn source_id(&self) -> &str;

    /// Pre-flight check: does this source know how to handle the given
    /// `CheckConfig`? Used by the registry to dispatch a check to the
    /// right source.
    fn supports_config(&self, config: &CheckConfig) -> bool;

    /// Execute the check and return a structured result.
    ///
    /// Errors here are runtime failures (process spawn errors, malformed
    /// output) — semantic failures (lint hits, test failures) ride inside
    /// `Verdict::Fail`. Use `CheckSourceError::Other` for impl-specific
    /// errors that don't fit the predefined variants.
    fn run(&self, config: &CheckConfig, state: &RepoState)
        -> Result<CheckResult, CheckSourceError>;
}
