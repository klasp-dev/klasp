//! Husky detector — scans `.husky/pre-commit` and `.husky/pre-push`.
//!
//! Each present file produces one [`DetectedGate`] with:
//! - `gate_type`: [`GateType::Husky`] carrying the hook name
//! - `confidence`: High for recognised invocation patterns, Medium otherwise
//! - `proposed_checks`: derived from the first substantive command in the script
//! - `chain_support`: always [`ChainSupport::ManualOnly`] (v1 never auto-edits
//!   `.husky/*`)
//!
//! See klasp-dev/klasp#97.

use std::path::Path;

use super::plan::{
    ChainSupport, Confidence, DetectedGate, GateType, ProposedCheck, ProposedCheckSource,
};

/// Hooks this detector examines, in order.
const HUSKY_HOOKS: &[&str] = &["pre-commit", "pre-push"];

/// Patterns that indicate Husky internal bookkeeping — these lines are skipped
/// when extracting the first substantive command from a hook script.
const HUSKY_BOOKKEEPING_PREFIXES: &[&str] = &[
    "#!/",
    ". \"$(dirname -- \"$0\")/_/husky.sh\"",
    ". \"$(dirname $0)/_/husky.sh\"",
    "husky_skip_init=",
    "[ -z \"$husky\"",
    "export HUSKY",
    ". ~/.huskyrc",
    ". ~/",
];

/// Run detection against `repo_root`, returning one [`DetectedGate`] per
/// `.husky/<hook>` file that is present, regardless of content.
pub fn detect(repo_root: &Path) -> std::io::Result<Vec<DetectedGate>> {
    let husky_dir = repo_root.join(".husky");
    if !husky_dir.is_dir() {
        return Ok(vec![]);
    }

    let mut findings = Vec::new();
    for hook in HUSKY_HOOKS {
        let hook_path = husky_dir.join(hook);
        if hook_path.is_file() {
            let body = std::fs::read_to_string(&hook_path)?;
            findings.push(build_gate(hook, hook_path, &body, repo_root));
        }
    }
    Ok(findings)
}

/// Build a [`DetectedGate`] for a single Husky hook file.
fn build_gate(
    hook: &str,
    source_path: std::path::PathBuf,
    body: &str,
    repo_root: &Path,
) -> DetectedGate {
    let trigger = hook_to_trigger(hook);
    let first_cmd = first_substantive_command(body);

    let (proposed_checks, confidence, warnings) =
        build_proposed_checks(hook, first_cmd.as_deref(), trigger, body, repo_root);

    let instructions = format!(
        "Add `klasp install` to your CI pipeline and wire klasp manually into \
         `.husky/{hook}` by appending `klasp gate` after your existing commands. \
         See https://github.com/klasp-dev/klasp for details."
    );

    DetectedGate {
        gate_type: GateType::Husky {
            hook: hook.to_string(),
        },
        source_path,
        confidence,
        proposed_checks,
        chain_support: ChainSupport::ManualOnly,
        manual_chain_instructions: Some(instructions),
        warnings,
    }
}

/// Return `"commit"` for `pre-commit`, `"push"` for `pre-push`.
fn hook_to_trigger(hook: &str) -> &'static str {
    if hook.contains("push") {
        "push"
    } else {
        "commit"
    }
}

/// Extract the first non-comment, non-empty, non-bookkeeping line from the
/// hook script body. Returns `None` if no such line is found.
fn first_substantive_command(body: &str) -> Option<String> {
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if is_bookkeeping(trimmed) {
            continue;
        }
        return Some(trimmed.to_string());
    }
    None
}

