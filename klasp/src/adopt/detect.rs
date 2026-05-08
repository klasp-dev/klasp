//! Aggregator that runs all per-gate detectors and collects their findings
//! into a single [`AdoptionPlan`].
//!
//! Detectors are run in a fixed order: pre_commit, husky, lefthook,
//! plain_hooks, lint_staged. Each detector is non-destructive (read-only)
//! so the order only affects the rendering order in the plan, not correctness.
//!
//! See klasp-dev/klasp#97.

use std::io;
use std::path::{Path, PathBuf};

use crate::adopt::plan::{AdoptionPlan, DetectedGate, HookStage, TriggerKind};

/// Run every detector against `repo_root` and aggregate the findings.
///
/// Detection order: pre_commit → husky → lefthook → plain_hooks → lint_staged.
/// Each detector may return zero or more findings. I/O errors from individual
/// detectors are propagated immediately (fail-fast).
///
/// # Errors
///
/// Returns `Err` if any detector encounters an I/O error while probing the
/// filesystem. Absence of known config files is never an error.
pub fn detect_all(repo_root: &Path) -> io::Result<AdoptionPlan> {
    let mut findings: Vec<DetectedGate> = Vec::new();

    findings.extend(super::detect_pre_commit::detect(repo_root)?);
    findings.extend(super::detect_husky::detect(repo_root)?);
    findings.extend(super::detect_lefthook::detect(repo_root)?);
    findings.extend(super::detect_plain_hooks::detect(repo_root)?);
    findings.extend(super::detect_lint_staged::detect(repo_root)?);

    Ok(AdoptionPlan { findings })
}

/// Return the first path in `candidates` (relative to `root`) that is a file.
pub(super) fn first_existing_file(root: &Path, candidates: &[&str]) -> Option<PathBuf> {
    candidates.iter().map(|name| root.join(name)).find(|p| p.is_file())
}

/// Map a [`HookStage`] to its corresponding [`TriggerKind`].
pub(super) fn hook_to_trigger(hook: HookStage) -> TriggerKind {
    match hook {
        HookStage::PreCommit => TriggerKind::Commit,
        HookStage::PrePush => TriggerKind::Push,
    }
}
