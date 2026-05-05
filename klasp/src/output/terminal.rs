//! Human-readable terminal output formatter for `klasp gate`.

use klasp_core::{Severity, Verdict, VerdictPolicy};

/// Render the verdict as a human-readable block of text suitable for writing
/// to stderr. Returns an empty string on `Verdict::Pass` (quiet happy path).
pub fn render(verdict: &Verdict, policy: VerdictPolicy) -> String {
    const PREFIX: &str = "klasp-gate:";
    let mut out = String::new();
    match verdict {
        Verdict::Pass => {}
        Verdict::Warn { findings, message } => {
            out.push_str(&format!(
                "{PREFIX} warnings ({} findings):\n",
                findings.len()
            ));
            if let Some(m) = message {
                out.push_str(&format!("  {m}\n"));
            }
            for f in findings {
                out.push_str(&format!("  - [{}] {}\n", f.rule, f.message));
            }
        }
        Verdict::Fail { findings, message } => {
            let error_count = findings
                .iter()
                .filter(|f| matches!(f.severity, Severity::Error))
                .count();
            out.push_str(&format!(
                "{PREFIX} blocked ({error_count} errors, {} findings total, policy={policy:?}):\n",
                findings.len(),
            ));
            out.push_str(&format!("{message}\n"));
            for f in findings {
                let location = match (f.file.as_deref(), f.line) {
                    (Some(file), Some(line)) => format!(" ({file}:{line})"),
                    (Some(file), None) => format!(" ({file})"),
                    _ => String::new(),
                };
                let tag = match f.severity {
                    Severity::Error => "error",
                    Severity::Warn => "warn",
                    Severity::Info => "info",
                };
                out.push_str(&format!(
                    "  - [{tag}][{}] {}{location}\n",
                    f.rule, f.message
                ));
            }
        }
    }
    out
}
