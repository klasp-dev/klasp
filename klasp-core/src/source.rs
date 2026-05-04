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

/// Snapshot of repo metadata passed to every check execution.
#[derive(Debug, Clone)]
pub struct RepoState {
    pub root: PathBuf,
    pub git_event: GitEvent,
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

    /// Execute the check and return a structured result. Errors here are
    /// runtime failures (process spawn errors, malformed output) — semantic
    /// failures (lint hits, test failures) ride inside `Verdict::Fail`.
    fn run(&self, config: &CheckConfig, state: &RepoState) -> Result<CheckResult, anyhow::Error>;
}
