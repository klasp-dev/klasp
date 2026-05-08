//! `klasp` library — exports public APIs needed by integration tests.
//!
//! The binary's modules are declared in `main.rs`; this lib re-exports the
//! output formatters so the `tests/output_formats.rs` integration tests can
//! call them directly without spawning a subprocess.

pub mod adopt;
pub mod output;
