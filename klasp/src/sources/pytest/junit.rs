//! JUnit XML helpers for the pytest recipe — failure extraction and
//! summary.
//!
//! Lifted out of the sibling `super` module to keep `pytest.rs` under
//! the project's 500-line cap, mirroring the W5 split between
//! `fallow.rs` and `fallow/json.rs`. Everything in here walks an XML
//! string and returns [`klasp_core::Finding`]s or summary strings;
//! nothing here spawns subprocesses or touches the filesystem.
//!
//! ## Why a bespoke parser
//!
//! Pytest's JUnit XML emission is well-formed, predictable, and only
//! exercises a tiny subset of the JUnit schema (`<testsuites>`,
//! `<testsuite>`, `<testcase>`, plus `<failure>` / `<error>` children).
//! Pulling in `quick-xml` or `xml-rs` to read four element names would
//! be 50-100 KiB of binary weight for no signal. The parser below scans
//! for `<testcase ...>` opens and pairs each with the next `</testcase>`
//! close, peeking inside to look for a `<failure` or `<error` element
//! that flags a per-case failure. Attribute extraction is character-
//! oriented (`name="..."`) with XML entity decoding for the standard
//! five entities.

use klasp_core::{Finding, Severity};

use super::verdict::finding;
use super::MAX_FINDINGS;

/// Parsed projection of one failed/errored `<testcase>`. Fields are
/// optional because pytest's emission omits `file` / `line` for
/// dynamically-generated tests.
struct TestCase {
    classname: Option<String>,
    name: Option<String>,
    file: Option<String>,
    line: Option<u32>,
    /// Failure / error message, taken from the `message="…"` attribute
    /// on the inner `<failure>` or `<error>` element. Falls back to
    /// the element's text content if the attribute is absent.
    failure_message: Option<String>,
}

/// Walk pytest's JUnit XML output and emit one structured finding per
/// failed `<testcase>`. Capped at [`MAX_FINDINGS`] so a 1000-test suite
/// with 800 reds doesn't drown the agent's stderr.
pub(super) fn collect_failures(check_name: &str, xml: &str) -> Vec<Finding> {
    let cases = scan_testcases(xml);
    cases
        .into_iter()
        .filter(|tc| tc.failure_message.is_some())
        .take(MAX_FINDINGS)
        .map(|tc| build_finding(check_name, &tc))
        .collect()
}

fn build_finding(check_name: &str, tc: &TestCase) -> Finding {
    let test_label = match (&tc.classname, &tc.name) {
        (Some(c), Some(n)) if !c.is_empty() => format!("{c}::{n}"),
        (_, Some(n)) => n.clone(),
        (Some(c), None) => c.clone(),
        (None, None) => "<unknown test>".to_string(),
    };
    let message = match &tc.failure_message {
        Some(m) if !m.is_empty() => format!("test `{test_label}` failed: {m}"),
        _ => format!("test `{test_label}` failed"),
    };
    finding(
        check_name,
        "failure",
        &message,
        tc.file.clone(),
        tc.line,
        Severity::Error,
    )
}

/// Render the verdict-level summary line. "N test(s) failed" is the
/// shape pytest itself uses in its trailer.
pub(super) fn summarise_failures(findings: &[Finding]) -> String {
    let n = findings.len();
    if n == 1 {
        "pytest reported 1 test failure".to_string()
    } else {
        format!("pytest reported {n} test failures")
    }
}

/// Scan for every `<testcase …>` open tag. For each, capture the
/// open-tag attributes, then look at the slice between the open and
/// the next `</testcase>` for an inner `<failure` or `<error` element.
/// Self-closing `<testcase … />` cases (the all-passed common case)
/// are skipped — they can't carry a failure child.
fn scan_testcases(xml: &str) -> Vec<TestCase> {
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(start) = xml[cursor..].find("<testcase") {
        let abs_start = cursor + start;
        let after_marker = abs_start + "<testcase".len();
        // Boundary check: ensure the next char ends the element name
        // (`<testcaseS>` would have been a different element).
        let next_ch = xml[after_marker..].chars().next();
        if !matches!(next_ch, Some(c) if c.is_whitespace() || c == '>' || c == '/') {
            cursor = after_marker;
            continue;
        }
        // Find end of the open tag: the first `>` that isn't inside an
        // attribute value. JUnit XML attribute values are double-quoted,
        // so we can naively walk while tracking quote state.
        let Some(open_end) = find_tag_close(&xml[after_marker..]) else {
            break;
        };
        let open_end_abs = after_marker + open_end;
        let open_attrs = &xml[after_marker..open_end_abs];
        let self_closing = open_attrs.trim_end().ends_with('/');

        let mut tc = parse_testcase_attrs(open_attrs);

        if !self_closing {
            // Slice between `>` and next `</testcase>`.
            let body_start = open_end_abs + 1;
            let close_offset = xml[body_start..].find("</testcase>");
            if let Some(off) = close_offset {
                let body = &xml[body_start..body_start + off];
                tc.failure_message = extract_failure_message(body);
                cursor = body_start + off + "</testcase>".len();
            } else {
                // Malformed — bail out so we don't loop forever.
                break;
            }
        } else {
            cursor = open_end_abs + 1;
        }

        out.push(tc);
    }
    out
}

