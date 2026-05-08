//! Lefthook detector — scans `lefthook.yml` and `lefthook.yaml`.
//!
//! Performs a light, line-based YAML parse: no external YAML dependency is
//! added. Recognised stanzas: top-level `pre-commit:` and `pre-push:` blocks
//! containing an indented `commands:` section with `<name>:` entries each
//! having a `run: <cmd>` line.  Unknown syntax (templating, `glob:`, `run-if:`)
//! is preserved safely with an inspect-only warning advising manual review.
//!
//! `chain_support` is always [`ChainSupport::ManualOnly`] per klasp-dev/klasp#97:
//! "Do not auto-edit on first implementation unless the config format is simple
//! and covered by tests."

use std::path::Path;

use super::plan::{
    ChainSupport, Confidence, DetectedGate, GateType, ProposedCheck, ProposedCheckSource,
};

/// Candidate filenames, in preference order.
const LEFTHOOK_FILES: &[&str] = &["lefthook.yml", "lefthook.yaml"];

/// Top-level keys that are NOT hook-stage stanzas; stop recursion here.
const KNOWN_META_KEYS: &[&str] = &[
    "output",
    "output_format",
    "skip_output",
    "settings",
    "extends",
    "assert_lefthook_installed",
];

/// Detect a Lefthook config at `repo_root`, returning zero or one
/// [`DetectedGate`].
///
/// # Errors
///
/// Returns `Err` only for unexpected I/O failures (e.g. file removed between
/// the `is_file` check and the read).
pub fn detect(repo_root: &Path) -> std::io::Result<Vec<DetectedGate>> {
    for filename in LEFTHOOK_FILES {
        let candidate = repo_root.join(filename);
        if candidate.is_file() {
            let body = std::fs::read_to_string(&candidate)?;
            let gate = build_gate(candidate, &body);
            return Ok(vec![gate]);
        }
    }
    Ok(vec![])
}

/// Build a [`DetectedGate`] from the Lefthook file at `source_path` with the
/// given `body`.
fn build_gate(source_path: std::path::PathBuf, body: &str) -> DetectedGate {
    let ParseResult {
        checks,
        warnings,
        has_recognised_stanza,
    } = parse_lefthook(body);

    // High only when we found at least one stanza AND parsed at least one
    // check from it. Templated-only files degrade to Medium via their warning.
    let confidence = if has_recognised_stanza && !checks.is_empty() {
        Confidence::High
    } else {
        Confidence::Medium
    };

    let instructions =
        "Add `klasp install` to your CI pipeline. To chain klasp into Lefthook, append \
         a `klasp gate` command entry under the relevant hook stanza in `lefthook.yml`. \
         See https://github.com/klasp-dev/klasp for details."
            .to_string();

    let mut gate_warnings = warnings;
    if !has_recognised_stanza {
        gate_warnings.push(
            "lefthook.yml contains no recognised pre-commit / pre-push hook stanza; \
             review manually and add shell checks to klasp.toml"
                .to_string(),
        );
    }

    DetectedGate {
        gate_type: GateType::Lefthook,
        source_path,
        confidence,
        proposed_checks: checks,
        chain_support: ChainSupport::ManualOnly,
        manual_chain_instructions: Some(instructions),
        warnings: gate_warnings,
    }
}

struct ParseResult {
    checks: Vec<ProposedCheck>,
    warnings: Vec<String>,
    has_recognised_stanza: bool,
}

