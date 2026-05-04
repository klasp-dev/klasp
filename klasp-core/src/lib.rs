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
pub mod protocol;
pub mod source;
pub mod surface;
pub mod trigger;
pub mod verdict;

pub use config::{
    CheckConfig, CheckSourceConfig, ConfigV1, GateConfig, TriggerConfig, CONFIG_VERSION,
};
pub use error::{KlaspError, Result};
pub use protocol::{GateError, GateInput, GateProtocol, ToolInput, GATE_SCHEMA_VERSION};
pub use source::{CheckResult, CheckSource, RepoState};
pub use surface::{AgentSurface, InstallContext, InstallError, InstallReport};
pub use trigger::{GitEvent, Trigger};
pub use verdict::{Finding, Severity, Verdict, VerdictPolicy};
