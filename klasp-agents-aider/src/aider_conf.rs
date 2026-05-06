//! `.aider.conf.yml` read / mutate / write helpers.
//!
//! ## Strategy: parse → mutate → serialize
//!
//! `.aider.conf.yml` is structured YAML. Line-text managed-block markers
//! (like those used in `AGENTS.md`) cannot safely round-trip structured YAML
//! without corrupting user content, so we parse the file into a
//! `serde_yaml_ng::Value`, mutate the `commit-cmd-pre` key in-place, and
//! re-serialize. This is always a semantic round-trip; comment preservation
//! is NOT guaranteed — see `### Limitations` in the crate README.
//!
//! ## Chain strategy for existing `commit-cmd-pre` values
//!
//! When the user already has a non-klasp `commit-cmd-pre`, we convert the
//! value to an array and prepend `klasp gate --agent aider`. This ensures
//! klasp always runs first (gate validates before commit), while the user's
//! existing command continues to run. Users who prefer skip-with-notice
//! behaviour can track <https://github.com/klasp-dev/klasp/issues/40>.

use serde_yaml_ng::Value;
use thiserror::Error;

/// The literal command string klasp writes into `commit-cmd-pre`.
pub const KLASP_CMD: &str = "klasp gate --agent aider";

const COMMIT_CMD_PRE_KEY: &str = "commit-cmd-pre";

