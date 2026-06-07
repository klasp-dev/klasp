//! Cross-recipe helpers shared by the named-recipe sources (`pre_commit`,
//! `fallow`, `cargo`, `pytest`).
//!
//! Every named recipe wraps a tool that klasp shells out to, parses its
//! output, and maps the result to a [`klasp_core::Verdict`]. Four chunks
//! of that machinery were byte-for-byte (or trivially-parameterised)
//! duplicates across the recipes before this module existed:
//!
//! - [`shell_quote`] — POSIX single-quoting for user-supplied argv tokens.
//! - [`finding`] / [`note`] — `Finding` builders that prepend a per-recipe
//!   rule prefix (`cargo:`, `pytest:`, …).
//! - [`fail_with_optional_warning`] — `Verdict::Fail` carrying a generic
//!   detail plus an optional version-warning row.
//! - [`sniff_version_warning`] — a cached `<tool> --version` probe that
//!   warns when the installed tool is older than the recipe's tested floor.
//!
//! The helpers are `pub(super)` so the recipe modules under
//! `crate::sources` can compose them; nothing here is part of klasp's
//! public surface. Pure functions with one exception: `sniff_version_warning`
//! spawns a subprocess and memoises the result per binary name.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use klasp_core::{Finding, Severity, Verdict};

/// Single-quote a value for inclusion in a `sh -c "<command>"` string.
/// Embedded single quotes become `'\''`, the standard POSIX trick. Used
/// only for user-supplied strings (`hook_stage`, `config_path`, `base`,
/// `package`, …); the flag literals are static and don't need quoting.
pub(super) fn shell_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Centralised `Finding` builder. The `rule_prefix` is the recipe's tool
/// slug (`pre_commit`, `fallow`, `cargo`, `pytest`). `rule_suffix = ""`
/// produces a top-level rule (`<prefix>:<check>`), suitable for
/// recipe-level notices; a non-empty suffix nests the rule
/// (`<prefix>:<check>:<suffix>`) for per-finding rows so a future filter
/// can target one category at a time.
pub(super) fn finding(
    rule_prefix: &str,
    check_name: &str,
    rule_suffix: &str,
    message: &str,
    file: Option<String>,
    line: Option<u32>,
    severity: Severity,
) -> Finding {
    let rule = if rule_suffix.is_empty() {
        format!("{rule_prefix}:{check_name}")
    } else {
        format!("{rule_prefix}:{check_name}:{rule_suffix}")
    };
    Finding {
        rule,
        message: message.to_string(),
        file,
        line,
        severity,
    }
}

/// One-line `Finding` builder for top-level (location-less) rows.
pub(super) fn note(
    rule_prefix: &str,
    check_name: &str,
    message: &str,
    severity: Severity,
) -> Finding {
    finding(rule_prefix, check_name, "", message, None, None, severity)
}

/// Build a `Verdict::Fail` whose findings carry the supplied detail plus
/// an optional version-warning prepended at `Severity::Warn`.
pub(super) fn fail_with_optional_warning(
    rule_prefix: &str,
    check_name: &str,
    detail: String,
    version_warning: Option<&str>,
) -> Verdict {
    let mut findings = vec![note(rule_prefix, check_name, &detail, Severity::Error)];
    if let Some(warning) = version_warning {
        findings.insert(0, note(rule_prefix, check_name, warning, Severity::Warn));
    }
    Verdict::Fail {
        findings,
        message: detail,
    }
}

/// Lazily run `<binary> --version`, parse the major.minor, and return a
/// warning when it falls outside the supported range. `None` means the
/// version is fine *or* we couldn't probe the binary (some wrappers don't
/// honour `--version`); both cases swallow the warning.
///
/// The probe result is memoised for the lifetime of the process, keyed by
/// binary name: a klasp gate invocation typically resolves each tool from
/// the same `$PATH` entry for every check, so re-running the probe per
/// check would multiply subprocess overhead by N for no signal. Keying by
/// name lets `sniff("pytest", …)` and `sniff("cargo", …)` share the cache
/// without colliding. `cwd` is the first caller's working directory; later
/// callers reuse the cached probe even from a different cwd, which is
/// correct because `<binary> --version` doesn't read the working
/// directory. If a future klasp use-case spans repos in one process with
/// divergent tools on `$PATH`, this cache becomes wrong and the keying
/// needs to be revisited.
///
/// When `check_stderr` is true the probe concatenates stderr after stdout
/// before parsing — pytest 7.x prints its banner on stderr while 8.x uses
/// stdout, so the parser has to see both.
pub(super) fn sniff_version_warning(
    binary: &'static str,
    min: (u32, u32),
    cwd: &Path,
    check_stderr: bool,
) -> Option<String> {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<&'static str, Option<String>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cached) = guard.get(binary) {
        return cached.clone();
    }
    let computed = sniff_version_warning_uncached(binary, min, cwd, check_stderr);
    guard.insert(binary, computed.clone());
    computed
}

