//! `klasp init --adopt` — first-run gate adoption.
//!
//! See klasp-dev/klasp#97. Detects existing quality gates (pre-commit
//! framework, Husky, Lefthook, plain `.git/hooks`, lint-staged) and
//! produces an [`plan::AdoptionPlan`] that the `init --mode <inspect|mirror|chain>`
//! flow renders, writes, or rejects. Detectors are non-destructive:
//! they read fixture files and propose `klasp.toml` checks; they never
//! modify hook infrastructure.
pub mod detect;
pub mod detect_husky;
pub mod detect_lefthook;
pub mod detect_lint_staged;
pub mod detect_plain_hooks;
pub mod detect_pre_commit;
pub mod mode;
pub mod plan;
pub mod render;
pub mod writer;