/// True when the line is Husky-internal scaffolding that should be ignored
/// when looking for user-authored commands.
fn is_bookkeeping(line: &str) -> bool {
    HUSKY_BOOKKEEPING_PREFIXES
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

/// Recognised patterns → `(name, timeout_secs, confidence)`.
/// Returns `None` when the command is not in the recognised set.
fn classify_command(cmd: &str) -> Option<(&'static str, u64, Confidence)> {
    // lint-staged
    if matches!(
        cmd,
        "npx lint-staged"
            | "npx lint-staged --"
            | "pnpm exec lint-staged"
            | "yarn lint-staged"
            | "pnpm lint-staged"
    ) || cmd.starts_with("npx lint-staged ")
        || cmd.starts_with("pnpm exec lint-staged ")
        || cmd.starts_with("yarn lint-staged ")
    {
        return Some(("lint-staged", 120, Confidence::High));
    }
    // test
    if matches!(cmd, "npm test" | "pnpm test" | "yarn test" | "pnpm run test")
        || cmd.starts_with("npm test ")
        || cmd.starts_with("pnpm test ")
        || cmd.starts_with("yarn test ")
    {
        return Some(("test", 180, Confidence::High));
    }
    // lint
    if matches!(
        cmd,
        "pnpm lint" | "npm run lint" | "yarn lint" | "pnpm run lint"
    ) || cmd.starts_with("pnpm lint ")
        || cmd.starts_with("npm run lint ")
        || cmd.starts_with("yarn lint ")
    {
        return Some(("lint", 120, Confidence::High));
    }
    // cargo
    if cmd.starts_with("cargo ") {
        return Some(("cargo", 300, Confidence::High));
    }
    // pytest
    if cmd.starts_with("pytest") || cmd.starts_with("python -m pytest") {
        return Some(("pytest", 180, Confidence::High));
    }
    None
}

/// Derive a check name from the first argument of an unrecognised command.
/// Falls back to `"husky-<hook>"` when the command has no usable path segment.
fn derive_name_from_command(cmd: &str, hook: &str) -> String {
    let first = cmd.split_whitespace().next().unwrap_or("");
    let segment = first.rsplit('/').next().unwrap_or(first);
    if segment.is_empty() || segment == first && first.is_empty() {
        return format!("husky-{hook}");
    }
    // Sanitise: keep only alphanumeric, `-`, `_`.
    let sanitised: String = segment
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    if sanitised.trim_matches('-').is_empty() {
        format!("husky-{hook}")
    } else {
        sanitised
    }
}

/// Check whether `package.json` at `repo_root` contains a `"lint-staged"` key.
fn package_json_has_lint_staged(repo_root: &Path) -> bool {
    let pkg_path = repo_root.join("package.json");
    std::fs::read_to_string(pkg_path)
        .map(|s| s.contains("\"lint-staged\""))
        .unwrap_or(false)
}

/// Build the `proposed_checks`, inferred `confidence`, and `warnings` for a
/// Husky gate finding.
fn build_proposed_checks(
    hook: &str,
    first_cmd: Option<&str>,
    trigger: &str,
    body: &str,
    repo_root: &Path,
) -> (Vec<ProposedCheck>, Confidence, Vec<String>) {
    let mut warnings = Vec::new();

    let Some(cmd) = first_cmd else {
        warnings.push(format!(
            "Husky {hook} hook is empty; no checks proposed"
        ));
        return (vec![], Confidence::Medium, warnings);
    };

    let (name, timeout_secs, confidence, source) =
        if let Some((n, t, conf)) = classify_command(cmd) {
            (
                n.to_string(),
                t,
                conf,
                ProposedCheckSource::Shell {
                    command: cmd.to_string(),
                },
            )
        } else {
            let n = derive_name_from_command(cmd, hook);
            (
                n,
                120,
                Confidence::Medium,
                ProposedCheckSource::Shell {
                    command: cmd.to_string(),
                },
            )
        };

    // Duplicate-execution warning: lint-staged in hook body AND in package.json.
    let references_lint_staged = body.contains("lint-staged");
    if references_lint_staged && package_json_has_lint_staged(repo_root) {
        warnings.push(
            "klasp's lint-staged check will overlap with Husky's pre-commit hook; \
             both run lint-staged at commit time — consider removing one"
                .to_string(),
        );
    }

    let check = ProposedCheck {
        name,
        triggers: vec![trigger.to_string()],
        timeout_secs: Some(timeout_secs),
        source,
    };

    (vec![check], confidence, warnings)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn write_hook(dir: &std::path::Path, hook: &str, body: &str) {
        let husky = dir.join(".husky");
        fs::create_dir_all(&husky).unwrap();
        fs::write(husky.join(hook), body).unwrap();
    }

    #[test]
    fn no_husky_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let result = detect(dir.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn pre_commit_with_npx_lint_staged() {
        let dir = TempDir::new().unwrap();
        write_hook(dir.path(), "pre-commit", "#!/bin/sh\nnpx lint-staged\n");
        let result = detect(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        let gate = &result[0];
        assert!(matches!(&gate.gate_type, GateType::Husky { hook } if hook == "pre-commit"));
        assert_eq!(gate.proposed_checks.len(), 1);
        let check = &gate.proposed_checks[0];
        assert_eq!(check.name, "lint-staged");
        assert_eq!(check.triggers, vec!["commit"]);
        assert!(matches!(&check.source, ProposedCheckSource::Shell { command } if command == "npx lint-staged"));
        assert_eq!(gate.confidence, Confidence::High);
    }

    #[test]
    fn pre_push_with_pnpm_test() {
        let dir = TempDir::new().unwrap();
        write_hook(dir.path(), "pre-push", "#!/bin/sh\npnpm test\n");
        let result = detect(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        let gate = &result[0];
        assert!(matches!(&gate.gate_type, GateType::Husky { hook } if hook == "pre-push"));
        let check = &gate.proposed_checks[0];
        assert_eq!(check.name, "test");
        assert_eq!(check.triggers, vec!["push"]);
        assert_eq!(gate.confidence, Confidence::High);
    }

    #[test]
    fn empty_hook_body_produces_no_checks_and_warning() {
        let dir = TempDir::new().unwrap();
        write_hook(dir.path(), "pre-commit", "#!/bin/sh\n# nothing here\n");
        let result = detect(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        let gate = &result[0];
        assert!(gate.proposed_checks.is_empty());
        assert!(gate.warnings.iter().any(|w| w.contains("empty")));
    }

    #[test]
    fn lint_staged_in_hook_and_package_json_triggers_duplicate_warning() {
        let dir = TempDir::new().unwrap();
        write_hook(dir.path(), "pre-commit", "#!/bin/sh\nnpx lint-staged\n");
        fs::write(
            dir.path().join("package.json"),
            r#"{"lint-staged": {"*.ts": "tsc"}}"#,
        )
        .unwrap();
        let result = detect(dir.path()).unwrap();
        let gate = &result[0];
        assert!(gate.warnings.iter().any(|w| w.contains("overlap")));
    }

    #[test]
    fn only_bookkeeping_comments_produces_no_checks() {
        let dir = TempDir::new().unwrap();
        write_hook(
            dir.path(),
            "pre-commit",
            "#!/bin/sh\n# klasp adopted: ignored\n# another comment\n",
        );
        let result = detect(dir.path()).unwrap();
        let gate = &result[0];
        assert!(gate.proposed_checks.is_empty());
    }

    #[test]
    fn unrecognised_command_yields_medium_confidence() {
        let dir = TempDir::new().unwrap();
        write_hook(dir.path(), "pre-commit", "#!/bin/sh\nmycustomlinter --fix\n");
        let result = detect(dir.path()).unwrap();
        let gate = &result[0];
        assert_eq!(gate.confidence, Confidence::Medium);
        assert_eq!(gate.proposed_checks.len(), 1);
        let check = &gate.proposed_checks[0];
        assert_eq!(check.name, "mycustomlinter");
        assert!(matches!(&check.source, ProposedCheckSource::Shell { command } if command == "mycustomlinter --fix"));
    }

    #[test]
    fn recognised_command_yields_high_confidence() {
        let dir = TempDir::new().unwrap();
        write_hook(dir.path(), "pre-commit", "#!/bin/sh\nnpm run lint\n");
        let result = detect(dir.path()).unwrap();
        assert_eq!(result[0].confidence, Confidence::High);
    }

    #[test]
    fn chain_support_is_always_manual_only() {
        let dir = TempDir::new().unwrap();
        write_hook(dir.path(), "pre-commit", "#!/bin/sh\npnpm test\n");
        let result = detect(dir.path()).unwrap();
        assert!(matches!(result[0].chain_support, ChainSupport::ManualOnly));
    }
}
