//! Output formatters for `klasp gate`.
//!
//! Four formatters ship in v0.3:
//! - [`terminal`] — human-readable stderr text (default, v0.1 behaviour)
//! - [`junit`] — JUnit XML (Surefire/Jenkins schema) for CI test reporters
//! - [`sarif`] — SARIF 2.1.0 JSON for GitHub Code Scanning / security tools
//! - [`json`] — Stable JSON (KLASP_OUTPUT_SCHEMA = 1) for downstream tooling

pub mod json;
pub mod junit;
pub mod sarif;
pub mod terminal;