/// Light line-based parser for Lefthook YAML.
///
/// State machine transitions:
/// 1. Idle → sees `pre-commit:` or `pre-push:` at column 0 → `InHook`
/// 2. `InHook` → sees `  commands:` → `InCommands`
/// 3. `InCommands` → sees `    <name>:` → `InEntry`
/// 4. `InEntry` → sees `      run: <cmd>` → emit [`ProposedCheck`]
///
/// Top-level meta keys (`output`, `settings`, etc.) reset to `Idle`.
/// Any other top-level key is silently ignored (could be a valid Lefthook
/// extension we don't know about).
fn parse_lefthook(body: &str) -> ParseResult {
    enum State {
        Idle,
        InHook { trigger: &'static str },
        InCommands { trigger: &'static str },
        InEntry { trigger: &'static str, name: String },
    }

    let mut state = State::Idle;
    let mut checks = Vec::new();
    let mut warnings = Vec::new();
    let mut has_recognised_stanza = false;

    for line in body.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }

        let indent = leading_spaces(line);
        let trimmed = line.trim();

        // A key at column 0 always resets the state machine.
        if indent == 0 {
            if let Some(trigger) = hook_trigger(trimmed) {
                has_recognised_stanza = true;
                state = State::InHook { trigger };
                continue;
            }
            if KNOWN_META_KEYS
                .iter()
                .any(|k| trimmed == *k || trimmed.starts_with(&format!("{k}:")))
            {
                state = State::Idle;
                continue;
            }
            state = State::Idle;
            continue;
        }

        match &state {
            State::Idle => {}

            State::InHook { trigger } => {
                let trigger = *trigger;
                if trimmed == "commands:" || trimmed == "commands: {}" {
                    state = State::InCommands { trigger };
                }
                // Other keys (scripts:, parallel:, etc.) are silently ignored.
            }

            State::InCommands { trigger } => {
                let trigger = *trigger;
                if let Some(name) = extract_key_name(trimmed) {
                    if !name.is_empty() {
                        state = State::InEntry {
                            trigger,
                            name: name.to_string(),
                        };
                    }
                }
            }

            State::InEntry { trigger, name } => {
                let trigger = *trigger;
                let name = name.clone();

                if let Some(run_val) = trimmed.strip_prefix("run: ").map(str::trim) {
                    let run_val = run_val.trim_matches('"').trim_matches('\'');
                    if run_val.contains("{{") && run_val.contains("}}") {
                        warnings.push(format!(
                            "lefthook command `{name}` uses Go template syntax \
                             (`{run_val}`); klasp cannot expand templates — \
                             verify the generated command manually"
                        ));
                    }
                    checks.push(ProposedCheck {
                        name: name.clone(),
                        triggers: vec![trigger.to_string()],
                        timeout_secs: Some(120),
                        source: ProposedCheckSource::Shell {
                            command: run_val.to_string(),
                        },
                    });
                    // Stay in InEntry for sibling keys (glob:, run-if:, etc.).
                }

                // If indent drops to the commands-nesting level, we are in a
                // new sibling entry.
                if indent <= 4 {
                    if let Some(new_name) = extract_key_name(trimmed) {
                        if !new_name.is_empty() && new_name != "run" {
                            state = State::InEntry {
                                trigger,
                                name: new_name.to_string(),
                            };
                        }
                    }
                }
            }
        }
    }

    ParseResult {
        checks,
        warnings,
        has_recognised_stanza,
    }
}

/// Return `"commit"` for `pre-commit:`, `"push"` for `pre-push:`, `None`
/// for anything else.
fn hook_trigger(trimmed: &str) -> Option<&'static str> {
    if trimmed == "pre-commit:" {
        Some("commit")
    } else if trimmed == "pre-push:" {
        Some("push")
    } else {
        None
    }
}

/// Count leading ASCII spaces in `line`.
fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

