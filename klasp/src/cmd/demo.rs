//! `klasp demo` — replay a captured agent session fixture and verify the
//! structured-verdict feedback loop.
//!
//! Loads a JSONL session transcript and checks that every time the gate
//! blocked a commit (`klasp-gate: blocked` in a `tool_result`), the very
//! next assistant message references at least one filename mentioned in the
//! verdict. This proves the agent parsed and acted on the structured output.

use std::process::ExitCode;

use crate::cli::DemoArgs;

/// Mirrors `cmd::gate::NOTICE_PREFIX` + " blocked" — kept local to avoid
/// coupling demo's fixture parser to gate's internal constant.
const GATE_BLOCKED_MARKER: &str = "klasp-gate: blocked";

/// One gate-block sequence parsed from the fixture.
struct GateBlock {
    /// 1-based index of the tool_result line that carried the block.
    tool_result_line: usize,
    /// Filenames extracted from the verdict (e.g. `["src/lib/foo.ts"]`).
    expected_filenames: Vec<String>,
    /// The assistant message content that followed the block (if any).
    assistant_response: Option<String>,
    /// 1-based line index of the assistant message.
    assistant_line: Option<usize>,
}

pub fn run(args: &DemoArgs) -> ExitCode {
    match try_run(args) {
        Ok(exit) => exit,
        Err(e) => {
            eprintln!("klasp demo: {e}");
            ExitCode::FAILURE
        }
    }
}

fn try_run(args: &DemoArgs) -> Result<ExitCode, String> {
    let fixture_path = &args.fixture;
    let raw = std::fs::read_to_string(fixture_path).map_err(|e| {
        format!(
            "fixture file not found or unreadable: {}: {e}",
            fixture_path.display()
        )
    })?;

    let mut lines: Vec<serde_json::Value> = Vec::new();
    let mut line_numbers: Vec<usize> = Vec::new(); // 1-based source line per parsed value
    for (i, raw_line) in raw.lines().enumerate() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(trimmed)
            .map_err(|e| format!("line {}: malformed JSON: {e}", i + 1))?;
        lines.push(v);
        line_numbers.push(i + 1);
    }

    let mut blocks: Vec<GateBlock> = Vec::new();

    let mut idx = 0usize;
    while idx < lines.len() {
        let v = &lines[idx];
        let line_no = line_numbers[idx];

        if is_gate_block(v) {
            let output = get_tool_result_output(v).unwrap_or_default();
            let expected_filenames = extract_filenames(output);

            if args.verbose {
                println!(
                    "[demo] gate block at line {line_no}: {} filename(s) in verdict",
                    expected_filenames.len()
                );
                for f in &expected_filenames {
                    println!("  expected filename: {f}");
                }
            }

            // Find the next assistant message after this tool_result.
            let mut assistant_response: Option<String> = None;
            let mut assistant_line: Option<usize> = None;
            for j in (idx + 1)..lines.len() {
                if is_assistant_message(&lines[j]) {
                    assistant_response = get_message_content(&lines[j]).map(str::to_owned);
                    assistant_line = Some(line_numbers[j]);
                    break;
                }
            }

            if args.verbose {
                if let Some(ref content) = assistant_response {
                    let snippet: String = content.chars().take(120).collect();
                    println!(
                        "  assistant response (line {}): {snippet}",
                        assistant_line.unwrap_or(0)
                    );
                } else {
                    println!("  assistant response: (none found)");
                }
            }

            blocks.push(GateBlock {
                tool_result_line: line_no,
                expected_filenames,
                assistant_response,
                assistant_line,
            });
        }

        idx += 1;
    }

    if blocks.is_empty() {
        println!("warning: no klasp-gate blocked sequences found in fixture — nothing to verify");
        return Ok(ExitCode::SUCCESS);
    }

    let mut failed = 0usize;
    for (n, block) in blocks.iter().enumerate() {
        let loop_label = n + 1;
        let Some(ref response) = block.assistant_response else {
            eprintln!(
                "demo replay: FAIL loop {loop_label} (gate block at line {}): \
                 no assistant message found after gate block",
                block.tool_result_line
            );
            failed += 1;
            continue;
        };

        let found = block
            .expected_filenames
            .iter()
            .any(|f| response.contains(f.as_str()));

        if found {
            if args.verbose {
                println!("[demo] loop {loop_label}: OK");
            }
        } else {
            eprintln!(
                "demo replay: FAIL loop {loop_label} (gate block at line {}, assistant at line {}):",
                block.tool_result_line,
                block.assistant_line.unwrap_or(0)
            );
            eprintln!(
                "  expected assistant message to reference at least one of: {}",
                block.expected_filenames.join(", ")
            );
            let snippet: String = response.chars().take(200).collect();
            eprintln!("  assistant message: {snippet:?}");
            failed += 1;
        }
    }

    if failed == 0 {
        let n = blocks.len();
        println!(
            "demo replay: OK ({n} gate-feedback loop{} verified)",
            if n == 1 { "" } else { "s" }
        );
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

fn is_gate_block(v: &serde_json::Value) -> bool {
    if v.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
        return false;
    }
    get_tool_result_output(v).is_some_and(|o| o.contains(GATE_BLOCKED_MARKER))
}

