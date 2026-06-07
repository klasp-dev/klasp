//! `ClaudeCodeSurface` — `klasp_core::AgentSurface` impl for Claude Code.
//!
//! Implements the install flow described in [docs/design.md §5]:
//!
//! 1. Compute paths.
//! 2. Render the hook script.
//! 3. Idempotency: managed-marker presence in an existing hook file.
//! 4. Honour `--dry-run` (preview only, no writes).
//! 5. Atomic write of the script + chmod 0o755 (Unix).
//! 6. Surgical merge into `.claude/settings.json`.
//!
//! ## Windows notes (audit W4)
//!
//! On Windows, `current_mode` and `apply_mode` are no-ops — NTFS has no
//! executable permission bit, and `bash.exe` (Git for Windows) interprets
//! the script's `#!/usr/bin/env bash` shebang at runtime regardless. The
//! generated hook script therefore works without any chmod step. Users on
//! Windows must have Git for Windows installed (which puts `bash.exe` on
//! PATH); the default installer satisfies this. All `Path::join` calls in
//! this module produce platform-correct separators via `std::path::Path` —
//! no manual separator handling is required, and `HOOK_COMMAND` uses
//! forward slashes (resolved by Claude Code at hook-invocation time).

use std::fs;
use std::path::{Path, PathBuf};

use klasp_core::fs::{atomic_write, current_mode, ensure_parent, read_or_empty, write_if_changed};
use klasp_core::{AgentSurface, InstallContext, InstallError, InstallReport};

use crate::hook_template::{self, MANAGED_MARKER};
use crate::settings::{self, SettingsError};

/// Claude Code agent surface. Stateless; the registry stores it as
/// `Box<dyn AgentSurface>`.
pub struct ClaudeCodeSurface;

impl ClaudeCodeSurface {
    pub const AGENT_ID: &'static str = "claude_code";

    /// The literal `command` string klasp embeds in `.claude/settings.json`'s
    /// `hooks.PreToolUse[Bash]` matcher. `${CLAUDE_PROJECT_DIR}` is resolved
    /// by Claude Code at hook-execution time, so the same settings.json works
    /// regardless of the CWD Claude is invoked from. See plan: "Hook entry
    /// `command` value" decision.
    pub const HOOK_COMMAND: &'static str = "${CLAUDE_PROJECT_DIR}/.claude/hooks/klasp-gate.sh";
}