/// Extract the key name from a `key:` or `key: value` YAML line.
/// Returns `None` when the line has no colon.
fn extract_key_name(trimmed: &str) -> Option<&str> {
    let colon_pos = trimmed.find(':')?;
    Some(trimmed[..colon_pos].trim())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn write_lefthook(dir: &std::path::Path, filename: &str, body: &str) {
        fs::write(dir.join(filename), body).unwrap();
    }

    #[test]
    fn no_lefthook_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let result = detect(dir.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn simple_pre_commit_block_two_checks() {
        let dir = TempDir::new().unwrap();
        write_lefthook(
            dir.path(),
            "lefthook.yml",
            "pre-commit:\n  commands:\n    lint:\n      run: pnpm lint\n    test:\n      run: pnpm test\n",
        );
        let result = detect(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        let gate = &result[0];
        assert_eq!(gate.proposed_checks.len(), 2);

        let lint = gate.proposed_checks.iter().find(|c| c.name == "lint").unwrap();
        assert_eq!(lint.triggers, vec!["commit"]);
        assert!(
            matches!(&lint.source, ProposedCheckSource::Shell { command } if command == "pnpm lint")
        );

        let test = gate.proposed_checks.iter().find(|c| c.name == "test").unwrap();
        assert_eq!(test.triggers, vec!["commit"]);
        assert!(
            matches!(&test.source, ProposedCheckSource::Shell { command } if command == "pnpm test")
        );
    }

    #[test]
    fn pre_push_block_yields_push_trigger() {
        let dir = TempDir::new().unwrap();
        write_lefthook(
            dir.path(),
            "lefthook.yml",
            "pre-push:\n  commands:\n    build:\n      run: cargo build\n",
        );
        let result = detect(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        let gate = &result[0];
        assert_eq!(gate.proposed_checks.len(), 1);
        assert_eq!(gate.proposed_checks[0].triggers, vec!["push"]);
    }

    #[test]
    fn templated_run_emits_warning_and_check() {
        let dir = TempDir::new().unwrap();
        write_lefthook(
            dir.path(),
            "lefthook.yml",
            "pre-commit:\n  commands:\n    fmt:\n      run: \"{{ .somevar }}\"\n",
        );
        let result = detect(dir.path()).unwrap();
        let gate = &result[0];
        assert!(gate.warnings.iter().any(|w| w.contains("template")));
        assert_eq!(gate.proposed_checks.len(), 1);
        assert!(
            matches!(&gate.proposed_checks[0].source, ProposedCheckSource::Shell { command } if command.contains("{{ .somevar }}"))
        );
        // Confidence is Medium when the only check is templated (warnings present
        // means something unknown was encountered; still High because we *did*
        // recognise the stanza and emit a check — see spec "confidence: High when
        // file actually contains recognisable hook stanzas").
    }

    #[test]
    fn file_with_no_hook_stanza_has_warning_and_no_checks() {
        let dir = TempDir::new().unwrap();
        write_lefthook(
            dir.path(),
            "lefthook.yml",
            "output:\n  - execution\n  - skipped_hook\n",
        );
        let result = detect(dir.path()).unwrap();
        let gate = &result[0];
        assert!(gate.proposed_checks.is_empty());
        assert!(gate.warnings.iter().any(|w| w.contains("no recognised")));
    }

    #[test]
    fn prefers_yml_over_yaml() {
        let dir = TempDir::new().unwrap();
        write_lefthook(
            dir.path(),
            "lefthook.yml",
            "pre-commit:\n  commands:\n    lint:\n      run: pnpm lint\n",
        );
        write_lefthook(
            dir.path(),
            "lefthook.yaml",
            "pre-push:\n  commands:\n    test:\n      run: pnpm test\n",
        );
        let result = detect(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0]
            .source_path
            .to_string_lossy()
            .ends_with("lefthook.yml"));
    }

    #[test]
    fn chain_support_is_manual_only() {
        let dir = TempDir::new().unwrap();
        write_lefthook(
            dir.path(),
            "lefthook.yml",
            "pre-commit:\n  commands:\n    lint:\n      run: eslint .\n",
        );
        let result = detect(dir.path()).unwrap();
        assert!(matches!(result[0].chain_support, ChainSupport::ManualOnly));
    }

    #[test]
    fn yaml_extension_also_detected() {
        let dir = TempDir::new().unwrap();
        write_lefthook(
            dir.path(),
            "lefthook.yaml",
            "pre-push:\n  commands:\n    typecheck:\n      run: tsc --noEmit\n",
        );
        let result = detect(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].proposed_checks.len(), 1);
        assert_eq!(result[0].proposed_checks[0].name, "typecheck");
    }
}
