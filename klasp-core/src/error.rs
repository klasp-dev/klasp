//! `KlaspError` — typed error hierarchy for the core crate.
//!
//! Module boundaries inside `klasp-core` use these typed variants so callers
//! can match on cause. The CLI layer (`klasp`) wraps with `anyhow` for
//! ergonomic propagation; this is the "thiserror at boundaries, anyhow at
//! the CLI" split called out in [docs/design.md §12].

use std::path::PathBuf;

use crate::protocol::GateError;
use crate::source::CheckSourceError;
use crate::surface::InstallError;

pub type Result<T, E = KlaspError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum KlaspError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not parse klasp.toml: {0}")]
    ConfigParse(#[from] toml::de::Error),

    #[error(
        "klasp.toml declares version = {found}, but this build of klasp only \
         understands version = {supported}. Upgrade klasp or downgrade the config."
    )]
    ConfigVersion { found: u32, supported: u32 },

    #[error("klasp.toml not found in {searched:?}")]
    ConfigNotFound { searched: Vec<PathBuf> },

    #[error("gate protocol error: {0}")]
    Protocol(#[from] GateError),

    #[error("install error: {0}")]
    Install(#[from] InstallError),

    #[error("check source error: {0}")]
    CheckSource(#[from] CheckSourceError),

    #[error("trigger classification failed: {0}")]
    Trigger(String),
}
