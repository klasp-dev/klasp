//! Husky detector — scans `.husky/pre-commit` and `.husky/pre-push`.
//!
//! Each present file produces one [`DetectedGate`] with:
//! - `gate_type`: [`GateType::Husky`] carrying the hook stage
//! - `proposed_checks`: one [`ProposedCheck`] per substantive command found in
//!   the script body (multi-line hook bodies produce multiple checks)
//! - `chain_support`: always [`ChainSupport::ManualOnly`] (v1 never auto-edits
//!   `.husky/*`)
//!
//! See klasp-dev/klasp#97.

use std::io;
use std::path::Path;

use super::detect::hook_to_trigger;
use super::detect_lint_staged::package_json_has_lint_staged;
use super::plan::{
    ChainSupport, DetectedGate, GateType, HookStage, ProposedCheck, ProposedCheckSource,
};

/// Hooks this detector examines, in order.
const HUSKY_HOOKS: &[(HookStage, &str)] =
    &[(HookStage::PreCommit, "pre-commit"), (HookStage::PrePush, "pre-push")];

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
pub fn detect(repo_root: &Path) -> io::Result<Vec<DetectedGate>> {
    let husky_dir = repo_root.join(".husky");
    if !husky_dir.is_dir() {
        return Ok(vec![]);
    }

    let mut findings = Vec::new();
    for (stage, name) in HUSKY_HOOKS {
        let hook_path = husky_dir.join(name);
        let body = match std::fs::read_to_string(&hook_path) {
            Ok(s) => s,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        findings.push(build_gate(*stage, name, hook_path, &body, repo_root));
    }
    Ok(findings)
}

/// Build a [`DetectedGate`] for a single Husky hook file.
fn build_gate(
    stage: HookStage,
    hook_name: &str,
    source_path: std::path::PathBuf,
    body: &str,
    repo_root: &Path,
) -> DetectedGate {
    let trigger = hook_to_trigger(stage);
    let cmds = substantive_commands(body);

    let (proposed_checks, warnings) =
        build_proposed_checks(hook_name, &cmds, trigger, body, repo_root);

    let instructions = format!(
        "Add `klasp install` to your CI pipeline and wire klasp manually into \
         `.husky/{hook_name}` by appending `klasp gate` after your existing commands. \
         See https://github.com/klasp-dev/klasp for details."
    );

    DetectedGate {
        gate_type: GateType::Husky { hook: stage },
        source_path,
        proposed_checks,
        chain_support: ChainSupport::ManualOnly,
        manual_chain_instructions: Some(instructions),
        warnings,
    }
}

/// Extract all non-comment, non-empty, non-bookkeeping lines from the hook
/// script body. Returns every substantive line in order so that multi-line
/// hook bodies (e.g. `pnpm lint\npnpm test`) produce one check per command.
/// Returns an empty `Vec` when no substantive lines are found.
fn substantive_commands(body: &str) -> Vec<String> {
    let mut cmds = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if is_bookkeeping(trimmed) {
            continue;
        }
        cmds.push(trimmed.to_string());
    }
    cmds
}

