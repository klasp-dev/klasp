//! Output formatters for `klasp gate`.
//!
//! Three formatters ship in v0.2.5:
//! - [`terminal`] — human-readable stderr text (default, v0.1 behaviour)
//! - [`junit`] — JUnit XML (Surefire/Jenkins schema) for CI test reporters
//! - [`sarif`] — SARIF 2.1.0 JSON for GitHub Code Scanning / security tools

pub mod junit;
pub mod sarif;
pub mod terminal;