fn get_tool_result_output(v: &serde_json::Value) -> Option<&str> {
    v.get("output").and_then(|o| o.as_str())
}

fn is_assistant_message(v: &serde_json::Value) -> bool {
    v.get("type").and_then(|t| t.as_str()) == Some("message")
        && v.get("role").and_then(|r| r.as_str()) == Some("assistant")
}

fn get_message_content(v: &serde_json::Value) -> Option<&str> {
    v.get("content").and_then(|c| c.as_str())
}

/// Extract `path/to/file.ext` tokens from a verdict string.
/// Matches tokens of the form `<path>:<digit>` — the line/col suffix is stripped.
fn extract_filenames(output: &str) -> Vec<String> {
    let mut results = Vec::new();
    // Walk each whitespace-separated token and check for file:line patterns.
    for token in output.split_whitespace() {
        // Strip surrounding punctuation like `'`, `"`, `(`, `)`.
        let token = token.trim_matches(|c: char| {
            !c.is_alphanumeric() && c != '/' && c != '.' && c != '_' && c != '-'
        });
        // Find a colon followed by a digit — that's the file:line boundary.
        if let Some(colon_pos) = token.find(':') {
            let after_colon = &token[colon_pos + 1..];
            if after_colon.starts_with(|c: char| c.is_ascii_digit()) {
                let path_part = &token[..colon_pos];
                // Must look like a file path: contains a dot.
                if path_part.contains('.') && !path_part.is_empty() {
                    let s = path_part.to_owned();
                    if !results.contains(&s) {
                        results.push(s);
                    }
                }
            }
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_filenames_simple() {
        let output = "  - [error][tsc] src/lib/foo.ts:3:1 — Cannot find module";
        let files = extract_filenames(output);
        assert_eq!(files, vec!["src/lib/foo.ts"]);
    }

    #[test]
    fn extract_filenames_multiple_unique() {
        let output = "src/a.ts:1:1 — err\n  src/b.rs:20:5 — warn\n  src/a.ts:5:2 — err2";
        let files = extract_filenames(output);
        assert_eq!(files, vec!["src/a.ts", "src/b.rs"]);
    }

    #[test]
    fn extract_filenames_no_match() {
        let output = "klasp-gate: blocked (1 errors, policy=AnyFail)";
        let files = extract_filenames(output);
        assert!(files.is_empty());
    }

    #[test]
    fn is_gate_block_detects_blocked() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"type":"tool_result","tool_call_id":"tc_1","output":"klasp-gate: blocked (1 errors)","exit_code":1}"#,
        )
        .unwrap();
        assert!(is_gate_block(&v));
    }

    #[test]
    fn is_gate_block_ignores_passing() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"type":"tool_result","tool_call_id":"tc_1","output":"OK","exit_code":0}"#,
        )
        .unwrap();
        assert!(!is_gate_block(&v));
    }
}
