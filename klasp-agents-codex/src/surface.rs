//! `CodexSurface` — `klasp_core::AgentSurface` impl for Codex.
//!
//! Mirrors the install flow of [`klasp_agents_claude::ClaudeCodeSurface`]:
//! compute paths → render the managed-block bodies → idempotent
//! merge / replace into the on-disk files → atomic write → report what
//! changed. The two surfaces differ in their target files (Claude writes
//! one bash shim and one JSON settings file; Codex writes AGENTS.md *and*
//! a pair of git hooks) and in their conflict-handling story: Codex has
//! to coexist with husky / lefthook / pre-commit framework, so the hook
//! writer skips-with-warning rather than failing the install.
//!
//! ## v0.2 W2 scope
//!
//! - `install` writes the AGENTS.md managed block (W1 behaviour) **and**
//!   the `.git/hooks/pre-commit` + `.git/hooks/pre-push` hook files.
//! - When a foreign hook manager is detected via
//!   [`git_hooks::detect_conflict`], the hook write is skipped and a
//!   [`HookWarning`] rides alongside the `InstallReport` (returned via
//!   the typed [`CodexSurface::install_detailed`] entry-point — the
//!   plain [`AgentSurface::install`] trait method, which W3 will wire
//!   into the CLI, discards warnings to keep the cross-crate contract
//!   `klasp-core` defines unchanged).
//! - `uninstall` strips the managed block from each managed file and
//!   removes any file klasp owned end-to-end (round-trip from the
//!   missing-file install). Sibling content — both other tools' hooks
//!   and any prose in AGENTS.md — is preserved byte-for-byte.
//!
//! ## Windows notes
//!
//! `AGENTS.md` is plain text. `.git/hooks/pre-commit` and `pre-push` are
//! shell scripts that git itself executes through `sh.exe` (Git for
//! Windows) or whatever the user's git is configured to use; they need
//! a shebang for portability but no executable bit on NTFS. Behaviour
//! parity with `klasp_agents_claude` — `apply_mode` is a no-op there too.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use klasp_core::{AgentSurface, InstallContext, InstallError, InstallReport};

use crate::agents_md::{self, AgentsMdError, DEFAULT_BLOCK_BODY};
use crate::git_hooks::{self, HookError, HookKind, HookWarning};

/// Codex agent surface. Stateless; the registry stores it as
/// `Box<dyn AgentSurface>`.
pub struct CodexSurface;

impl CodexSurface {
    pub const AGENT_ID: &'static str = "codex";

    /// Filename of the markdown file Codex reads from the repo root.
    pub const AGENTS_MD: &'static str = "AGENTS.md";

