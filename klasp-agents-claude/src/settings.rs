//! Surgical merge of klasp's hook entry into `.claude/settings.json`.
//!
//! Highest-risk module of v0.1 — see [docs/design.md §5] (closing line) and
//! §14 (4th bullet on key-order roundtrip). Every key the user (or any
//! sibling tool: fallow, claude-code itself, the user's own pre-tool hooks)
//! has set must survive untouched. Idempotency is by exact match on the
//! `command` string of klasp's hook entry: re-running the merge with the
//! same input produces a byte-identical output.

use serde_json::{Map, Value};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("settings.json: invalid JSON: {0}")]
    Parse(#[from] serde_json::Error),

    /// Some node along the path `hooks.PreToolUse[*]` has a JSON type that
    /// conflicts with the Claude Code hook schema. We refuse to coerce
    /// (silently dropping a user's data is the worst possible outcome).
    #[error("settings.json: at `{path}`, expected {expected} but found {got}")]
    Shape {
        path: String,
        expected: &'static str,
        got: &'static str,
    },
}

/// Append klasp's PreToolUse `Bash` hook entry to `settings_json`, returning
/// the new file body. Idempotent: if an entry with the exact `hook_command`
/// is already present, returns the input unchanged (modulo re-serialisation
/// whitespace — see §14).
///
/// Empty input is treated as `"{}"`, so the call site doesn't have to special-case
/// missing settings files.
pub fn merge_hook_entry(settings_json: &str, hook_command: &str) -> Result<String, SettingsError> {
    let trimmed = settings_json.trim();
    let mut root: Value = if trimmed.is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(trimmed)?
    };

    let root_obj = expect_object_mut(&mut root, "")?;

    let hooks = get_or_insert_object(root_obj, "hooks")?;
    let pretool = get_or_insert_array(hooks, "PreToolUse", "hooks.PreToolUse")?;

    let bash_idx = find_bash_matcher(pretool)?;
    let bash_entry = match bash_idx {
        Some(i) => &mut pretool[i],
        None => {
            pretool.push(Value::Object({
                let mut m = Map::new();
                m.insert("matcher".into(), Value::String("Bash".into()));
                m.insert("hooks".into(), Value::Array(Vec::new()));
                m
            }));
            pretool.last_mut().expect("just pushed")
        }
    };

    let bash_obj = expect_object_mut(bash_entry, "hooks.PreToolUse[Bash]")?;
    let inner = get_or_insert_array(bash_obj, "hooks", "hooks.PreToolUse[Bash].hooks")?;

    if !inner.iter().any(|h| hook_command_matches(h, hook_command)) {
        let mut entry = Map::new();
        entry.insert("type".into(), Value::String("command".into()));
        entry.insert("command".into(), Value::String(hook_command.into()));
        inner.push(Value::Object(entry));
    }

    Ok(serialise(&root))
}

/// Inverse of [`merge_hook_entry`]: remove every hook with `command` exactly
/// equal to `hook_command`. Cleans up empty arrays/objects so a fresh-install
/// settings.json round-trips through install→uninstall to its pre-install shape.
///
/// Idempotent: running on a settings.json that has no klasp entry returns the
/// input unchanged.
pub fn unmerge_hook_entry(
    settings_json: &str,
    hook_command: &str,
) -> Result<String, SettingsError> {
    let trimmed = settings_json.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }

    let mut root: Value = serde_json::from_str(trimmed)?;
    let root_obj = expect_object_mut(&mut root, "")?;

    let Some(hooks_val) = root_obj.get_mut("hooks") else {
        return Ok(serialise(&root));
    };
    let hooks = expect_object_mut(hooks_val, "hooks")?;

    let Some(pretool_val) = hooks.get_mut("PreToolUse") else {
        return Ok(serialise(&root));
    };
    let pretool = expect_array_mut(pretool_val, "hooks.PreToolUse")?;

    for matcher in pretool.iter_mut() {
        let Some(matcher_obj) = matcher.as_object_mut() else {
            continue;
        };
        let Some(inner_val) = matcher_obj.get_mut("hooks") else {
            continue;
        };
        let Some(inner) = inner_val.as_array_mut() else {
            continue;
        };
        inner.retain(|h| !hook_command_matches(h, hook_command));
    }

    // Sweep up Bash matchers whose `hooks` array is now empty (klasp put
    // them there in the first place). Untouched matchers — those that
    // started with sibling hooks — keep their `hooks: []` if a teammate
    // emptied them, since that's the user's data.
    pretool.retain(|m| {
        let Some(obj) = m.as_object() else {
            return true;
        };
        if obj.get("matcher").and_then(Value::as_str) != Some("Bash") {
            return true;
        }
        !matches!(
            obj.get("hooks").and_then(Value::as_array),
            Some(arr) if arr.is_empty()
        )
    });

    if pretool.is_empty() {
        hooks.remove("PreToolUse");
    }
    if hooks.is_empty() {
        root_obj.remove("hooks");
    }

    Ok(serialise(&root))
}

