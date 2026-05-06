//! Per-user plugin disable list for klasp.
//!
//! Location: `$KLASP_DISABLED_PLUGINS_FILE` env override, or
//! `$HOME/.config/klasp/disabled-plugins.toml`.
//!
//! Format:
//! ```toml
//! disabled = ["my-linter", "another-plugin"]
//! ```
//!
//! The disable list is a klasp-side concept — it does not affect the plugin
//! wire format (`PluginGateInput` / `PluginGateOutput`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Environment variable that overrides the default disable list path.
/// Essential for test isolation.
pub const KLASP_DISABLED_PLUGINS_FILE_ENV: &str = "KLASP_DISABLED_PLUGINS_FILE";

/// TOML envelope for the disable list file.
#[derive(Debug, Default, Serialize, Deserialize)]
struct DisableList {
    #[serde(default)]
    disabled: Vec<String>,
}

/// Resolve the disable list path: env override or `~/.config/klasp/disabled-plugins.toml`.
pub fn resolve_disable_list_path() -> PathBuf {
    if let Ok(p) = std::env::var(KLASP_DISABLED_PLUGINS_FILE_ENV) {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    home.join(".config")
        .join("klasp")
        .join("disabled-plugins.toml")
}

/// Load the set of disabled plugin names from `path` (or default path if
/// `None`). Returns an empty set if the file does not exist.
pub fn load(path: Option<&Path>) -> HashSet<String> {
    let resolved: PathBuf;
    let p = match path {
        Some(p) => p,
        None => {
            resolved = resolve_disable_list_path();
            &resolved
        }
    };

    let raw = match std::fs::read_to_string(p) {
        Ok(s) => s,
        Err(_) => return HashSet::new(),
    };

    let list: DisableList = toml::from_str(&raw).unwrap_or_default();
    list.disabled.into_iter().collect()
}

/// Add `name` to the disable list at `path` (or default path if `None`).
///
/// Creates the parent directory and file if they do not exist. No-ops
/// (with a return value of `Ok(())`) if `name` is already disabled.
/// Writes atomically: writes to a sibling `.tmp` file, then renames.
pub fn add(name: &str, path: Option<&Path>) -> Result<(), String> {
    let resolved: PathBuf;
    let p: &Path = match path {
        Some(p) => p,
        None => {
            resolved = resolve_disable_list_path();
            &resolved
        }
    };

    // Ensure parent directory exists.
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create dir {}: {e}", parent.display()))?;
    }

    // Load existing list.
    let raw = std::fs::read_to_string(p).unwrap_or_default();
    let mut list: DisableList = toml::from_str(&raw).unwrap_or_default();

    if list.disabled.iter().any(|n| n == name) {
        return Ok(());
    }
    list.disabled.push(name.to_string());

    let serialized =
        toml::to_string_pretty(&list).map_err(|e| format!("serialize disable list: {e}"))?;

    // Atomic write: temp file + rename.
    let tmp = p.with_extension("toml.tmp");
    std::fs::write(&tmp, &serialized)
        .map_err(|e| format!("write temp file {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, p)
        .map_err(|e| format!("rename {} → {}: {e}", tmp.display(), p.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_path(dir: &TempDir) -> PathBuf {
        dir.path().join("disabled-plugins.toml")
    }

    #[test]
    fn load_returns_empty_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let path = tmp_path(&dir);
        let set = load(Some(&path));
        assert!(set.is_empty());
    }

    #[test]
    fn add_creates_file_and_loads_back() {
        let dir = TempDir::new().unwrap();
        let path = tmp_path(&dir);
        add("my-linter", Some(&path)).unwrap();
        let set = load(Some(&path));
        assert!(set.contains("my-linter"));
    }

    #[test]
    fn add_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = tmp_path(&dir);
        add("my-linter", Some(&path)).unwrap();
        add("my-linter", Some(&path)).unwrap();
        let set = load(Some(&path));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn add_creates_parent_dir() {
        let dir = TempDir::new().unwrap();
        let path = dir
            .path()
            .join("nested")
            .join("dir")
            .join("disabled-plugins.toml");
        add("my-linter", Some(&path)).unwrap();
        assert!(path.exists());
    }
}