    /// Repo-relative path of the git pre-commit hook.
    pub const PRE_COMMIT_RELPATH: &'static [&'static str] = &[".git", "hooks", "pre-commit"];

    /// Repo-relative path of the git pre-push hook.
    pub const PRE_PUSH_RELPATH: &'static [&'static str] = &[".git", "hooks", "pre-push"];

    /// Repo-relative path of the *primary* hook reported via the trait's
    /// `hook_path` method. Kept as `pre-commit` for parity with the W1
    /// API; consumers needing both paths should use
    /// [`Self::all_hook_paths`].
    pub const HOOK_RELPATH: &'static [&'static str] = Self::PRE_COMMIT_RELPATH;

    /// Both managed hook paths, in install order. W3 callers that want
    /// to render the full install report (e.g. for `klasp install --dry-run`)
    /// should iterate this rather than relying on the trait's single
    /// `hook_path`.
    pub fn all_hook_paths(repo_root: &Path) -> [(HookKind, PathBuf); 2] {
        [
            (HookKind::Commit, hook_path_for(repo_root, HookKind::Commit)),
            (HookKind::Push, hook_path_for(repo_root, HookKind::Push)),
        ]
    }

    /// Detailed install entry-point. Returns the standard
    /// [`InstallReport`] *and* the list of [`HookWarning`]s collected
    /// from the hook writer. The trait's [`AgentSurface::install`]
    /// method calls this and discards the warnings to keep the
    /// cross-crate trait surface unchanged; W3's CLI plumbing calls this
    /// method directly so it can render warnings to the user.
    pub fn install_detailed(
        &self,
        ctx: &InstallContext,
    ) -> Result<CodexInstallReport, InstallError> {
        // 1. AGENTS.md — same merge contract as W1.
        let settings_path = self.settings_path(&ctx.repo_root);
        let agents_existing = read_or_empty(&settings_path)?;
        let agents_merged = agents_md::install_block(&agents_existing, DEFAULT_BLOCK_BODY)
            .map_err(|e| agents_md_error(&settings_path, e))?;
        let agents_unchanged = agents_merged == agents_existing;

        // 2. Hooks — pre-commit and pre-push. Per-hook conflict check;
        //    on conflict, record a warning and skip the write.
        let mut hook_plans = Vec::with_capacity(2);
        let mut warnings = Vec::new();
        for (kind, path) in Self::all_hook_paths(&ctx.repo_root) {
            let plan = plan_hook_install(&path, kind, ctx.schema_version)?;
            if let HookPlanOutcome::Conflict(conflict) = plan.outcome {
                warnings.push(HookWarning::Skipped {
                    path: path.clone(),
                    kind,
                    conflict,
                });
            }
            hook_plans.push(plan);
        }

        let all_already_installed = agents_unchanged
            && hook_plans
                .iter()
                .all(|p| matches!(p.outcome, HookPlanOutcome::Unchanged));

        // 3. Dry-run: report shape only, no writes. Preview is the
        //    AGENTS.md merged body — that's the most user-readable thing
        //    we can show, and matches W1 behaviour.
        if ctx.dry_run {
            return Ok(CodexInstallReport {
                report: InstallReport {
                    agent_id: Self::AGENT_ID.to_string(),
                    hook_path: hook_path_for(&ctx.repo_root, HookKind::Commit),
                    settings_path,
                    already_installed: all_already_installed,
                    paths_written: Vec::new(),
                    preview: Some(agents_merged),
                },
                warnings,
            });
        }

        // 4. Apply the plans. Order: AGENTS.md first (cheapest to roll
        //    back if a hook write fails partway through), then each
        //    hook with its individual atomic write.
        let mut paths_written = Vec::new();

        if !agents_unchanged {
            ensure_parent(&settings_path)?;
            let mode = current_mode(&settings_path).unwrap_or(0o644);
            atomic_write(&settings_path, agents_merged.as_bytes(), mode)?;
            paths_written.push(settings_path.clone());
        }

        for plan in hook_plans {
            match plan.outcome {
                HookPlanOutcome::Write(merged) => {
                    ensure_parent(&plan.path)?;
                    // Hook scripts must be executable. Honour the user's
                    // pre-existing mode if they had one (so we don't
                    // *demote* a 0o775 hook to 0o755), otherwise fall
                    // back to the canonical 0o755.
                    let mode = current_mode(&plan.path).unwrap_or(0o755);
                    atomic_write(&plan.path, merged.as_bytes(), mode)?;
                    paths_written.push(plan.path);
                }
                HookPlanOutcome::Unchanged | HookPlanOutcome::Conflict(_) => {
                    // Either already up-to-date or owned by a foreign
                    // tool — both no-op for the writer.
                }
            }
        }

        Ok(CodexInstallReport {
            report: InstallReport {
                agent_id: Self::AGENT_ID.to_string(),
                hook_path: hook_path_for(&ctx.repo_root, HookKind::Commit),
                settings_path,
                already_installed: all_already_installed,
                paths_written,
                preview: None,
            },
            warnings,
        })
    }
}

/// Result of a [`CodexSurface::install_detailed`] call. Bundles the
/// standard [`InstallReport`] with the per-hook warnings collected
/// during install.
#[derive(Debug)]
pub struct CodexInstallReport {
    pub report: InstallReport,
    pub warnings: Vec<HookWarning>,
}


