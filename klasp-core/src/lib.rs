//! `klasp-core` — public traits, types, and protocol for klasp.
//!
//! This crate is the v0.3 plugin-API contract surface. The trait shapes,
//! type signatures, and constants exposed here are committed at v0.1; v0.2
//! and v0.3 add capabilities by introducing new impls, not by mutating
//! these definitions.
//!
//! See [`docs/design.md`](https://github.com/klasp-dev/klasp/blob/main/docs/design.md)
//! §3 for the rationale behind each abstraction.

pub mod config;
pub mod error;
pub mod plugin;
pub mod plugin_disable;
pub mod protocol;
pub mod source;
pub mod surface;
pub mod trigger;
pub mod verdict;

pub use config::{
    discover_config_for_path, load_config_for_path, CheckConfig, CheckSourceConfig, ConfigV1,
    GateConfig, TriggerConfig, CLAUDE_PROJECT_DIR_ENV, CONFIG_VERSION,
};
pub use error::{KlaspError, Result};
pub use plugin::{
    plugin_error_warn, PluginConfig, PluginDescribe, PluginFinding, PluginGateInput,
    PluginGateOutput, PluginSupports, PluginTrigger, PluginTriggerKind, PluginVerdict,
    KLASP_PLUGIN_BIN_PREFIX, KLASP_PLUGIN_RULE,
};
pub use plugin_disable::{
    add as plugin_disable_add, load as plugin_disable_load, resolve_disable_list_path,
    validate_plugin_name, KLASP_DISABLED_PLUGINS_FILE_ENV,
};
pub use protocol::{
    GateError, GateInput, GateProtocol, ToolInput, GATE_SCHEMA_VERSION, PLUGIN_PROTOCOL_VERSION,
};
pub use source::{CheckResult, CheckSource, CheckSourceError, RepoState};
pub use surface::{AgentSurface, InstallContext, InstallError, InstallReport};
pub use trigger::{GitEvent, Trigger};
pub use verdict::{Finding, Severity, Verdict, VerdictPolicy};
