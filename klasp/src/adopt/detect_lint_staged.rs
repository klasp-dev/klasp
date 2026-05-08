//! Detector for [lint-staged](https://github.com/lint-staged/lint-staged).
//!
//! Detection strategy (in priority order):
//!
//! 1. `.lintstagedrc` (bare, no extension)
//! 2. `.lintstagedrc.json`
//! 3. `.lintstagedrc.js`
//! 4. `.lintstagedrc.cjs`
//! 5. `.lintstagedrc.mjs`
//! 6. `.lintstagedrc.yaml`
//! 7. `.lintstagedrc.yml`
//! 8. `package.json` — substring search for `"lint-staged"` followed by `:`
//!
//! When multiple files match, the first match in the list above wins.
//!
//! # JSON/TOML parsing note
//!
//! Rather than pulling in a JSON parser, the detector uses a substring
//! search for `"lint-staged":` inside `package.json`. This is a
//! deliberate heuristic: it will match any top-level or nested key named
//! `lint-staged`. In practice `lint-staged` only ever appears as a
//! top-level key in `package.json`, so false positives are extremely
//! unlikely. A false negative is possible if the key is split across lines
//! in a non-standard formatter, but that is not a real-world concern.
//!
//! # Package manager detection
//!
//! The shell command in the proposed check is package-manager-aware:
//!
//! - `pnpm-lock.yaml` present → `pnpm exec lint-staged`
//! - `yarn.lock` present → `yarn lint-staged`
//! - fallback → `npx lint-staged`
//!
//! # Duplicate-execution warning
//!
//! If `.husky/pre-commit` exists and its contents reference `lint-staged`,
//! both the Husky hook and klasp's shell check would run lint-staged at
//! commit time. We emit a warning so the user can remove the duplication.
//!
//! See klasp-dev/klasp#97.

use std::io;
use std::path::Path;

use crate::adopt::detect::first_existing_file;
use crate::adopt::plan::{
    ChainSupport, DetectedGate, GateType, ProposedCheck, ProposedCheckSource, TriggerKind,
};

/// Standalone lint-staged config files, in priority order.
const STANDALONE_CONFIGS: &[&str] = &[
    ".lintstagedrc",
    ".lintstagedrc.json",
    ".lintstagedrc.js",
    ".lintstagedrc.cjs",
    ".lintstagedrc.mjs",
    ".lintstagedrc.yaml",
    ".lintstagedrc.yml",
];

/// Substring searched inside `package.json` to detect lint-staged.
///
/// Limitation: this heuristic matches any key named `lint-staged` regardless
/// of nesting depth. Real-world `package.json` files always put lint-staged
/// at the top level, so false positives are not a practical concern.
const PACKAGE_JSON_MARKER: &str = "\"lint-staged\"";

/// Detect lint-staged config at `repo_root`.
///
/// Returns a single [`DetectedGate`] with [`GateType::LintStaged`] when a
/// config file or `package.json` key is found, or an empty `Vec` otherwise.
///
/// # Errors
///
/// Returns `Err` only for unexpected I/O failures. Absence of config files
/// is not an error.
pub fn detect(repo_root: &Path) -> io::Result<Vec<DetectedGate>> {
    let (source_path, from_package_json) = match find_config(repo_root)? {
        Some(found) => found,
        None => return Ok(vec![]),
    };

    let command = resolve_command(repo_root);
    let warnings = build_warnings(repo_root)?;

    let gate = DetectedGate {
        gate_type: GateType::LintStaged,
        source_path,
        proposed_checks: vec![proposed_check(command)],
        chain_support: ChainSupport::ManualOnly,
        manual_chain_instructions: Some(manual_instructions(from_package_json)),
        warnings,
    };

    Ok(vec![gate])
}

/// Find the first lint-staged config file, returning the path and whether it
/// came from `package.json` (as opposed to a standalone file).
///
/// Returns `None` when no config is found.
fn find_config(repo_root: &Path) -> io::Result<Option<(std::path::PathBuf, bool)>> {
    // Check standalone files first.
    if let Some(path) = first_existing_file(repo_root, STANDALONE_CONFIGS) {
        return Ok(Some((path, false)));
    }

    // Fall back to package.json substring search.
    let pkg_json = repo_root.join("package.json");
    let contents = match std::fs::read_to_string(&pkg_json) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if package_json_has_lint_staged(&contents) {
        return Ok(Some((pkg_json, true)));
    }

    Ok(None)
}