impl AgentSurface for CodexSurface {
    fn agent_id(&self) -> &'static str {
        Self::AGENT_ID
    }

    fn detect(&self, repo_root: &Path) -> bool {
        // Codex looks for `AGENTS.md` at the repo root. We treat its
        // presence as the auto-detect signal; `klasp install --force`
        // overrides a `false` return if the user wants to bootstrap the
        // file from scratch.
        repo_root.join(Self::AGENTS_MD).is_file()
    }

    fn hook_path(&self, repo_root: &Path) -> PathBuf {
        hook_path_for(repo_root, HookKind::Commit)
    }

    fn settings_path(&self, repo_root: &Path) -> PathBuf {
        repo_root.join(Self::AGENTS_MD)
    }

    fn render_hook_script(&self, ctx: &InstallContext) -> String {
        // Trait contract returns a single string; we pick the
        // pre-commit body since that's what `hook_path` reports. W3's
        // CLI dry-run renderer can call `git_hooks::install_block`
        // directly when it needs the pre-push body too.
        git_hooks::install_block("", HookKind::Commit, ctx.schema_version).unwrap_or_default()
    }

    fn install(&self, ctx: &InstallContext) -> Result<InstallReport, InstallError> {
        // Discard warnings; the `InstallReport` shape is fixed by
        // `klasp-core` and we may not extend it from here. W3 will
        // call `install_detailed` from the CLI to surface them.
        Ok(self.install_detailed(ctx)?.report)
    }

    fn install_with_warnings(
        &self,
        ctx: &InstallContext,
    ) -> Result<(InstallReport, Vec<klasp_core::SurfaceWarning>), InstallError> {
        let detailed = self.install_detailed(ctx)?;
        let warnings = detailed
            .warnings
            .into_iter()
            .map(|w| {
                let path = match &w {
                    HookWarning::Skipped { path, .. } => path.clone(),
                };
                klasp_core::SurfaceWarning {
                    path,
                    message: w.to_string().into(),
                }
            })
            .collect();
        Ok((detailed.report, warnings))
    }

    fn doctor_check(&self, repo_root: &Path, schema_version: u32) -> Vec<klasp_core::DoctorFinding> {
        use klasp_core::DoctorFinding;
        let mut findings = Vec::new();
        let agent_id = self.agent_id();

        // 1. AGENTS.md — managed-block presence.
        let agents_md_path = self.settings_path(repo_root);
        match std::fs::read_to_string(&agents_md_path) {
            Err(_) => findings.push(DoctorFinding::Fail(format!(
                "settings[{agent_id}]: {} not found; re-run `klasp install`",
                agents_md_path.display()
            ))),
            Ok(content) => {
                if content.contains(agents_md::MANAGED_START) {
                    findings.push(DoctorFinding::Ok(format!(
                        "settings[{agent_id}]: AGENTS.md managed block present"
                    )));
                } else {
                    findings.push(DoctorFinding::Fail(format!(
                        "settings[{agent_id}]: AGENTS.md managed block missing; \
                         re-run `klasp install`"
                    )));
                }
            }
        }

        // 2. Both git hooks — compare managed block only (user content above/below is allowed).
        for (kind, hook_path) in Self::all_hook_paths(repo_root) {
            let label = kind.filename();
            match std::fs::read_to_string(&hook_path) {
                Err(_) => findings.push(DoctorFinding::Fail(format!(
                    "hook[{agent_id}][{label}]: {} not found; re-run `klasp install`",
                    hook_path.display()
                ))),
                Ok(actual) => {
                    let expected_block =
                        git_hooks::render_managed_block(kind, schema_version);
                    match git_hooks::extract_managed_block(&actual) {
                        Some(actual_block) if actual_block == expected_block => {
                            findings.push(DoctorFinding::Ok(format!(
                                "hook[{agent_id}][{label}]: \
                                 current (schema v{schema_version})"
                            )));
                        }
                        Some(_) => findings.push(DoctorFinding::Fail(format!(
                            "hook[{agent_id}][{label}]: schema drift detected \
                             (re-run `klasp install`)"
                        ))),
                        None => findings.push(DoctorFinding::Fail(format!(
                            "hook[{agent_id}][{label}]: managed block missing; \
                             re-run `klasp install`"
                        ))),
                    }
                }
            }
        }

        findings
    }

    fn uninstall(&self, repo_root: &Path, dry_run: bool) -> Result<Vec<PathBuf>, InstallError> {
        let mut paths = Vec::new();

        // 1. AGENTS.md — strip block, remove the file if klasp was the
        //    sole content (round-trip from missing-file install).
        let settings_path = self.settings_path(repo_root);
        let agents_existing = read_or_empty(&settings_path)?;
        let agents_stripped = agents_md::uninstall_block(&agents_existing)
            .map_err(|e| agents_md_error(&settings_path, e))?;
        if agents_stripped != agents_existing {
            if !dry_run {
                if agents_stripped.is_empty() {
                    fs::remove_file(&settings_path).map_err(|e| InstallError::Io {
                        path: settings_path.clone(),
                        source: e,
                    })?;
                } else {
                    let mode = current_mode(&settings_path).unwrap_or(0o644);
                    atomic_write(&settings_path, agents_stripped.as_bytes(), mode)?;
                }
            }
            paths.push(settings_path);
        }

        // 2. Each hook — same shape, but if klasp was the only content
        //    we delete the file (so `git` falls back to its no-hook
        //    default rather than executing a shebang-only stub).
        //
        //    Mangled-marker tolerance: a hook that has the start marker
        //    without a matching end (or the pair in the wrong order) is
        //    treated as "user has hand-edited this and we don't know how
        //    to safely strip" — we leave the file alone rather than
        //    erroring partway through and leaving the repo half-uninstalled
        //    (AGENTS.md gone but hooks intact). The user can fix the
        //    markers and re-run; meanwhile install reports a non-fatal
        //    skip for that path.
        for (_, hook_path) in Self::all_hook_paths(repo_root) {
            if !hook_path.exists() {
                continue;
            }
            let existing = fs::read_to_string(&hook_path).map_err(|e| InstallError::Io {
                path: hook_path.clone(),
                source: e,
            })?;
            // If klasp doesn't own this file, leave it alone. This is
            // the symmetric inverse of the install-time conflict skip:
            // a husky / lefthook / pre-commit-framework hook never
            // gained a klasp marker, so it has nothing for us to strip.
            if !existing.contains(git_hooks::MANAGED_START) {
                continue;
            }
            let stripped = match git_hooks::uninstall_block(&existing) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if stripped == existing {
                continue;
            }
            if !dry_run {
                if stripped.is_empty() {
                    fs::remove_file(&hook_path).map_err(|e| InstallError::Io {
                        path: hook_path.clone(),
                        source: e,
                    })?;
                } else {
                    let mode = current_mode(&hook_path).unwrap_or(0o755);
                    atomic_write(&hook_path, stripped.as_bytes(), mode)?;
                }
            }
            paths.push(hook_path);
        }

        Ok(paths)
    }
}

