//! The **reader half** of the agentic-flow receipt schema.
//!
//! The receipt schema is co-owned: the *writer* side ships in the agentic-flow
//! orchestrator (`~/.claude/agentic-flow/`, a separate repo from klasp) and the
//! *reader* side lives here. The single source of truth for the field shapes
//! and the canonical `diff_hash` recipe is
//! [`docs/agentic-flow-receipts.md`](../../../docs/agentic-flow-receipts.md) in
//! the klasp repository — keep both sides byte-identical.
//!
//! All fields are optional with `#[serde(default)]` so a partially-written or
//! older-manifest receipt still parses; the auditor (`runner.rs`) enforces the
//! *required-for-this-status* fields itself. Unknown extra fields are silently
//! ignored (serde default behaviour) for forward-compat.

// These structs are the documentation-grade *reader contract* for the receipt
// schema. Several fields (e.g. `artifacts`, `verdict`, `manifest_version`, the
// `history[]` index, `confirmed_at`) are part of the documented JSON shape and
// are deserialized for forward-compat, but the v1 audit logic does not act on
// every one of them. Keeping them as named fields documents the contract; the
// allow prevents dead-code noise without dropping schema fidelity.
#![allow(dead_code)]

use serde::Deserialize;

/// A single per-step receipt: `.agentic-flow/receipts/NN-step.json`.
///
/// agentic-flow (the WRITER) creates one of these per step on
/// completion/skip; this plugin (the AUDITOR) only ever READS them.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Receipt {
    /// `"NN-id"` — ties to flow.yaml position + id (e.g. `"07-code-review"`).
    #[serde(default)]
    pub step: Option<String>,
    /// `"completed"` | `"skipped"` | `"blocked"`.
    #[serde(default)]
    pub status: Option<String>,
    /// `"auto"` | `"user-confirm"` — copied from flow.yaml.
    #[serde(default)]
    pub gating: Option<String>,
    /// Git branch the step ran on (required for `completed`).
    #[serde(default)]
    pub branch: Option<String>,
    /// The KLASP_BASE_REF the step compared against (required for `completed`).
    #[serde(default)]
    pub base_ref: Option<String>,
    /// Full HEAD sha when the step ran (required for `completed`).
    #[serde(default)]
    pub head: Option<String>,
    /// The load-bearing freshness field: `"sha256:<hex>"` (required for `completed`).
    #[serde(default)]
    pub diff_hash: Option<String>,
    /// Machine-readable artifact list (optional).
    #[serde(default)]
    pub artifacts: Vec<String>,
    /// The step's own outcome, e.g. `"pass"` (informational).
    #[serde(default)]
    pub verdict: Option<String>,
    /// Required `true` for `gating == "user-confirm"` completed receipts.
    #[serde(default)]
    pub user_confirmed: bool,
    /// Opaque confirmation id — required-if `user_confirmed == true`.
    #[serde(default)]
    pub confirmation_id: Option<String>,
    /// RFC3339 companion to `confirmation_id` (optional).
    #[serde(default)]
    pub confirmed_at: Option<String>,
    /// Required for `status == "skipped"`.
    #[serde(default)]
    pub skip_reason: Option<String>,
    /// flow.yaml `version` echo — lets the plugin spot an older-manifest receipt.
    #[serde(default)]
    pub manifest_version: Option<u32>,
    /// RFC3339 timestamp (required for `completed`).
    #[serde(default)]
    pub started_at: Option<String>,
    /// RFC3339 timestamp (required for `completed`).
    #[serde(default)]
    pub completed_at: Option<String>,
}

impl Receipt {
    /// True when `status == "completed"`.
    pub fn is_completed(&self) -> bool {
        self.status.as_deref() == Some("completed")
    }

    /// True when `status == "skipped"`.
    pub fn is_skipped(&self) -> bool {
        self.status.as_deref() == Some("skipped")
    }
}

/// The version-1 shape of `.agentic-flow/state.json`. The plugin uses state.json
/// as the INDEX/cursor; receipts are the per-step source of truth.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StateJson {
    /// Schema version — the gate warns if it differs from the supported version.
    #[serde(default)]
    pub version: Option<u32>,
    /// The cursor: id of the step currently in progress.
    #[serde(default)]
    pub current_step: Option<String>,
    /// Bare step ids that were intentionally skipped (e.g. `"ideate"`).
    #[serde(default)]
    pub skipped: Vec<String>,
    /// Ordering + outcomes history.
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
}

/// One `history[]` entry in state.json.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HistoryEntry {
    /// Bare step id.
    #[serde(default)]
    pub id: Option<String>,
    /// RFC3339 timestamp.
    #[serde(default)]
    pub ran_at: Option<String>,
    /// `"completed"` | `"skipped"` | `"blocked"`.
    #[serde(default)]
    pub outcome: Option<String>,
    /// Free-text reason (skip) — mirrors `skip_reason` on a receipt.
    #[serde(default)]
    pub reason: Option<String>,
    /// Free-text artifact (legacy; superseded by receipt `artifacts[]`).
    #[serde(default)]
    pub artifact: Option<String>,
}

/// flow.yaml manifest — the source of truth for step order, ids, gating, enabled.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Manifest {
    /// Manifest version.
    #[serde(default)]
    pub version: Option<u32>,
    /// Ordered list of steps.
    #[serde(default)]
    pub steps: Vec<ManifestStep>,
}

/// One `steps[]` entry in flow.yaml.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ManifestStep {
    /// Bare step id (e.g. `"code-review"`).
    #[serde(default)]
    pub id: Option<String>,
    /// `true` unless explicitly disabled. Defaults to `true` when absent.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// `"auto"` | `"user-confirm"`.
    #[serde(default)]
    pub gating: Option<String>,
}

fn default_true() -> bool {
    true
}
