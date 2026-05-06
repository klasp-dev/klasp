//! `klasp-agents-aider` — AiderSurface for klasp.
//!
//! Edits `.aider.conf.yml` to insert `commit-cmd-pre: klasp gate --agent aider`
//! so the klasp gate runs before every aider commit. Existing `commit-cmd-pre`
//! values are chained (klasp first, user value second) rather than overwritten.

pub mod aider_conf;
pub mod surface;

pub use surface::AiderSurface;