/// Returns `true` when `contents` looks like a `package.json` that declares
/// a `lint-staged` key.
///
/// The search looks for `"lint-staged"` followed (after optional whitespace)
/// by `:`, which is the JSON syntax for a key–value pair. This is a
/// heuristic; see module-level docs for limitations.
pub(super) fn package_json_has_lint_staged(contents: &str) -> bool {
    let mut rest = contents;
    while let Some(pos) = rest.find(PACKAGE_JSON_MARKER) {
        let after_key = &rest[pos + PACKAGE_JSON_MARKER.len()..];
        let trimmed = after_key.trim_start();
        if trimmed.starts_with(':') {
            return true;
        }
        // Move past this occurrence and keep searching.
        rest = &rest[pos + 1..];
    }
    false
}

/// Determine the package-manager-aware lint-staged invocation.
fn resolve_command(repo_root: &Path) -> String {
    if repo_root.join("pnpm-lock.yaml").is_file() {
        return "pnpm exec lint-staged".to_string();
    }
    if repo_root.join("yarn.lock").is_file() {
        return "yarn lint-staged".to_string();
    }
    "npx lint-staged".to_string()
}

/// Build the list of duplicate-execution warnings.
fn build_warnings(repo_root: &Path) -> io::Result<Vec<String>> {
    let husky_hook = repo_root.join(".husky/pre-commit");
    let contents = match std::fs::read_to_string(&husky_hook) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };

    if contents.contains("lint-staged") {
        return Ok(vec![
            "if Husky pre-commit also runs lint-staged, klasp will run it a second time on \
             commit; either disable the Husky entry or remove this klasp check to avoid \
             duplicate execution."
                .to_string(),
        ]);
    }

    Ok(vec![])
}

/// The proposed `klasp.toml` check that mirrors lint-staged.
fn proposed_check(command: String) -> ProposedCheck {
    ProposedCheck {
        name: "lint-staged".to_string(),
        triggers: vec![TriggerKind::Commit],
        timeout_secs: 120,
        source: ProposedCheckSource::Shell { command },
    }
}