#[derive(Debug, Error)]
pub enum AiderConfError {
    #[error("could not parse .aider.conf.yml: {0}")]
    Parse(#[from] serde_yaml_ng::Error),
    #[error(".aider.conf.yml is not a YAML mapping at the top level")]
    NotAMapping,
}

#[cfg(test)]
fn contains_klasp(doc: &Value) -> bool {
    match doc.get(COMMIT_CMD_PRE_KEY) {
        None => false,
        Some(Value::String(s)) => is_klasp_cmd(s),
        Some(Value::Sequence(seq)) => seq
            .iter()
            .any(|v| matches!(v, Value::String(s) if is_klasp_cmd(s))),
        _ => false,
    }
}

// Match exactly `KLASP_CMD` or any aider-tagged variant
// (`klasp gate --agent aider [extra-flags]`). Cross-agent invocations like
// `klasp gate --agent codex` are user-owned and must NOT be uninstalled by
// the aider surface — the `--agent aider` prefix is the discriminator.
fn is_klasp_cmd(s: &str) -> bool {
    s == KLASP_CMD || s.starts_with("klasp gate --agent aider ")
}

/// Insert `KLASP_CMD` into the parsed document. Returns `true` when the
/// document was changed, `false` when it was already present (idempotent).
///
/// Chain strategy:
/// - absent → set scalar `KLASP_CMD`
/// - already-klasp scalar/array → no-op (return `false`)
/// - non-klasp scalar → convert to array `[KLASP_CMD, old_value]`
/// - non-klasp array → prepend `KLASP_CMD`
pub fn install_into_doc(doc: &mut Value) -> Result<bool, AiderConfError> {
    let map = doc.as_mapping_mut().ok_or(AiderConfError::NotAMapping)?;

    let key = Value::String(COMMIT_CMD_PRE_KEY.to_string());
    match map.get(&key).cloned() {
        None => {
            map.insert(key, Value::String(KLASP_CMD.to_string()));
            Ok(true)
        }
        Some(Value::String(ref s)) if is_klasp_cmd(s) => Ok(false),
        Some(Value::Sequence(ref seq))
            if seq
                .iter()
                .any(|v| matches!(v, Value::String(s) if is_klasp_cmd(s))) =>
        {
            Ok(false)
        }
        Some(Value::String(old)) => {
            let arr = Value::Sequence(vec![
                Value::String(KLASP_CMD.to_string()),
                Value::String(old),
            ]);
            map.insert(key, arr);
            Ok(true)
        }
        Some(Value::Sequence(mut seq)) => {
            seq.insert(0, Value::String(KLASP_CMD.to_string()));
            map.insert(key, Value::Sequence(seq));
            Ok(true)
        }
        Some(_) => {
            // Unexpected shape (e.g. integer). Treat as non-klasp scalar by
            // leaving it and not overwriting — safer than silently corrupting.
            Ok(false)
        }
    }
}

/// Remove `KLASP_CMD` from the parsed document. Returns `true` when the
/// document was changed.
///
/// - scalar `KLASP_CMD` → remove key entirely
/// - array containing `KLASP_CMD` → remove that element; if length becomes 1,
///   collapse to scalar; if empty, remove key
pub fn uninstall_from_doc(doc: &mut Value) -> Result<bool, AiderConfError> {
    let map = doc.as_mapping_mut().ok_or(AiderConfError::NotAMapping)?;
    let key = Value::String(COMMIT_CMD_PRE_KEY.to_string());

    match map.get(&key).cloned() {
        None => Ok(false),
        Some(Value::String(ref s)) if is_klasp_cmd(s) => {
            map.remove(&key);
            Ok(true)
        }
        Some(Value::Sequence(seq)) => {
            let original_len = seq.len();
            let filtered: Vec<Value> = seq
                .into_iter()
                .filter(|v| !matches!(v, Value::String(s) if is_klasp_cmd(s)))
                .collect();
            if filtered.len() == original_len {
                return Ok(false);
            }
            match filtered.len() {
                0 => {
                    map.remove(&key);
                }
                1 => {
                    map.insert(key, filtered.into_iter().next().unwrap());
                }
                _ => {
                    map.insert(key, Value::Sequence(filtered));
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Parse YAML bytes. Returns an empty mapping `{}` when `src` is empty or
/// all-whitespace (aider doesn't require the file to pre-exist).
pub fn parse(src: &str) -> Result<Value, AiderConfError> {
    let trimmed = src.trim();
    if trimmed.is_empty() {
        return Ok(Value::Mapping(serde_yaml_ng::Mapping::new()));
    }
    let v: Value = serde_yaml_ng::from_str(src)?;
    Ok(v)
}

/// Serialize the document back to YAML text.
pub fn serialize(doc: &Value) -> Result<String, AiderConfError> {
    Ok(serde_yaml_ng::to_string(doc)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_doc() -> Value {
        parse("").unwrap()
    }

    fn doc_with(yaml: &str) -> Value {
        parse(yaml).unwrap()
    }

    #[test]
    fn install_into_empty_sets_scalar() {
        let mut doc = empty_doc();
        assert!(install_into_doc(&mut doc).unwrap());
        assert_eq!(
            doc.get("commit-cmd-pre"),
            Some(&Value::String(KLASP_CMD.to_string()))
        );
    }

    #[test]
    fn install_with_no_key_sets_scalar() {
        let mut doc = doc_with("model: gpt-4o\nauto-commits: false\n");
        assert!(install_into_doc(&mut doc).unwrap());
        assert_eq!(
            doc.get("commit-cmd-pre"),
            Some(&Value::String(KLASP_CMD.to_string()))
        );
        // Other keys preserved.
        assert_eq!(doc.get("model"), Some(&Value::String("gpt-4o".to_string())));
    }

    #[test]
    fn install_with_existing_klasp_scalar_is_idempotent() {
        let mut doc = doc_with(&format!("commit-cmd-pre: {KLASP_CMD}\n"));
        assert!(!install_into_doc(&mut doc).unwrap());
    }

    #[test]
    fn install_with_non_klasp_scalar_chains() {
        let mut doc = doc_with("commit-cmd-pre: pytest -q\n");
        assert!(install_into_doc(&mut doc).unwrap());
        let seq = doc.get("commit-cmd-pre").unwrap().as_sequence().unwrap();
        assert_eq!(seq.len(), 2);
        assert_eq!(seq[0].as_str(), Some(KLASP_CMD));
        assert_eq!(seq[1].as_str(), Some("pytest -q"));
    }

    #[test]
    fn install_with_non_klasp_array_prepends() {
        let mut doc = doc_with("commit-cmd-pre:\n  - lint\n  - format\n");
        assert!(install_into_doc(&mut doc).unwrap());
        let seq = doc.get("commit-cmd-pre").unwrap().as_sequence().unwrap();
        assert_eq!(seq.len(), 3);
        assert_eq!(seq[0].as_str(), Some(KLASP_CMD));
        assert_eq!(seq[1].as_str(), Some("lint"));
        assert_eq!(seq[2].as_str(), Some("format"));
    }

    #[test]
    fn install_with_existing_klasp_in_array_is_idempotent() {
        let mut doc = doc_with(&format!("commit-cmd-pre:\n  - {KLASP_CMD}\n  - other\n"));
        assert!(!install_into_doc(&mut doc).unwrap());
    }

    #[test]
    fn uninstall_removes_scalar() {
        let mut doc = doc_with(&format!("commit-cmd-pre: {KLASP_CMD}\n"));
        assert!(uninstall_from_doc(&mut doc).unwrap());
        assert!(doc.get("commit-cmd-pre").is_none());
    }

    #[test]
    fn uninstall_removes_from_array_and_collapses_single() {
        let mut doc = doc_with(&format!(
            "commit-cmd-pre:\n  - {KLASP_CMD}\n  - pytest -q\n"
        ));
        assert!(uninstall_from_doc(&mut doc).unwrap());
        assert_eq!(
            doc.get("commit-cmd-pre"),
            Some(&Value::String("pytest -q".to_string()))
        );
    }

    #[test]
    fn uninstall_removes_from_array_keeps_multiple_siblings() {
        let mut doc = doc_with(&format!(
            "commit-cmd-pre:\n  - {KLASP_CMD}\n  - lint\n  - format\n"
        ));
        assert!(uninstall_from_doc(&mut doc).unwrap());
        let seq = doc.get("commit-cmd-pre").unwrap().as_sequence().unwrap();
        assert_eq!(seq.len(), 2);
    }

    #[test]
    fn uninstall_when_key_absent_is_noop() {
        let mut doc = doc_with("model: gpt-4o\n");
        assert!(!uninstall_from_doc(&mut doc).unwrap());
    }

    #[test]
    fn contains_klasp_scalar() {
        let doc = doc_with(&format!("commit-cmd-pre: {KLASP_CMD}\n"));
        assert!(contains_klasp(&doc));
    }

    #[test]
    fn contains_klasp_in_array() {
        let doc = doc_with(&format!("commit-cmd-pre:\n  - {KLASP_CMD}\n  - other\n"));
        assert!(contains_klasp(&doc));
    }

    #[test]
    fn not_contains_klasp_when_absent() {
        let doc = doc_with("model: gpt-4o\n");
        assert!(!contains_klasp(&doc));
    }
}
