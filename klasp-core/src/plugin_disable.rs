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

/// Validate that `name` is a well-formed plugin name. Same shape as the
/// `klasp-plugin-<name>` binary lookup: ASCII letters, digits, `_`, `-`.
/// Rejects path separators, shell metachars, control chars, and empty names.
pub fn validate_plugin_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("plugin name is empty".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "plugin name `{name}` contains invalid characters; allowed: ASCII letters, digits, `-`, `_`"
        ));
    }
    Ok(())
}

/// Load the set of disabled plugin names from `path` (or default path if
/// `None`). Returns an empty set if the file does not exist.
///
/// On TOML parse failure (user hand-edited and produced invalid syntax) this
/// degrades silently to an empty set after writing a warning to stderr — the
/// gate must continue running. To loudly reject malformed input, use the
/// stricter `add()` path which refuses to overwrite a malformed file.
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

    match toml::from_str::<DisableList>(&raw) {
        Ok(list) => list.disabled.into_iter().collect(),
        Err(e) => {
            eprintln!(
                "warning: disable list at {} is malformed ({e}); ignoring (run `klasp plugins disable` to overwrite)",
                p.display()
            );
            HashSet::new()
        }
    }
}

/// Add `name` to the disable list at `path` (or default path if `None`).
///
/// Creates the parent directory and file if they do not exist. No-ops
/// (with `Ok(())`) if `name` is already disabled.
/// Writes atomically: writes to a sibling `.tmp` file, then renames.
///
/// Refuses to overwrite a malformed file: if the existing list is invalid
/// TOML, returns `Err` so the user can fix or delete it manually rather than
/// losing the previously-disabled entries.
///
/// Note: not concurrency-safe — concurrent `add()` calls may lose writes.
/// v0.3 limitation; documented in `docs/plugin-protocol.md` §Disable list.
pub fn add(name: &str, path: Option<&Path>) -> Result<(), String> {
    validate_plugin_name(name)?;

    let resolved: PathBuf;
    let p: &Path = match path {
        Some(p) => p,
        None => {
            resolved = resolve_disable_list_path();
            &resolved
        }
    };

    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create dir {}: {e}", parent.display()))?;
    }

    let raw = std::fs::read_to_string(p).unwrap_or_default();
    let mut list: DisableList = if raw.trim().is_empty() {
        DisableList::default()
    } else {
        toml::from_str(&raw).map_err(|e| {
            format!(
                "disable list at {} is malformed: {e}; refusing to overwrite — fix or delete the file and retry",
                p.display()
            )
        })?
    };

    if list.disabled.iter().any(|n| n == name) {
        return Ok(());
    }
    list.disabled.push(name.to_string());

    let serialized =
        toml::to_string_pretty(&list).map_err(|e| format!("serialize disable list: {e}"))?;

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

    #[test]
    fn add_refuses_to_overwrite_malformed_file() {
        let dir = TempDir::new().unwrap();
        let path = tmp_path(&dir);
        std::fs::write(&path, "this is not valid toml = = =").unwrap();
        let result = add("my-linter", Some(&path));
        assert!(result.is_err(), "expected Err on malformed file");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("malformed") && msg.contains("refusing to overwrite"),
            "expected refusal message, got: {msg}"
        );
        // File contents preserved.
        let preserved = std::fs::read_to_string(&path).unwrap();
        assert_eq!(preserved, "this is not valid toml = = =");
    }

    #[test]
    fn load_returns_empty_on_malformed_file() {
        let dir = TempDir::new().unwrap();
        let path = tmp_path(&dir);
        std::fs::write(&path, "this is not valid toml = = =").unwrap();
        let set = load(Some(&path));
        assert!(set.is_empty());
    }

    #[test]
    fn validate_rejects_bad_names() {
        assert!(validate_plugin_name("").is_err());
        assert!(validate_plugin_name("../etc/passwd").is_err());
        assert!(validate_plugin_name("name with space").is_err());
        assert!(validate_plugin_name("name\nwith\nnewline").is_err());
        assert!(validate_plugin_name("name;rm-rf").is_err());
    }

    #[test]
    fn validate_accepts_valid_names() {
        assert!(validate_plugin_name("my-linter").is_ok());
        assert!(validate_plugin_name("my_linter_v2").is_ok());
        assert!(validate_plugin_name("Linter123").is_ok());
    }

    #[test]
    fn add_rejects_invalid_name() {
        let dir = TempDir::new().unwrap();
        let path = tmp_path(&dir);
        let result = add("../etc/passwd", Some(&path));
        assert!(result.is_err());
        assert!(!path.exists(), "must not create file for invalid name");
    }
}