fn hook_path_for(repo_root: &Path, kind: HookKind) -> PathBuf {
    let segments = match kind {
        HookKind::Commit => CodexSurface::PRE_COMMIT_RELPATH,
        HookKind::Push => CodexSurface::PRE_PUSH_RELPATH,
    };
    let mut p = repo_root.to_path_buf();
    for seg in segments {
        p.push(seg);
    }
    p
}

/// What `install` should do with one hook file.
enum HookPlanOutcome {
    /// Existing content already matches what we'd write — no-op.
    Unchanged,
    /// Foreign hook manager detected; skip with a warning.
    Conflict(crate::git_hooks::HookConflict),
    /// Write `merged` to disk.
    Write(String),
}

struct HookPlan {
    path: PathBuf,
    outcome: HookPlanOutcome,
}

fn plan_hook_install(
    path: &Path,
    kind: HookKind,
    schema_version: u32,
) -> Result<HookPlan, InstallError> {
    let existing = read_or_empty(path)?;

    // klasp already manages this file → drive the standard managed-block
    // merge. Conflict detection on a klasp-owned file is meaningless;
    // checking *first* would force-skip even our own hooks if a tool
    // marker happened to land in a sibling line, so we route through
    // marker detection before fingerprint sniffing.
    let already_klasp = git_hooks::contains_block(&existing).map_err(|e| hook_error(path, e))?;
    if !already_klasp {
        if let Some(conflict) = git_hooks::detect_conflict(&existing) {
            return Ok(HookPlan {
                path: path.to_path_buf(),
                outcome: HookPlanOutcome::Conflict(conflict),
            });
        }
    }

    let merged = git_hooks::install_block(&existing, kind, schema_version)
        .map_err(|e| hook_error(path, e))?;

    if merged == existing {
        Ok(HookPlan {
            path: path.to_path_buf(),
            outcome: HookPlanOutcome::Unchanged,
        })
    } else {
        Ok(HookPlan {
            path: path.to_path_buf(),
            outcome: HookPlanOutcome::Write(merged),
        })
    }
}

fn read_or_empty(path: &Path) -> Result<String, InstallError> {
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(path).map_err(|e| InstallError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

fn ensure_parent(path: &Path) -> Result<(), InstallError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    fs::create_dir_all(parent).map_err(|e| InstallError::Io {
        path: parent.to_path_buf(),
        source: e,
    })
}

/// Atomic write via tempfile + rename. `mode` is applied to the *temp*
/// file before the rename so the published file is never visible at
/// `NamedTempFile`'s `0o600` default — a concurrent `git commit` between
/// `persist` and a post-rename `chmod` would otherwise see a hook with
/// the executable bit cleared and abort with EACCES.
fn atomic_write(path: &Path, contents: &[u8], mode: u32) -> Result<(), InstallError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tf = tempfile::NamedTempFile::new_in(dir).map_err(|e| InstallError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    tf.write_all(contents).map_err(|e| InstallError::Io {
        path: tf.path().to_path_buf(),
        source: e,
    })?;
    tf.flush().map_err(|e| InstallError::Io {
        path: tf.path().to_path_buf(),
        source: e,
    })?;
    apply_mode(tf.path(), mode)?;
    tf.persist(path).map_err(|e| InstallError::Io {
        path: path.to_path_buf(),
        source: e.error,
    })?;
    Ok(())
}

#[cfg(unix)]
fn current_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).ok().map(|m| m.permissions().mode())
}

#[cfg(not(unix))]
fn current_mode(_path: &Path) -> Option<u32> {
    None
}

fn apply_mode(path: &Path, mode: u32) -> Result<(), InstallError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(mode);
        fs::set_permissions(path, perms).map_err(|e| InstallError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

fn agents_md_error(path: &Path, error: AgentsMdError) -> InstallError {
    InstallError::Surface {
        agent_id: CodexSurface::AGENT_ID.to_string(),
        message: format!("{}: {error}", path.display()),
    }
}

fn hook_error(path: &Path, error: HookError) -> InstallError {
    InstallError::Surface {
        agent_id: CodexSurface::AGENT_ID.to_string(),
        message: format!("{}: {error}", path.display()),
    }
}
