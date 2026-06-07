//! Wire-protocol types for the klasp plugin protocol v0.
//!
//! # Why are these types duplicated?
//!
//! This plugin is intentionally outside the klasp workspace to prove that
//! third-party plugin authors can ship plugins without depending on klasp-core.
//! The **protocol is the contract**, not klasp-core's types.
//!
//! Plugin authors: copy-paste these definitions into your own crate. The JSON
//! field names are stable for `PROTOCOL_VERSION = 0` (experimental) and will
//! be stable from `1` onward. Track `docs/plugin-protocol.md` in the klasp
//! repository for any changes.
//!
//! > **Warning:** `PROTOCOL_VERSION = 0` is explicitly unstable. It may change
//! > in any v0.3.x release without a deprecation period.

use serde::{Deserialize, Serialize};

/// Plugin protocol version. Must match klasp's `PLUGIN_PROTOCOL_VERSION`.
/// Increment signals a breaking wire-format change.
pub const PROTOCOL_VERSION: u32 = 0;

// ── Describe ─────────────────────────────────────────────────────────────────

/// Sent to stdout on `--describe`. klasp reads this before every `--gate`
/// invocation to verify forward-compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDescribe {
    /// Must equal [`PROTOCOL_VERSION`].
    pub protocol_version: u32,
    /// Canonical name including the `klasp-plugin-` prefix.
    pub name: String,
    /// Informational: config `type` values this plugin supports.
    pub config_types: Vec<String>,
    /// Capability flags.
    #[serde(default)]
    pub supports: PluginSupports,
}

/// Capability flags in [`PluginDescribe`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginSupports {
    /// Plugin speaks the v0 verdict protocol.
    #[serde(default)]
    pub verdict_v0: bool,
}

// ── Gate input ────────────────────────────────────────────────────────────────

/// Read from stdin on `--gate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginGateInput {
    /// Must equal [`PROTOCOL_VERSION`] (currently `0`).
    pub protocol_version: u32,
    /// Gate wire-protocol version (currently `2`).
    pub schema_version: u32,
    /// Git event that triggered the gate.
    pub trigger: PluginTrigger,
    /// Config forwarded from `klasp.toml`.
    pub config: PluginConfig,
    /// Absolute path to the repository root.
    pub repo_root: String,
    /// Merge-base ref (same as `KLASP_BASE_REF` env var).
    pub base_ref: String,
}

/// Git trigger information forwarded by klasp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginTrigger {
    /// `"commit"` or `"push"`.
    pub kind: PluginTriggerKind,
    /// Absolute paths of staged files in scope. Empty on push events.
    pub files: Vec<String>,
}

/// Git event tier on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginTriggerKind {
    Commit,
    Push,
}

/// Plugin-facing view of `[checks.source]` in `klasp.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Plugin name from `klasp.toml`.
    pub r#type: String,
    /// Extra args forwarded from `klasp.toml`'s `args` field.
    #[serde(default)]
    pub args: Vec<String>,
    /// Opaque settings blob from `[checks.source.settings]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
}

// ── Gate output ───────────────────────────────────────────────────────────────

/// Written to stdout on `--gate`. Plugin must exit 0 even for `fail` verdicts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginGateOutput {
    /// Must equal [`PROTOCOL_VERSION`].
    pub protocol_version: u32,
    /// `"pass"`, `"warn"`, or `"fail"`.
    pub verdict: PluginVerdict,
    /// Structured findings. Empty is valid for `pass`.
    #[serde(default)]
    pub findings: Vec<PluginFinding>,
}

/// Verdict tier as reported on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginVerdict {
    Pass,
    Warn,
    Fail,
}

/// A single finding reported by the plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginFinding {
    /// `"info"`, `"warn"`, or `"error"`.
    pub severity: String,
    /// Rule identifier (e.g. `"pre-commit/ruff"`).
    pub rule: String,
    /// Optional file path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Optional 1-based line number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Human-readable description.
    pub message: String,
}
