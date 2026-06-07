//! Text-manipulation helpers shared across klasp's agent surfaces.
//!
//! These are pure, filesystem-free string transforms. The first inhabitant
//! is [`managed_block`], the delimited-region writer that every surface
//! (AGENTS.md markdown, git-hook shell, future YAML configs) layers its
//! file-format-specific framing on top of.

pub mod managed_block;