/// True when the line is Husky-internal scaffolding that should be ignored
/// when looking for user-authored commands.
fn is_bookkeeping(line: &str) -> bool {
    HUSKY_BOOKKEEPING_PREFIXES
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

/// Recognised patterns → `(name, timeout_secs)`.
/// Returns `None` when the command is not in the recognised set.
fn classify_command(cmd: &str) -> Option<(&'static str, u64)> {
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
        return Some(("lint-staged", 120));
    }
    // test
    if matches!(cmd, "npm test" | "pnpm test" | "yarn test" | "pnpm run test")
        || cmd.starts_with("npm test ")
        || cmd.starts_with("pnpm test ")
        || cmd.starts_with("yarn test ")
    {
        return Some(("test", 180));
    }
    // lint
    if matches!(
        cmd,
        "pnpm lint" | "npm run lint" | "yarn lint" | "pnpm run lint"
    ) || cmd.starts_with("pnpm lint ")
        || cmd.starts_with("npm run lint ")
        || cmd.starts_with("yarn lint ")
    {
        return Some(("lint", 120));
    }
    // cargo
    if cmd.starts_with("cargo ") {
        return Some(("cargo", 300));
    }
    // pytest
    if cmd.starts_with("pytest") || cmd.starts_with("python -m pytest") {
        return Some(("pytest", 180));
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

/// Build the `proposed_checks` and `warnings` for a Husky gate finding.
///
/// Emits one [`ProposedCheck`] per entry in `cmds` so that multi-line hook
/// bodies (e.g. `pnpm lint\npnpm test`) produce one check per command.
/// Each check inherits the same `trigger` and uses `timeout_secs = 120` unless
/// the command is a recognised pattern with its own timeout.
fn build_proposed_checks(
    hook: &str,
    cmds: &[String],
    trigger: super::plan::TriggerKind,
    body: &str,
    repo_root: &Path,
) -> (Vec<ProposedCheck>, Vec<String>) {
    let mut warnings = Vec::new();

    if cmds.is_empty() {
        warnings.push(format!(
            "Husky {hook} hook is empty; no checks proposed"
        ));
        return (vec![], warnings);
    }

    // Duplicate-execution warning: lint-staged in hook body AND in package.json.
    let references_lint_staged = body.contains("lint-staged");
    if references_lint_staged {
        let pkg_contents = std::fs::read_to_string(repo_root.join("package.json"))
            .unwrap_or_default();
        if package_json_has_lint_staged(&pkg_contents) {
            warnings.push(
                "klasp's lint-staged check will overlap with Husky's pre-commit hook; \
                 both run lint-staged at commit time — consider removing one"
                    .to_string(),
            );
        }
    }

    let mut checks = Vec::new();
    for cmd in cmds {
        let (name, timeout_secs) = if let Some((n, t)) = classify_command(cmd) {
            (n.to_string(), t)
        } else {
            (derive_name_from_command(cmd, hook), 120)
        };
        checks.push(ProposedCheck {
            name,
            triggers: vec![trigger],
            timeout_secs,
            source: ProposedCheckSource::Shell {
                command: cmd.clone(),
            },
        });
    }

    (checks, warnings)
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
        assert!(matches!(&gate.gate_type, GateType::Husky { hook } if *hook == HookStage::PreCommit));
        assert_eq!(gate.proposed_checks.len(), 1);
        let check = &gate.proposed_checks[0];
        assert_eq!(check.name, "lint-staged");
        assert_eq!(check.triggers, vec![super::super::plan::TriggerKind::Commit]);
        assert!(matches!(&check.source, ProposedCheckSource::Shell { command } if command == "npx lint-staged"));
    }

    #[test]
    fn pre_push_with_pnpm_test() {
        let dir = TempDir::new().unwrap();
        write_hook(dir.path(), "pre-push", "#!/bin/sh\npnpm test\n");
        let result = detect(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        let gate = &result[0];
        assert!(matches!(&gate.gate_type, GateType::Husky { hook } if *hook == HookStage::PrePush));
        let check = &gate.proposed_checks[0];
        assert_eq!(check.name, "test");
        assert_eq!(check.triggers, vec![super::super::plan::TriggerKind::Push]);
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
    fn unrecognised_command_yields_one_check_with_shell_source() {
        let dir = TempDir::new().unwrap();
        write_hook(dir.path(), "pre-commit", "#!/bin/sh\nmycustomlinter --fix\n");
        let result = detect(dir.path()).unwrap();
        let gate = &result[0];
        assert_eq!(gate.proposed_checks.len(), 1);
        let check = &gate.proposed_checks[0];
        assert_eq!(check.name, "mycustomlinter");
        assert!(matches!(&check.source, ProposedCheckSource::Shell { command } if command == "mycustomlinter --fix"));
    }

    #[test]
    fn recognised_command_has_correct_timeout() {
        let dir = TempDir::new().unwrap();
        write_hook(dir.path(), "pre-commit", "#!/bin/sh\nnpm run lint\n");
        let result = detect(dir.path()).unwrap();
        assert_eq!(result[0].proposed_checks[0].timeout_secs, 120);
    }

    #[test]
    fn chain_support_is_always_manual_only() {
        let dir = TempDir::new().unwrap();
        write_hook(dir.path(), "pre-commit", "#!/bin/sh\npnpm test\n");
        let result = detect(dir.path()).unwrap();
        assert!(matches!(result[0].chain_support, ChainSupport::ManualOnly));
    }

    #[test]
    fn multi_line_pnpm_lint_and_pnpm_test_produces_two_checks() {
        let dir = TempDir::new().unwrap();
        write_hook(
            dir.path(),
            "pre-commit",
            "#!/bin/sh\npnpm lint\npnpm test\n",
        );
        let result = detect(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        let gate = &result[0];
        assert_eq!(
            gate.proposed_checks.len(),
            2,
            "expected 2 checks for 2-line body, got: {:?}",
            gate.proposed_checks.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
        assert_eq!(gate.proposed_checks[0].name, "lint");
        assert_eq!(gate.proposed_checks[1].name, "test");
        // Both checks share the same trigger.
        assert_eq!(
            gate.proposed_checks[0].triggers,
            vec![super::super::plan::TriggerKind::Commit]
        );
        assert_eq!(
            gate.proposed_checks[1].triggers,
            vec![super::super::plan::TriggerKind::Commit]
        );
    }

    #[test]
    fn multi_line_lint_staged_and_pnpm_test_produces_two_named_checks() {
        let dir = TempDir::new().unwrap();
        write_hook(
            dir.path(),
            "pre-commit",
            "#!/bin/sh\nnpx lint-staged\npnpm test\n",
        );
        let result = detect(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        let gate = &result[0];
        assert_eq!(
            gate.proposed_checks.len(),
            2,
            "expected 2 checks, got: {:?}",
            gate.proposed_checks.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
        assert_eq!(gate.proposed_checks[0].name, "lint-staged");
        assert_eq!(gate.proposed_checks[1].name, "test");
    }
}