/// Manual chaining instructions.
fn manual_instructions(from_package_json: bool) -> String {
    let config_hint = if from_package_json {
        "your `package.json` `lint-staged` key"
    } else {
        "your `.lintstagedrc*` file"
    };
    format!(
        "klasp's proposed check runs lint-staged as a shell command that reads {config_hint}. \
         If you also have a Husky pre-commit hook that runs lint-staged, remove one to avoid \
         running lint-staged twice per commit."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn empty_repo_yields_no_findings() {
        let dir = tempfile::tempdir().unwrap();
        let findings = detect(dir.path()).unwrap();
        assert!(findings.is_empty());
    }

    #[test]
    fn package_json_with_lint_staged_key_yields_one_finding() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{ "lint-staged": { "*.ts": "eslint" } }"#,
        )
        .unwrap();

        let findings = detect(dir.path()).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].gate_type, GateType::LintStaged);
    }

    #[test]
    fn command_defaults_to_npx_when_no_lockfile() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{ "lint-staged": {} }"#,
        )
        .unwrap();

        let findings = detect(dir.path()).unwrap();
        let check = &findings[0].proposed_checks[0];
        assert!(
            matches!(&check.source, ProposedCheckSource::Shell { command } if command == "npx lint-staged"),
            "expected npx lint-staged, got {:?}",
            check.source
        );
    }

    #[test]
    fn command_uses_pnpm_when_pnpm_lockfile_present() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{ "lint-staged": {} }"#,
        )
        .unwrap();
        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();

        let findings = detect(dir.path()).unwrap();
        let check = &findings[0].proposed_checks[0];
        assert!(
            matches!(&check.source, ProposedCheckSource::Shell { command } if command == "pnpm exec lint-staged"),
            "expected pnpm exec lint-staged, got {:?}",
            check.source
        );
    }

    #[test]
    fn command_uses_yarn_when_yarn_lock_present() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{ "lint-staged": {} }"#,
        )
        .unwrap();
        fs::write(dir.path().join("yarn.lock"), "").unwrap();

        let findings = detect(dir.path()).unwrap();
        let check = &findings[0].proposed_checks[0];
        assert!(
            matches!(&check.source, ProposedCheckSource::Shell { command } if command == "yarn lint-staged"),
            "expected yarn lint-staged, got {:?}",
            check.source
        );
    }

    #[test]
    fn standalone_lintstagedrc_json_detected() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".lintstagedrc.json"), r#"{"*.ts": "eslint"}"#).unwrap();

        let findings = detect(dir.path()).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].source_path,
            dir.path().join(".lintstagedrc.json")
        );
    }

    #[test]
    fn standalone_lintstagedrc_bare_detected() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".lintstagedrc"), r#"{"*.ts": "eslint"}"#).unwrap();

        let findings = detect(dir.path()).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].source_path, dir.path().join(".lintstagedrc"));
    }

    #[test]
    fn standalone_takes_priority_over_package_json() {
        let dir = tempfile::tempdir().unwrap();
        // Both a standalone file and package.json present.
        fs::write(dir.path().join(".lintstagedrc.json"), r#"{"*.ts": "eslint"}"#).unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{ "lint-staged": {} }"#,
        )
        .unwrap();

        let findings = detect(dir.path()).unwrap();
        assert_eq!(findings.len(), 1, "must not produce two findings");
        assert_eq!(
            findings[0].source_path,
            dir.path().join(".lintstagedrc.json"),
            "standalone file must take priority over package.json"
        );
    }

    #[test]
    fn duplicate_warning_fires_when_husky_hook_references_lint_staged() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{ "lint-staged": {} }"#,
        )
        .unwrap();

        let husky_dir = dir.path().join(".husky");
        fs::create_dir_all(&husky_dir).unwrap();
        fs::write(
            husky_dir.join("pre-commit"),
            "#!/bin/sh\nnpx lint-staged\n",
        )
        .unwrap();

        let findings = detect(dir.path()).unwrap();
        assert!(!findings[0].warnings.is_empty(), "should warn about duplicate execution");
        assert!(
            findings[0].warnings[0].contains("twice") || findings[0].warnings[0].contains("duplicate"),
            "warning should mention duplicate execution: {}",
            findings[0].warnings[0]
        );
    }

    #[test]
    fn no_duplicate_warning_when_husky_hook_does_not_reference_lint_staged() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{ "lint-staged": {} }"#,
        )
        .unwrap();

        let husky_dir = dir.path().join(".husky");
        fs::create_dir_all(&husky_dir).unwrap();
        fs::write(husky_dir.join("pre-commit"), "#!/bin/sh\nnpm test\n").unwrap();

        let findings = detect(dir.path()).unwrap();
        assert!(
            findings[0].warnings.is_empty(),
            "no warning when Husky hook does not run lint-staged"
        );
    }

    #[test]
    fn no_duplicate_warning_when_husky_hook_absent() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{ "lint-staged": {} }"#,
        )
        .unwrap();
        // No .husky directory.

        let findings = detect(dir.path()).unwrap();
        assert!(findings[0].warnings.is_empty());
    }

    #[test]
    fn package_json_without_lint_staged_key_yields_no_findings() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{ "scripts": { "test": "jest" } }"#,
        )
        .unwrap();

        let findings = detect(dir.path()).unwrap();
        assert!(findings.is_empty());
    }

    // Edge-case: key present as a string value rather than an object key.
    // e.g. `"description": "uses lint-staged"` — should NOT match.
    #[test]
    fn does_not_false_positive_on_value_containing_lint_staged_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{ "description": "This project uses lint-staged for formatting." }"#,
        )
        .unwrap();

        let findings = detect(dir.path()).unwrap();
        assert!(
            findings.is_empty(),
            "should not match when lint-staged appears only in a value, not as a key"
        );
    }

    #[test]
    fn proposed_check_name_and_triggers() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{ "lint-staged": {} }"#,
        )
        .unwrap();

        let findings = detect(dir.path()).unwrap();
        let check = &findings[0].proposed_checks[0];
        assert_eq!(check.name, "lint-staged");
        assert_eq!(check.triggers, vec![TriggerKind::Commit]);
        assert_eq!(check.timeout_secs, 120);
    }
}
