//! JUnit XML (Surefire 3.0 schema) formatter for `klasp gate`.

use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;

use klasp_core::{Finding, Severity, Verdict, VerdictPolicy};

/// Render the verdict as JUnit XML. Always returns valid, UTF-8 encoded XML.
pub fn render(verdict: &Verdict, _policy: VerdictPolicy) -> String {
    let mut buf = Vec::new();
    let mut w = Writer::new_with_indent(&mut buf, b' ', 2);

    w.write_event(Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)))
        .unwrap();

    let (findings, failure_count, error_count) = match verdict {
        Verdict::Pass => (&[][..], 0usize, 0usize),
        Verdict::Warn { findings, .. } => (findings.as_slice(), 0, 0),
        Verdict::Fail { findings, .. } => {
            let failures = findings
                .iter()
                .filter(|f| matches!(f.severity, Severity::Error))
                .count();
            (findings.as_slice(), failures, 0usize)
        }
    };

    let test_count = if findings.is_empty() {
        1
    } else {
        findings.len()
    };

    let mut suite_elem = BytesStart::new("testsuites");
    suite_elem.push_attribute(("name", "klasp"));
    suite_elem.push_attribute(("tests", test_count.to_string().as_str()));
    suite_elem.push_attribute(("failures", failure_count.to_string().as_str()));
    suite_elem.push_attribute(("errors", error_count.to_string().as_str()));
    w.write_event(Event::Start(suite_elem)).unwrap();

    let mut ts_elem = BytesStart::new("testsuite");
    ts_elem.push_attribute(("name", "klasp"));
    ts_elem.push_attribute(("tests", test_count.to_string().as_str()));
    ts_elem.push_attribute(("failures", failure_count.to_string().as_str()));
    ts_elem.push_attribute(("errors", error_count.to_string().as_str()));
    ts_elem.push_attribute(("time", "0"));
    w.write_event(Event::Start(ts_elem)).unwrap();

    if findings.is_empty() {
        write_passing_case(&mut w);
    } else {
        for f in findings {
            write_finding_case(&mut w, f);
        }
    }

    w.write_event(Event::End(BytesEnd::new("testsuite")))
        .unwrap();
    w.write_event(Event::End(BytesEnd::new("testsuites")))
        .unwrap();

    let mut result = String::from_utf8(buf).expect("quick-xml produced non-UTF-8");
    result.push('\n');
    result
}

fn write_passing_case(w: &mut Writer<&mut Vec<u8>>) {
    let mut tc = BytesStart::new("testcase");
    tc.push_attribute(("classname", "klasp"));
    tc.push_attribute(("name", "gate"));
    tc.push_attribute(("time", "0"));
    w.write_event(Event::Empty(tc)).unwrap();
}

fn write_finding_case(w: &mut Writer<&mut Vec<u8>>, f: &Finding) {
    let classname = f.file.as_deref().unwrap_or(&f.rule);
    let name = match (f.file.as_deref(), f.line) {
        (Some(file), Some(line)) => format!("{}:{}:{}", f.rule, file, line),
        (Some(file), None) => format!("{}:{}", f.rule, file),
        _ => f.rule.clone(),
    };
    let is_error = matches!(f.severity, Severity::Error);
    if is_error {
        let mut tc = BytesStart::new("testcase");
        tc.push_attribute(("classname", classname));
        tc.push_attribute(("name", name.as_str()));
        tc.push_attribute(("time", "0"));
        w.write_event(Event::Start(tc)).unwrap();
        let mut fail_elem = BytesStart::new("failure");
        fail_elem.push_attribute(("message", f.message.as_str()));
        w.write_event(Event::Start(fail_elem)).unwrap();
        w.write_event(Event::Text(BytesText::new(&f.message)))
            .unwrap();
        w.write_event(Event::End(BytesEnd::new("failure"))).unwrap();
        w.write_event(Event::End(BytesEnd::new("testcase")))
            .unwrap();
    } else {
        let warn_name = format!("{name} [{}]", severity_tag(f.severity));
        let mut tc = BytesStart::new("testcase");
        tc.push_attribute(("classname", classname));
        tc.push_attribute(("name", warn_name.as_str()));
        tc.push_attribute(("time", "0"));
        w.write_event(Event::Empty(tc)).unwrap();
    }
}

fn severity_tag(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warn => "warn",
        Severity::Info => "info",
    }
}