/// Find the index of the closing `>` for the element open tag, treating
/// quoted attribute values as opaque. Returns the offset *within* the
/// passed-in slice.
fn find_tag_close(s: &str) -> Option<usize> {
    let mut in_quote = false;
    for (i, ch) in s.char_indices() {
        match ch {
            '"' => in_quote = !in_quote,
            '>' if !in_quote => return Some(i),
            _ => {}
        }
    }
    None
}

fn parse_testcase_attrs(attrs: &str) -> TestCase {
    TestCase {
        classname: attr_value(attrs, "classname"),
        name: attr_value(attrs, "name"),
        file: attr_value(attrs, "file"),
        line: attr_value(attrs, "line").and_then(|s| s.parse::<u32>().ok()),
        failure_message: None,
    }
}

/// Pull the value of `name="…"` out of an XML element's attribute
/// string. Returns `None` if the attribute is absent. Decodes the
/// standard five XML entities. The match is whitespace-anchored so
/// `name="x"` doesn't pick up `classname="x"`.
fn attr_value(attrs: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    // Walk, requiring the byte just before the match to be whitespace
    // or the start of the attribute string.
    let mut search_from = 0;
    while let Some(idx) = attrs[search_from..].find(&needle) {
        let abs = search_from + idx;
        let prev = if abs == 0 {
            ' '
        } else {
            // Safe because XML attribute names are ASCII.
            attrs.as_bytes()[abs - 1] as char
        };
        if prev.is_whitespace() {
            let value_start = abs + needle.len();
            let rest = &attrs[value_start..];
            if let Some(end) = rest.find('"') {
                return Some(decode_entities(&rest[..end]));
            }
            return None;
        }
        search_from = abs + needle.len();
    }
    None
}

/// Pytest's `<failure>` or `<error>` child element. The interesting
/// payload is the `message="…"` attribute (a one-line summary, e.g.
/// `assert 1 == 2`); the element's text content carries the full
/// traceback which is too long for a finding row.
fn extract_failure_message(body: &str) -> Option<String> {
    for tag in ["<failure", "<error"] {
        if let Some(start) = body.find(tag) {
            let after = start + tag.len();
            if let Some(end) = find_tag_close(&body[after..]) {
                let attrs = &body[after..after + end];
                if let Some(msg) = attr_value(attrs, "message") {
                    return Some(msg);
                }
                // Some emitters (xdist, pytest plugins) skip the
                // attribute and put the message inline. Fall back to
                // the element's text content, trimmed to the first
                // line so the finding stays one row tall.
                let body_start = after + end + 1;
                let close_token = if tag == "<failure" {
                    "</failure>"
                } else {
                    "</error>"
                };
                if let Some(close_off) = body[body_start..].find(close_token) {
                    let inner = &body[body_start..body_start + close_off];
                    let first_line = inner.lines().find(|l| !l.trim().is_empty())?;
                    return Some(decode_entities(first_line.trim()));
                }
            }
        }
    }
    None
}

fn decode_entities(s: &str) -> String {
    let unescaped = s
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    strip_ansi(&unescaped)
}

