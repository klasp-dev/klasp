//! Plugin subprocess protocol types — v0.3 experimental.
//!
//! Design: [docs/plugin-protocol.md]. The protocol is `PLUGIN_PROTOCOL_VERSION = 0`
//! — explicitly experimental. It may break in any v0.3.x release and graduates
//! to `1` only at v1.0.
//!
//! Plugins are separate binaries named `klasp-plugin-<name>` discovered on
//! `$PATH` at gate time. They communicate over stdin/stdout using JSON. Two
//! subcommands: `--describe` (capability query) and `--gate` (execute a check).
//!
//! See `docs/plugin-protocol.md` for the full wire format specification.

use serde::{Deserialize, Serialize};

use crate::protocol::PLUGIN_PROTOCOL_VERSION;
use crate::trigger::GitEvent;
use crate::verdict::{Finding, Severity};

/// Prefix for plugin binary names on `$PATH`. A plugin named `my-linter` is
/// invoked as `klasp-plugin-my-linter`. Renaming this prefix is a single-site edit.
pub const KLASP_PLUGIN_BIN_PREFIX: &str = "klasp-plugin-";

/// Rule slug used for all plugin infrastructure errors (binary missing,
/// non-zero exit, malformed JSON, version mismatch, timeout). Plugin-reported
/// findings carry their own rule strings — this slug only identifies klasp's
/// own plugin-runtime warnings.
pub const KLASP_PLUGIN_RULE: &str = "klasp::plugin";

/// What a plugin sends in response to `--describe`. Used by klasp to
/// verify forward-compatibility before invoking `--gate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDescribe {
    /// Must equal [`PLUGIN_PROTOCOL_VERSION`] for klasp to accept the plugin.
    pub protocol_version: u32,
    /// Canonical plugin name (e.g. `"klasp-plugin-pre-commit"`).
    pub name: String,
    /// List of config `type` names this plugin supports. Informational only.
    pub config_types: Vec<String>,
    /// Capability flags. Currently only `verdict_v0` is defined.
    #[serde(default)]
    pub supports: PluginSupports,
}

/// Capability flags advertised in `PluginDescribe`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginSupports {
    /// Plugin speaks the v0 verdict protocol (`pass | warn | fail` + `findings`).
    #[serde(default)]
    pub verdict_v0: bool,
}

/// Git event tier as reported on the plugin wire. Mirrors `GitEvent` but
/// kept distinct so wire-format evolution is decoupled from internal types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginTriggerKind {
    Commit,
    Push,
}

impl From<GitEvent> for PluginTriggerKind {
    fn from(event: GitEvent) -> Self {
        match event {
            GitEvent::Commit => PluginTriggerKind::Commit,
            GitEvent::Push => PluginTriggerKind::Push,
        }
    }
}

/// Git event information forwarded to plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginTrigger {
    pub kind: PluginTriggerKind,
    /// Absolute paths of staged files in scope for this check group.
    /// Empty array when running in single-config / push mode.
    pub files: Vec<String>,
}

impl PluginTrigger {
    pub fn from_event(event: GitEvent, staged_files: &[std::path::PathBuf]) -> Self {
        Self {
            kind: event.into(),
            files: staged_files
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        }
    }
}

/// The JSON object written to plugin stdin on `--gate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginGateInput {
    /// Must equal [`PLUGIN_PROTOCOL_VERSION`].
    pub protocol_version: u32,
    /// Mirrors `KLASP_GATE_SCHEMA` for plugins that inspect it.
    pub schema_version: u32,
    /// Current git event.
    pub trigger: PluginTrigger,
    /// Config block forwarded from `klasp.toml`.
    pub config: PluginConfig,
    /// Absolute path to the repo root.
    pub repo_root: String,
    /// Merge-base ref (same value exported as `KLASP_BASE_REF`).
    pub base_ref: String,
}

impl PluginGateInput {
    /// Build a `PluginGateInput` from gate runtime data.
    pub fn new(
        trigger: PluginTrigger,
        config: PluginConfig,
        repo_root: &std::path::Path,
        base_ref: &str,
    ) -> Self {
        Self {
            protocol_version: PLUGIN_PROTOCOL_VERSION,
            schema_version: crate::protocol::GATE_SCHEMA_VERSION,
            trigger,
            config,
            repo_root: repo_root.to_string_lossy().into_owned(),
            base_ref: base_ref.to_string(),
        }
    }
}

/// Plugin-facing view of the `[checks.source]` block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// Plugin name (same as `name` in `CheckSourceConfig::Plugin`).
    pub r#type: String,
    /// Extra args forwarded from `klasp.toml`.
    #[serde(default)]
    pub args: Vec<String>,
    /// Opaque settings blob forwarded from `klasp.toml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
}

/// The JSON object a plugin writes to stdout on `--gate`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginGateOutput {
    /// Must equal [`PLUGIN_PROTOCOL_VERSION`].
    pub protocol_version: u32,
    /// `"pass"`, `"warn"`, or `"fail"`.
    pub verdict: PluginVerdict,
    /// Structured findings. Empty array is valid for `pass` verdicts.
    #[serde(default)]
    pub findings: Vec<PluginFinding>,
}

/// Verdict tier as reported by a plugin. Maps to klasp's `Verdict` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginVerdict {
    Pass,
    Warn,
    Fail,
}

/// A single finding reported by a plugin. Maps to klasp's `Finding` struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginFinding {
    /// `"info"`, `"warn"`, or `"error"`.
    pub severity: Severity,
    /// Rule identifier (e.g. `"ruff/E501"`).
    pub rule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub message: String,
}

impl From<PluginFinding> for Finding {
    fn from(pf: PluginFinding) -> Self {
        Finding {
            rule: pf.rule,
            message: pf.message,
            file: pf.file,
            line: pf.line,
            severity: pf.severity,
        }
    }
}

/// Construct a `Verdict::Warn` for a plugin infrastructure error. Plugin
/// errors (non-zero exit, malformed JSON, timeout, unknown version) produce a
/// `Verdict::Warn` with `rule = KLASP_PLUGIN_RULE`. The gate continues with
/// the remaining checks — plugin errors never crash klasp.
///
/// The plugin name is prepended to the message so renderers and JUnit
/// formatters can attribute the warning to a specific plugin.
pub fn plugin_error_warn(plugin_name: &str, reason: impl Into<String>) -> crate::verdict::Verdict {
    let reason = reason.into();
    crate::verdict::Verdict::Warn {
        findings: vec![Finding {
            rule: KLASP_PLUGIN_RULE.to_string(),
            message: format!("plugin `{plugin_name}`: {reason}"),
            file: None,
            line: None,
            severity: Severity::Warn,
        }],
        message: None,
    }
}
