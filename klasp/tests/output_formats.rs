//! Integration tests for JUnit XML and SARIF 2.1.0 output formatters.

use klasp_core::{Finding, Severity, Verdict, VerdictPolicy};

fn make_fail_verdict() -> Verdict {
    Verdict::Fail {
        findings: vec![
            Finding {
                rule: "test-failure".into(),
                message: "Expected 1 + 1 == 3".into(),
                file: Some("src/math.rs".into()),
                line: Some(10),
                severity: Severity::Error,
            },
            Finding {
                rule: "lint-warning".into(),
                message: "unused variable `x`".into(),
                file: Some("src/lib.rs".into()),
                line: Some(5),
                severity: Severity::Warn,
            },
            Finding {
                rule: "test-failure".into(),
                message: "assertion failed: a < b".into(),
                file: Some("src/compare.rs".into()),
                line: Some(20),
                severity: Severity::Error,
            },
        ],
        message: "2 test(s) failed".into(),
    }
}

mod junit {
    use super::*;
    use klasp::output::junit;

    #[test]
    fn pass_verdict_renders_single_passing_testcase() {
        let xml = junit::render(&Verdict::Pass, VerdictPolicy::AnyFail);
        assert!(
            xml.contains("<testcase"),
            "must contain at least one testcase"
        );
        assert!(
            !xml.contains("<failure"),
            "pass verdict must not contain failure elements"
        );
        assert!(xml.contains("tests=\"1\""), "test count must be 1 for pass");
        assert!(
            xml.contains("failures=\"0\""),
            "failure count must be 0 for pass"
        );
    }

    #[test]
    fn fail_verdict_renders_failure_testcases() {
        let xml = junit::render(&make_fail_verdict(), VerdictPolicy::AnyFail);
        let failure_count = xml.matches("<failure").count();
        assert_eq!(
            failure_count, 2,
            "two error-severity findings need two <failure> elements"
        );
    }

    #[test]
    fn warn_finding_renders_as_passing_testcase() {
        let xml = junit::render(&make_fail_verdict(), VerdictPolicy::AnyFail);
        assert!(
            xml.contains("lint-warning"),
            "warn finding rule must appear in output"
        );
    }

    #[test]
    fn aggregate_counts_match_findings() {
        let xml = junit::render(&make_fail_verdict(), VerdictPolicy::AnyFail);
        assert!(
            xml.contains("tests=\"3\""),
            "test count must match finding count\n{xml}"
        );
        assert!(
            xml.contains("failures=\"2\""),
            "failure count must be 2\n{xml}"
        );
        assert!(
            xml.contains("errors=\"0\""),
            "errors count must be 0\n{xml}"
        );
    }

    #[test]
    fn xml_is_well_formed() {
        let xml = junit::render(&make_fail_verdict(), VerdictPolicy::AnyFail);
        let mut reader = quick_xml::Reader::from_str(&xml);
        reader.config_mut().check_end_names = true;
        let mut depth = 0i32;
        loop {
            match reader.read_event() {
                Ok(quick_xml::events::Event::Start(_)) => depth += 1,
                Ok(quick_xml::events::Event::End(_)) => depth -= 1,
                Ok(quick_xml::events::Event::Eof) => break,
                Err(e) => panic!("JUnit XML is not well-formed: {e}\n{xml}"),
                _ => {}
            }
        }
        assert_eq!(depth, 0, "unbalanced XML tags\n{xml}");
    }

    #[test]
    fn file_and_line_appear_in_classname_name() {
        let xml = junit::render(&make_fail_verdict(), VerdictPolicy::AnyFail);
        assert!(
            xml.contains("test-failure:src/math.rs:10"),
            "name must encode rule:file:line\n{xml}"
        );
    }

    #[test]
    fn xml_message_is_escaped() {
        let verdict = Verdict::Fail {
            findings: vec![Finding {
                rule: "xss".into(),
                message: "<script>alert('xss')</script>".into(),
                file: None,
                line: None,
                severity: Severity::Error,
            }],
            message: "xss test".into(),
        };
        let xml = junit::render(&verdict, VerdictPolicy::AnyFail);
        assert!(
            !xml.contains("<script>"),
            "raw angle brackets must be escaped\n{xml}"
        );
        assert!(
            xml.contains("&lt;script&gt;"),
            "must contain escaped angle brackets\n{xml}"
        );
    }

    #[test]
    fn matches_golden_fixture() {
        let xml = junit::render(&make_fail_verdict(), VerdictPolicy::AnyFail);
        let golden_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/output/junit_basic.xml"
        );
        let golden = std::fs::read_to_string(golden_path)
            .unwrap_or_else(|_| panic!("golden fixture missing: {golden_path}\n{xml}"));
        assert_xml_equal(&xml, &golden);
    }

    fn assert_xml_equal(actual: &str, expected: &str) {
        let norm = |s: &str| {
            s.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .replace("> <", "><")
        };
        assert_eq!(
            norm(actual),
            norm(expected),
            "JUnit XML does not match golden fixture"
        );
    }
}

mod sarif {
    use super::*;
    use klasp::output::sarif;