/// Remove ANSI CSI escape sequences (e.g. color codes) from a string.
/// Pytest emits ANSI when `TERM` is set in the gate's env; carrying the
/// raw `\x1b[31m…\x1b[0m` markers through to the agent's stderr renders
/// as visual noise rather than red text. We strip on read because the
/// JUnit XML carries `message=` attribute values verbatim.
fn strip_ansi(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            // CSI sequence: ESC `[` <params> <intermediate> <final>
            // <final> is in the range 0x40..=0x7e, terminating the seq.
            let mut j = i + 2;
            while j < bytes.len() && !(0x40..=0x7e).contains(&bytes[j]) {
                j += 1;
            }
            i = j.saturating_add(1);
        } else {
            // Walk by char boundaries so multi-byte UTF-8 stays intact.
            let ch_end = (i + 1..=bytes.len())
                .find(|&k| s.is_char_boundary(k))
                .unwrap_or(bytes.len());
            out.push_str(&s[i..ch_end]);
            i = ch_end;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_testcases_yields_empty() {
        let xml = r#"<?xml version="1.0"?><testsuites/>"#;
        assert!(collect_failures("t", xml).is_empty());
    }

    #[test]
    fn passing_testcases_only_yields_empty() {
        let xml = r#"<testsuites><testsuite name="pytest" tests="2">
            <testcase classname="x" name="t1" file="t.py" line="1"/>
            <testcase classname="x" name="t2" file="t.py" line="2"/>
        </testsuite></testsuites>"#;
        assert!(collect_failures("tests", xml).is_empty());
    }

    #[test]
    fn one_failure_yields_one_finding() {
        let xml = r#"<testsuites><testsuite>
            <testcase classname="t.x" name="test_a" file="tests/x.py" line="7">
                <failure message="assert 1 == 2">stacktrace</failure>
            </testcase>
        </testsuite></testsuites>"#;
        let findings = collect_failures("tests", xml);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("test_a"));
        assert!(findings[0].message.contains("assert 1 == 2"));
        assert_eq!(findings[0].file.as_deref(), Some("tests/x.py"));
        assert_eq!(findings[0].line, Some(7));
        assert_eq!(findings[0].severity, Severity::Error);
        assert!(findings[0].rule.contains("failure"));
    }

    #[test]
    fn error_element_treated_like_failure() {
        // Collection-time errors emit `<error>` rather than `<failure>`;
        // both block the run and both should surface as findings.
        let xml = r#"<testsuites><testsuite>
            <testcase classname="t.x" name="test_a">
                <error message="ImportError: no module">stacktrace</error>
            </testcase>
        </testsuite></testsuites>"#;
        let findings = collect_failures("tests", xml);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("ImportError"));
    }

    #[test]
    fn entity_decoding_in_message() {
        let xml = r#"<testsuites><testsuite>
            <testcase name="test_a">
                <failure message="assert 'a' &lt; 'b'">x</failure>
            </testcase>
        </testsuite></testsuites>"#;
        let findings = collect_failures("tests", xml);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("'a' < 'b'"));
    }

    #[test]
    fn classname_and_name_concatenated() {
        let xml = r#"<testsuites><testsuite>
            <testcase classname="tests.x.TestY" name="test_z">
                <failure message="m"/>
            </testcase>
        </testsuite></testsuites>"#;
        let findings = collect_failures("t", xml);
        assert!(findings[0].message.contains("tests.x.TestY::test_z"));
    }

    #[test]
    fn handles_self_closing_failure_element() {
        let xml = r#"<testsuites><testsuite>
            <testcase name="t">
                <failure message="boom"/>
            </testcase>
        </testsuite></testsuites>"#;
        let findings = collect_failures("t", xml);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("boom"));
    }

    #[test]
    fn cap_caps_at_max_findings() {
        // Generate 60 failing testcases; the cap should hold at
        // MAX_FINDINGS (50).
        let mut xml = String::from("<testsuites><testsuite>");
        for i in 0..60 {
            xml.push_str(&format!(
                r#"<testcase name="t{i}"><failure message="m{i}"/></testcase>"#
            ));
        }
        xml.push_str("</testsuite></testsuites>");
        let findings = collect_failures("t", &xml);
        assert_eq!(findings.len(), MAX_FINDINGS);
    }

    #[test]
    fn summarise_pluralisation() {
        let one = vec![Finding {
            rule: "r".into(),
            message: "m".into(),
            file: None,
            line: None,
            severity: Severity::Error,
        }];
        assert!(summarise_failures(&one).contains("1 test"));

        let three: Vec<Finding> = (0..3)
            .map(|_| Finding {
                rule: "r".into(),
                message: "m".into(),
                file: None,
                line: None,
                severity: Severity::Error,
            })
            .collect();
        let s = summarise_failures(&three);
        assert!(s.contains("3"));
        assert!(s.contains("test failures"));
    }

    #[test]
    fn attr_value_does_not_match_substring() {
        // `classname="x"` must not be returned as the value of `name`.
        let attrs = r#"classname="t.x" name="test_a""#;
        assert_eq!(attr_value(attrs, "name").as_deref(), Some("test_a"));
        assert_eq!(attr_value(attrs, "classname").as_deref(), Some("t.x"));
    }
}