impl AgentSurface for ClaudeCodeSurface {
    fn agent_id(&self) -> &'static str {
        Self::AGENT_ID
    }

    fn detect(&self, repo_root: &Path) -> bool {
        repo_root.join(".claude").is_dir()
    }

    fn hook_path(&self, repo_root: &Path) -> PathBuf {
        repo_root
            .join(".claude")
            .join("hooks")
            .join("klasp-gate.sh")
    }

    fn settings_path(&self, repo_root: &Path) -> PathBuf {
        repo_root.join(".claude").join("settings.json")
    }

    fn render_hook_script(&self, ctx: &InstallContext) -> String {
        hook_template::render(ctx.schema_version)
    }

    fn doctor_check(
        &self,
        repo_root: &Path,
        schema_version: u32,
    ) -> Vec<klasp_core::DoctorFinding> {
        use klasp_core::DoctorFinding;
        let mut findings = Vec::new();
        let agent_id = self.agent_id();

        let hook_path = self.hook_path(repo_root);
        let hook_actual = match std::fs::read_to_string(&hook_path) {
            Ok(s) => s,
            Err(_) => {
                findings.push(DoctorFinding::Fail(format!(
                    "hook[{agent_id}]: {} not found; re-run `klasp install`",
                    hook_path.display()
                )));
                return findings;
            }
        };
        let ctx = InstallContext {
            repo_root: repo_root.to_path_buf(),
            dry_run: false,
            force: false,
            schema_version,
        };
        if hook_actual == self.render_hook_script(&ctx) {
            findings.push(DoctorFinding::Ok(format!(
                "hook[{agent_id}]: current (schema v{schema_version})"
            )));
        } else {
            findings.push(DoctorFinding::Fail(format!(
                "hook[{agent_id}]: schema drift detected (re-run `klasp install`)"
            )));
        }

        let settings_path = self.settings_path(repo_root);
        let raw = match std::fs::read_to_string(&settings_path) {
            Ok(s) => s,
            Err(_) => {
                findings.push(DoctorFinding::Fail(format!(
                    "settings[{agent_id}]: {} not found; re-run `klasp install`",
                    settings_path.display()
                )));
                return findings;
            }
        };
        let root: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(e) => {
                findings.push(DoctorFinding::Fail(format!(
                    "settings[{agent_id}]: failed to parse {} as JSON: {e}",
                    settings_path.display()
                )));
                return findings;
            }
        };
        let hook_command = Self::HOOK_COMMAND;
        let has_entry = root
            .get("hooks")
            .and_then(|h| h.get("PreToolUse"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|arr| {
                arr.iter().any(|matcher_entry| {
                    matcher_entry
                        .get("matcher")
                        .and_then(serde_json::Value::as_str)
                        == Some("Bash")
                        && matcher_entry
                            .get("hooks")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|inner| {
                                inner.iter().any(|hook| {
                                    hook.get("command").and_then(serde_json::Value::as_str)
                                        == Some(hook_command)
                                })
                            })
                })
            });
        if has_entry {
            findings.push(DoctorFinding::Ok(format!(
                "settings[{agent_id}]: hook entry present"
            )));
        } else {
            findings.push(DoctorFinding::Fail(format!(
                "settings[{agent_id}]: klasp hook entry missing; re-run `klasp install`"
            )));
        }

        findings
    }

    fn install(&self, ctx: &InstallContext) -> Result<InstallReport, InstallError> {
        let hook_path = self.hook_path(&ctx.repo_root);
        let settings_path = self.settings_path(&ctx.repo_root);
        let rendered = self.render_hook_script(ctx);

        let hook_state = inspect_hook_file(&hook_path, &rendered, ctx.force)?;

        let settings_input = read_or_empty(&settings_path)?;
        let merged = settings::merge_hook_entry(&settings_input, Self::HOOK_COMMAND)
            .map_err(|e| settings_error(&settings_path, e))?;
        let settings_unchanged = merged == settings_input;

        let already_installed = matches!(hook_state, HookState::Identical) && settings_unchanged;

        if ctx.dry_run {
            return Ok(InstallReport {
                agent_id: Self::AGENT_ID.to_string(),
                hook_path,
                settings_path,
                already_installed,
                paths_written: Vec::new(),
                preview: Some(rendered),
            });
        }

        let mut paths_written = Vec::new();

        if !matches!(hook_state, HookState::Identical) {
            ensure_parent(&hook_path)?;
            atomic_write(&hook_path, rendered.as_bytes(), 0o755)?;
            paths_written.push(hook_path.clone());
        }

        if write_if_changed(&settings_path, &settings_input, &merged)? {
            paths_written.push(settings_path.clone());
        }

        Ok(InstallReport {
            agent_id: Self::AGENT_ID.to_string(),
            hook_path,
            settings_path,
            already_installed,
            paths_written,
            preview: None,
        })
    }

    fn uninstall(&self, repo_root: &Path, dry_run: bool) -> Result<Vec<PathBuf>, InstallError> {
        let hook_path = self.hook_path(repo_root);
        let settings_path = self.settings_path(repo_root);
        let mut paths = Vec::new();

        if hook_path.exists() {
            let existing = fs::read_to_string(&hook_path).map_err(|e| InstallError::Io {
                path: hook_path.clone(),
                source: e,
            })?;
            if existing.contains(MANAGED_MARKER) {
                if !dry_run {
                    fs::remove_file(&hook_path).map_err(|e| InstallError::Io {
                        path: hook_path.clone(),
                        source: e,
                    })?;
                }
                paths.push(hook_path);
            }
        }

        if settings_path.exists() {
            let existing = fs::read_to_string(&settings_path).map_err(|e| InstallError::Io {
                path: settings_path.clone(),
                source: e,
            })?;
            let new = settings::unmerge_hook_entry(&existing, Self::HOOK_COMMAND)
                .map_err(|e| settings_error(&settings_path, e))?;
            if new != existing {
                if !dry_run {
                    let mode = current_mode(&settings_path).unwrap_or(0o644);
                    atomic_write(&settings_path, new.as_bytes(), mode)?;
                }
                paths.push(settings_path);
            }
        }

        Ok(paths)
    }
}

enum HookState {
    Identical,
    Writable,
}

fn inspect_hook_file(
    hook_path: &Path,
    rendered: &str,
    force: bool,
) -> Result<HookState, InstallError> {
    if !hook_path.exists() {
        return Ok(HookState::Writable);
    }
    let existing = fs::read_to_string(hook_path).map_err(|e| InstallError::Io {
        path: hook_path.to_path_buf(),
        source: e,
    })?;
    if existing.contains(MANAGED_MARKER) {
        if existing == rendered {
            Ok(HookState::Identical)
        } else {
            Ok(HookState::Writable)
        }
    } else if force {
        Ok(HookState::Writable)
    } else {
        Err(InstallError::MarkerConflict {
            path: hook_path.to_path_buf(),
        })
    }
}

fn settings_error(path: &Path, error: SettingsError) -> InstallError {
    match error {
        SettingsError::Parse(source) => InstallError::SettingsParse {
            path: path.to_path_buf(),
            source,
        },
        shape @ SettingsError::Shape { .. } => InstallError::Surface {
            agent_id: ClaudeCodeSurface::AGENT_ID.to_string(),
            message: shape.to_string(),
        },
    }
}