    #[test]
    fn pass_verdict_renders_empty_results() {
        let json = sarif::render(&Verdict::Pass, VerdictPolicy::AnyFail);
        let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        let results = &v["runs"][0]["results"];
        assert!(
            results.as_array().is_some_and(|a| a.is_empty()),
            "pass verdict must have empty results array\n{json}"
        );
    }

    #[test]
    fn version_field_is_2_1_0() {
        let json = sarif::render(&Verdict::Pass, VerdictPolicy::AnyFail);
        let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        assert_eq!(v["version"], "2.1.0", "SARIF version must be 2.1.0");
    }

    #[test]
    fn schema_field_is_present() {
        let json = sarif::render(&Verdict::Pass, VerdictPolicy::AnyFail);
        let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        assert!(
            v["$schema"].is_string(),
            "$schema field must be present\n{json}"
        );
    }

    #[test]
    fn tool_driver_name_is_klasp() {
        let json = sarif::render(&Verdict::Pass, VerdictPolicy::AnyFail);
        let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        assert_eq!(
            v["runs"][0]["tool"]["driver"]["name"], "klasp",
            "tool driver name must be klasp"
        );
    }

    #[test]
    fn results_count_matches_findings() {
        let json = sarif::render(&make_fail_verdict(), VerdictPolicy::AnyFail);
        let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        let results = v["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), 3, "must have one result per finding");
    }

    #[test]
    fn error_severity_maps_to_error_level() {
        let json = sarif::render(&make_fail_verdict(), VerdictPolicy::AnyFail);
        let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        let results = v["runs"][0]["results"].as_array().unwrap();
        let error_results: Vec<_> = results.iter().filter(|r| r["level"] == "error").collect();
        assert_eq!(
            error_results.len(),
            2,
            "two error-severity findings -> two error level results"
        );
    }

    #[test]
    fn warn_severity_maps_to_warning_level() {
        let json = sarif::render(&make_fail_verdict(), VerdictPolicy::AnyFail);
        let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        let results = v["runs"][0]["results"].as_array().unwrap();
        let warn_results: Vec<_> = results.iter().filter(|r| r["level"] == "warning").collect();
        assert_eq!(
            warn_results.len(),
            1,
            "one warn-severity finding -> one warning level result"
        );
    }

    #[test]
    fn rules_deduplicated_by_rule_id() {
        let json = sarif::render(&make_fail_verdict(), VerdictPolicy::AnyFail);
        let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        let rules = v["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
        let rule_ids: Vec<_> = rules.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(rule_ids.len(), 2, "rules must be deduplicated");
        assert!(rule_ids.contains(&"test-failure"));
        assert!(rule_ids.contains(&"lint-warning"));
    }

    #[test]
    fn location_encodes_file_and_line() {
        let json = sarif::render(&make_fail_verdict(), VerdictPolicy::AnyFail);
        let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        let results = v["runs"][0]["results"].as_array().unwrap();
        let math_result = results
            .iter()
            .find(|r| {
                r["locations"][0]["physicalLocation"]["artifactLocation"]["uri"] == "src/math.rs"
            })
            .expect("must have a result for src/math.rs");
        assert_eq!(
            math_result["locations"][0]["physicalLocation"]["region"]["startLine"],
            10
        );
    }

    #[test]
    fn matches_golden_fixture() {
        let json = sarif::render(&make_fail_verdict(), VerdictPolicy::AnyFail);
        let golden_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/output/sarif_basic.json"
        );
        let golden = std::fs::read_to_string(golden_path)
            .unwrap_or_else(|_| panic!("golden fixture missing: {golden_path}\n{json}"));
        let mut actual: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        let mut expected: serde_json::Value =
            serde_json::from_str(&golden).expect("invalid golden JSON");
        // tool.driver.version is sourced from CARGO_PKG_VERSION at build time;
        // strip it from both sides so the fixture survives version bumps.
        strip_driver_version(&mut actual);
        strip_driver_version(&mut expected);
        assert_eq!(actual, expected, "SARIF JSON does not match golden fixture");
    }

    fn strip_driver_version(v: &mut serde_json::Value) {
        if let Some(driver) = v
            .pointer_mut("/runs/0/tool/driver")
            .and_then(|d| d.as_object_mut())
        {
            driver.remove("version");
        }
    }

    /// SARIF 2.1.0 §3.27.12: `result.locations`, if present, must be a
    /// non-empty array. Omit the field entirely when the finding carries
    /// no physical location.
    #[test]
    fn finding_without_location_omits_locations_key() {
        let verdict = Verdict::Fail {
            findings: vec![Finding {
                rule: "no-loc".into(),
                message: "no source coordinates".into(),
                file: None,
                line: None,
                severity: Severity::Error,
            }],
            message: "missing-loc test".into(),
        };
        let json = sarif::render(&verdict, VerdictPolicy::AnyFail);
        let v: serde_json::Value = serde_json::from_str(&json).expect("invalid JSON");
        let result = v
            .pointer("/runs/0/results/0")
            .and_then(|r| r.as_object())
            .expect("result[0] missing");
        assert!(
            !result.contains_key("locations"),
            "locations must be omitted when empty per SARIF 2.1.0 minItems=1\n{json}"
        );
    }
}
