//! Install-time filesystem helpers shared by every `AgentSurface` impl.
//!
//! Each surface (Claude Code, Codex, Aider, …) writes one or more managed
//! files during `install`/`uninstall`. The mechanics — atomic
//! tempfile-then-rename, parent-directory creation, Unix mode
//! preservation — are identical across surfaces, so they live here rather
//! than being triplicated per crate.
//!
//! ## Atomic write ordering
//!
//! [`atomic_write`] applies `mode` to the *temp* file **before** the
//! rename, so the published file is never briefly visible at
//! `NamedTempFile`'s `0o600` default. Applying the mode after `persist`
//! would expose a window where a concurrent reader (e.g. a `git commit`
//! racing an install) sees a hook with the executable bit cleared and
//! aborts with `EACCES`.
//!
//! ## Windows notes
//!
//! On Windows, [`current_mode`] and [`apply_mode`] are no-ops — NTFS has
//! no executable permission bit, and `bash.exe` / `sh.exe` (Git for
//! Windows) interpret a script's shebang at runtime regardless. The
//! generated hook scripts therefore work without any chmod step.

use std::fs;
use std::io::Write;
use std::path::Path;

use crate::InstallError;

/// Read `path` to a string, returning an empty string if it does not
/// exist. Matches on [`std::io::ErrorKind::NotFound`] rather than probing
/// with `Path::exists` first, so there is no time-of-check/time-of-use gap
/// between the existence test and the read.
pub fn read_or_empty(path: &Path) -> Result<String, InstallError> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(InstallError::Io {
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

/// Ensure `path`'s parent directory exists, creating it (and any missing
/// ancestors) if needed. A path with no parent, or whose parent is the
/// empty string (a bare relative filename), is a no-op.
pub fn ensure_parent(path: &Path) -> Result<(), InstallError> {
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
/// `NamedTempFile`'s `0o600` default (see module docs for the race this
/// avoids).
pub fn atomic_write(path: &Path, contents: &[u8], mode: u32) -> Result<(), InstallError> {
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

/// The file's current Unix mode (low bits), or `None` if the file does not
/// exist or we are not on Unix. Call this *before* [`atomic_write`] so the
/// published file inherits the user's prior mode rather than the tempfile
/// default.
#[cfg(unix)]
pub fn current_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).ok().map(|m| m.permissions().mode())
}

#[cfg(not(unix))]
pub fn current_mode(_path: &Path) -> Option<u32> {
    None
}

/// Set `path`'s Unix permission bits to `mode`. A no-op on non-Unix
/// targets, where NTFS has no executable bit and the shebang is honoured
/// at runtime regardless.
pub fn apply_mode(path: &Path, mode: u32) -> Result<(), InstallError> {
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

/// Write `merged` to `path` only if it differs from `existing`, preserving
/// the file's current Unix mode (falling back to `0o644` for new files).
/// Returns `true` if a write happened, `false` if the content was already
/// up to date.
///
/// Encapsulates the merge-then-write idiom every surface uses for its
/// settings/config file: compare the freshly merged text against what was
/// read off disk, and on a difference ensure the parent dir, look up the
/// current mode, and atomically write. Hook-script writes that need a
/// different default mode (e.g. `0o755`) call [`atomic_write`] directly.
pub fn write_if_changed(path: &Path, existing: &str, merged: &str) -> Result<bool, InstallError> {
    if merged == existing {
        return Ok(false);
    }
    ensure_parent(path)?;
    let mode = current_mode(path).unwrap_or(0o644);
    atomic_write(path, merged.as_bytes(), mode)?;
    Ok(true)
}