fn serialise(value: &Value) -> String {
    let mut out = serde_json::to_string_pretty(value).expect("Value -> string is infallible");
    out.push('\n');
    out
}

fn expect_object_mut<'a>(
    value: &'a mut Value,
    path: &str,
) -> Result<&'a mut Map<String, Value>, SettingsError> {
    let got = describe(value);
    value.as_object_mut().ok_or_else(|| SettingsError::Shape {
        path: path.to_string(),
        expected: "object",
        got,
    })
}

fn expect_array_mut<'a>(
    value: &'a mut Value,
    path: &str,
) -> Result<&'a mut Vec<Value>, SettingsError> {
    let got = describe(value);
    value.as_array_mut().ok_or_else(|| SettingsError::Shape {
        path: path.to_string(),
        expected: "array",
        got,
    })
}

fn get_or_insert_object<'a>(
    map: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>, SettingsError> {
    let entry = map.entry(key).or_insert_with(|| Value::Object(Map::new()));
    let got = describe(entry);
    entry.as_object_mut().ok_or(SettingsError::Shape {
        path: key.to_string(),
        expected: "object",
        got,
    })
}

fn get_or_insert_array<'a>(
    map: &'a mut Map<String, Value>,
    key: &str,
    full_path: &str,
) -> Result<&'a mut Vec<Value>, SettingsError> {
    let entry = map.entry(key).or_insert_with(|| Value::Array(Vec::new()));
    let got = describe(entry);
    entry.as_array_mut().ok_or(SettingsError::Shape {
        path: full_path.to_string(),
        expected: "array",
        got,
    })
}