fn sniff_version_warning_uncached(
    binary: &str,
    min: (u32, u32),
    cwd: &Path,
    check_stderr: bool,
) -> Option<String> {
    let output = Command::new(binary)
        .arg("--version")
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut raw = String::from_utf8_lossy(&output.stdout).into_owned();
    if check_stderr {
        raw.push('\n');
        raw.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    let (major, minor) = parse_version(&raw)?;
    if (major, minor) < min {
        let (rmaj, rmin) = min;
        return Some(format!(
            "{binary} {major}.{minor} is older than the minimum tested version \
             {rmaj}.{rmin}; output parsing may be incomplete"
        ));
    }
    None
}

/// Parse a `--version` banner into `Some((major, minor))`. Tolerant: scans
/// every line for the first whitespace-separated `MAJOR.MINOR.…` token
/// whose first two dot-separated segments parse as integers. This handles
/// `"pre-commit 3.8.0"`, `"cargo 1.79.0 (ded6e.. 2024-04-19)"` with a
/// build-date suffix, and `"This is pytest version 8.0.1, imported from …"`
/// where the version isn't the last token. Returns `None` when no
/// version-shaped token is found.
pub(super) fn parse_version(raw: &str) -> Option<(u32, u32)> {
    for line in raw.lines() {
        for token in line.split_whitespace() {
            let mut parts = token.split('.');
            let Some(maj_raw) = parts.next() else {
                continue;
            };
            let Some(min_raw) = parts.next() else {
                continue;
            };
            let Ok(major) = maj_raw.parse::<u32>() else {
                continue;
            };
            let Ok(minor) = min_raw.parse::<u32>() else {
                continue;
            };
            return Some((major, minor));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_handles_embedded_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn shell_quote_wraps_plain_value() {
        assert_eq!(shell_quote("origin/main"), "'origin/main'");
    }

    #[test]
    fn note_uses_top_level_rule() {
        let f = note("cargo", "build", "boom", Severity::Error);
        assert_eq!(f.rule, "cargo:build");
        assert_eq!(f.severity, Severity::Error);
        assert!(f.file.is_none());
        assert!(f.line.is_none());
    }

    #[test]
    fn finding_with_suffix_nests_rule() {
        let f = finding(
            "fallow",
            "audit",
            "complexity",
            "too complex",
            Some("src/x.ts".into()),
            Some(7),
            Severity::Warn,
        );
        assert_eq!(f.rule, "fallow:audit:complexity");
        assert_eq!(f.file.as_deref(), Some("src/x.ts"));
        assert_eq!(f.line, Some(7));
    }

    #[test]
    fn finding_empty_suffix_matches_note() {
        let a = finding("pytest", "tests", "", "msg", None, None, Severity::Error);
        let b = note("pytest", "tests", "msg", Severity::Error);
        assert_eq!(a.rule, b.rule);
        assert_eq!(a.message, b.message);
    }

    #[test]
    fn fail_with_optional_warning_prepends_warn_row() {
        let v = fail_with_optional_warning("cargo", "build", "boom".into(), Some("old cargo"));
        match v {
            Verdict::Fail { findings, message } => {
                assert_eq!(findings.len(), 2);
                assert_eq!(findings[0].severity, Severity::Warn);
                assert!(findings[0].message.contains("old cargo"));
                assert_eq!(findings[1].severity, Severity::Error);
                assert_eq!(message, "boom");
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn fail_with_optional_warning_omits_warn_when_none() {
        let v = fail_with_optional_warning("pre_commit", "lint", "detail".into(), None);
        match v {
            Verdict::Fail { findings, .. } => {
                assert_eq!(findings.len(), 1);
                assert_eq!(findings[0].rule, "pre_commit:lint");
            }
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn parse_version_extracts_major_minor() {
        assert_eq!(parse_version("pre-commit 3.8.0"), Some((3, 8)));
        assert_eq!(parse_version("fallow 2.62.0\n"), Some((2, 62)));
        assert_eq!(
            parse_version("cargo 1.79.0 (ded6ed5ec 2024-04-19)"),
            Some((1, 79))
        );
        assert_eq!(parse_version("pytest 8.3.2\n"), Some((8, 3)));
        assert_eq!(
            parse_version("This is pytest version 8.0.1, imported from …"),
            Some((8, 0))
        );
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("not a version"), None);
    }
}