/// Find the index of a matcher object in `PreToolUse` whose `matcher` field
/// is exactly the string `"Bash"`. Errors only if a matcher entry isn't an
/// object (Claude's schema requires it).
fn find_bash_matcher(pretool: &[Value]) -> Result<Option<usize>, SettingsError> {
    for (i, m) in pretool.iter().enumerate() {
        let Some(obj) = m.as_object() else {
            return Err(SettingsError::Shape {
                path: format!("hooks.PreToolUse[{i}]"),
                expected: "object",
                got: describe(m),
            });
        };
        if obj.get("matcher").and_then(Value::as_str) == Some("Bash") {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

fn hook_command_matches(hook: &Value, expected_command: &str) -> bool {
    hook.get("command").and_then(Value::as_str) == Some(expected_command)
}

fn describe(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KLASP_CMD: &str = "${CLAUDE_PROJECT_DIR}/.claude/hooks/klasp-gate.sh";

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).expect("test fixture must be valid JSON")
    }

    #[test]
    fn merge_into_empty_creates_full_path() {
        let out = merge_hook_entry("", KLASP_CMD).unwrap();
        let v = parse(&out);
        assert_eq!(
            v["hooks"]["PreToolUse"][0]["matcher"],
            Value::String("Bash".into())
        );
        let hook = &v["hooks"]["PreToolUse"][0]["hooks"][0];
        assert_eq!(hook["type"], Value::String("command".into()));
        assert_eq!(hook["command"], Value::String(KLASP_CMD.into()));
    }

    #[test]
    fn merge_into_empty_object_creates_full_path() {
        let out = merge_hook_entry("{}", KLASP_CMD).unwrap();
        let v = parse(&out);
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "Bash");
    }

    #[test]
    fn merge_preserves_unrelated_top_level_keys() {
        let input = r#"{
            "theme": "dark",
            "permissions": { "allow": ["Read"] }
        }"#;
        let out = merge_hook_entry(input, KLASP_CMD).unwrap();
        let v = parse(&out);
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["permissions"]["allow"][0], "Read");
    }

    #[test]
    fn merge_preserves_sibling_hook_types() {
        let input = r#"{
            "hooks": {
                "PostToolUse": [
                    { "matcher": "Write", "hooks": [{ "type": "command", "command": "echo wrote" }] }
                ]
            }
        }"#;
        let out = merge_hook_entry(input, KLASP_CMD).unwrap();
        let v = parse(&out);
        assert_eq!(v["hooks"]["PostToolUse"][0]["matcher"], "Write");
        assert_eq!(
            v["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
            "echo wrote"
        );
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "Bash");
    }

    #[test]
    fn merge_appends_alongside_existing_bash_hooks() {
        let input = r#"{
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            { "type": "command", "command": "fallow gate" }
                        ]
                    }
                ]
            }
        }"#;
        let out = merge_hook_entry(input, KLASP_CMD).unwrap();
        let v = parse(&out);
        let inner = v["hooks"]["PreToolUse"][0]["hooks"].as_array().unwrap();
        assert_eq!(inner.len(), 2);
        assert_eq!(inner[0]["command"], "fallow gate");
        assert_eq!(inner[1]["command"], KLASP_CMD);
    }

    #[test]
    fn merge_is_idempotent() {
        let once = merge_hook_entry("{}", KLASP_CMD).unwrap();
        let twice = merge_hook_entry(&once, KLASP_CMD).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn merge_does_not_duplicate_existing_klasp_entry() {
        let input = format!(
            r#"{{
                "hooks": {{
                    "PreToolUse": [
                        {{
                            "matcher": "Bash",
                            "hooks": [
                                {{ "type": "command", "command": "{KLASP_CMD}" }}
                            ]
                        }}
                    ]
                }}
            }}"#
        );
        let out = merge_hook_entry(&input, KLASP_CMD).unwrap();
        let v = parse(&out);
        let inner = v["hooks"]["PreToolUse"][0]["hooks"].as_array().unwrap();
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0]["command"], KLASP_CMD);
    }

    #[test]
    fn merge_creates_bash_matcher_alongside_other_matchers() {
        let input = r#"{
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Write|Edit", "hooks": [{ "type": "command", "command": "lint" }] }
                ]
            }
        }"#;
        let out = merge_hook_entry(input, KLASP_CMD).unwrap();
        let v = parse(&out);
        let arr = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["matcher"], "Write|Edit");
        assert_eq!(arr[1]["matcher"], "Bash");
    }

    #[test]
    fn merge_fails_on_malformed_json() {
        let err = merge_hook_entry("{ not json", KLASP_CMD).expect_err("must fail");
        assert!(matches!(err, SettingsError::Parse(_)));
    }

    #[test]
    fn merge_fails_when_root_is_array() {
        let err = merge_hook_entry("[]", KLASP_CMD).expect_err("must fail");
        match err {
            SettingsError::Shape {
                expected,
                got,
                path,
            } => {
                assert_eq!(expected, "object");
                assert_eq!(got, "array");
                assert_eq!(path, "");
            }
            other => panic!("expected Shape, got {other:?}"),
        }
    }

    #[test]
    fn merge_fails_when_pretooluse_is_object() {
        let input = r#"{ "hooks": { "PreToolUse": {} } }"#;
        let err = merge_hook_entry(input, KLASP_CMD).expect_err("must fail");
        assert!(matches!(err, SettingsError::Shape { .. }));
    }

    #[test]
    fn unmerge_removes_only_klasp_entry() {
        let input = format!(
            r#"{{
                "theme": "dark",
                "hooks": {{
                    "PreToolUse": [
                        {{
                            "matcher": "Bash",
                            "hooks": [
                                {{ "type": "command", "command": "fallow gate" }},
                                {{ "type": "command", "command": "{KLASP_CMD}" }}
                            ]
                        }}
                    ]
                }}
            }}"#
        );
        let out = unmerge_hook_entry(&input, KLASP_CMD).unwrap();
        let v = parse(&out);
        assert_eq!(v["theme"], "dark");
        let inner = v["hooks"]["PreToolUse"][0]["hooks"].as_array().unwrap();
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0]["command"], "fallow gate");
    }

    #[test]
    fn unmerge_drops_empty_bash_matcher_and_collapses_path() {
        let input = format!(
            r#"{{
                "hooks": {{
                    "PreToolUse": [
                        {{
                            "matcher": "Bash",
                            "hooks": [
                                {{ "type": "command", "command": "{KLASP_CMD}" }}
                            ]
                        }}
                    ]
                }}
            }}"#
        );
        let out = unmerge_hook_entry(&input, KLASP_CMD).unwrap();
        let v = parse(&out);
        // hooks key should be gone (the only matcher was klasp's, which we
        // then dropped because its hooks array became empty).
        assert!(v.get("hooks").is_none(), "got: {v:#?}");
    }

    #[test]
    fn unmerge_is_noop_when_klasp_not_present() {
        let input = r#"{ "theme": "dark" }"#;
        let out = unmerge_hook_entry(input, KLASP_CMD).unwrap();
        let v = parse(&out);
        assert_eq!(v["theme"], "dark");
    }

    #[test]
    fn unmerge_is_idempotent() {
        let installed = merge_hook_entry("{}", KLASP_CMD).unwrap();
        let once = unmerge_hook_entry(&installed, KLASP_CMD).unwrap();
        let twice = unmerge_hook_entry(&once, KLASP_CMD).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn install_uninstall_round_trip_drops_to_empty_object() {
        let installed = merge_hook_entry("{}", KLASP_CMD).unwrap();
        let restored = unmerge_hook_entry(&installed, KLASP_CMD).unwrap();
        let v = parse(&restored);
        assert!(v.as_object().unwrap().is_empty());
    }
}
