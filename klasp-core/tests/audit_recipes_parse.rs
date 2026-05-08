//! Smoke test for the per-language audit recipe docs under
//! `docs/audits/`. Every fenced ```` ```toml ```` block in those docs is
//! extracted and parsed against [`klasp_core::ConfigV1`]. Drift between
//! the schema and the recipe docs (renamed fields, deleted variants,
//! tightened `deny_unknown_fields`) shows up as a test failure here
//! before it ships.
//!
//! ## What gets parsed
//!
//! The TOML blocks fall into two shapes:
//!
//! 1. **Whole configs** — start with `version = 1` and parse straight
//!    through [`ConfigV1::parse`].
//! 2. **Fragments** — bare `[[checks]]`, `[gate]`, or `[[trigger]]`
//!    blocks that illustrate a single concept and intentionally omit
//!    the surrounding scaffolding. These are detected by the absence
//!    of `version = 1` at the top of the block and wrapped with a
//!    minimal valid prelude before parsing so the documented snippet
//!    is still validated against the live schema.
//!
//! ## On failure
//!
//! Failure messages include the source markdown file, the 1-indexed
//! block number within that file, the line range of the block in the
//! original markdown, and the TOML parse error. The original snippet
//! is reproduced in the failure output so reviewers can find the
//! offending lines without re-running the test under `--nocapture`.

use klasp_core::ConfigV1;

/// Minimal valid prelude prepended to fragment-shaped TOML blocks
/// before parsing. Keep this in sync with the smallest config that
/// satisfies `ConfigV1`'s required fields (`version`, `[gate]`).
const FRAGMENT_PRELUDE: &str = "version = 1\n[gate]\nagents = [\"claude_code\"]\npolicy = \"any_fail\"\n";

/// `[gate]`-only fragments (recipe docs use these to illustrate a
/// single policy switch like `policy = "majority_fail"`) need to
/// *replace* the prelude's `[gate]` block — appending a second
/// `[gate]` produces a duplicate-table TOML parse error. This prelude
/// supplies only the `version` line so the fragment's `[gate]` lands
/// at the document root.
const GATE_FRAGMENT_PRELUDE: &str = "version = 1\n";

/// Single-field illustrations like `command = "tsgo --noEmit --incremental"`
/// are common in the recipe docs (showing what one field would look
/// like inside a `[checks.source]` block). Wrap them in a full check
/// scaffold so the field's own type is what the parser validates.
const BARE_FIELD_PRELUDE: &str =
    "version = 1\n[gate]\nagents = [\"claude_code\"]\npolicy = \"any_fail\"\n\
     [[checks]]\nname = \"doc-snippet\"\n[checks.source]\ntype = \"shell\"\n";

/// One TOML block extracted from a markdown audit recipe.
struct TomlBlock {
    /// Index of the block within its source file, 1-based, used in
    /// failure messages so reviewers can grep the doc directly.
    index: usize,
    /// 1-indexed line numbers of the opening and closing ` ``` ` fences
    /// in the source markdown — also used in failure messages.
    start_line: usize,
    end_line: usize,
    /// Raw block contents between the fences, without the fence lines.
    body: String,
}

/// Bundles the full text of an audit recipe with the file path used in
/// failure messages. The path is purely informational — the body comes
/// from `include_str!` so the test is hermetic and runs from any CWD.
struct AuditDoc {
    path: &'static str,
    source: &'static str,
}

/// All four single-stack audit recipes shipped under `docs/audits/`.
/// Each new recipe doc must be appended here so its TOML blocks are
/// covered by the smoke test. Listed explicitly rather than discovered
/// at compile time so reviewers can grep for the file set.
const AUDIT_DOCS: &[AuditDoc] = &[
    AuditDoc {
        path: "docs/audits/python.md",
        source: include_str!("../../docs/audits/python.md"),
    },
    AuditDoc {
        path: "docs/audits/typescript.md",
        source: include_str!("../../docs/audits/typescript.md"),
    },
    AuditDoc {
        path: "docs/audits/rust.md",
        source: include_str!("../../docs/audits/rust.md"),
    },
    AuditDoc {
        path: "docs/audits/go.md",
        source: include_str!("../../docs/audits/go.md"),
    },
    AuditDoc {
        path: "docs/audits/polyglot.md",
        source: include_str!("../../docs/audits/polyglot.md"),
    },
    AuditDoc {
        path: "docs/audits/monorepo.md",
        source: include_str!("../../docs/audits/monorepo.md"),
    },
];

/// Walk a markdown document and collect every fenced ` ```toml ` block.
///
/// Recognises only the exact opening fence `` ```toml `` (case-sensitive,
/// no extra info-string after `toml`) — siblings like ` ```bash `,
/// ` ```yaml `, ` ```jsonc `, ` ```text ` are skipped. The closing fence
/// is the first line that is exactly ` ``` ` after a `toml` opener.
///
/// Nested fences inside a TOML block aren't a thing in TOML, so a simple
/// state machine suffices. If a doc author ever embeds a `` ``` `` literal
/// inside a TOML string, the loop will close early — that's a doc-side
/// pathology this test is happy to surface as a parse failure.
fn extract_toml_blocks(source: &str) -> Vec<TomlBlock> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut current = String::new();
    let mut start_line: usize = 0;
    let mut index: usize = 0;

    for (line_idx, line) in source.lines().enumerate() {
        let line_no = line_idx + 1;
        let trimmed = line.trim_start();

        if !in_block {
            // Reject `` ```toml-ish `` or `` ```toml,custom `` — only
            // accept ` ```toml ` exactly or with trailing whitespace.
            if trimmed == "```toml" || trimmed.starts_with("```toml ") {
                in_block = true;
                start_line = line_no;
                current.clear();
            }
        } else if trimmed == "```" {
            index += 1;
            blocks.push(TomlBlock {
                index,
                start_line,
                end_line: line_no,
                body: std::mem::take(&mut current),
            });
            in_block = false;
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }

    blocks
}

/// Classification of a TOML block, drives which prelude (if any) gets
/// prepended before [`ConfigV1::parse`] sees the snippet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    /// Block contains `version = 1` at top — feed straight to the parser.
    WholeConfig,
    /// Block is a `[gate]` fragment (e.g. illustrating a policy change).
    /// Wrap with [`GATE_FRAGMENT_PRELUDE`] only — the fragment's own
    /// `[gate]` table lands at the document root.
    GateFragment,
    /// Block has at least one table header (`[[checks]]`, `[[trigger]]`,
    /// `[checks.source]`, …) but no `[gate]` and no `version`. Wrap with
    /// [`FRAGMENT_PRELUDE`].
    StructuredFragment,
    /// Block is a single bare `key = value` line (or several) with no
    /// table headers. Wrap with [`BARE_FIELD_PRELUDE`] which lands the
    /// fields inside a `[checks.source]` `type = "shell"` block.
    BareFieldFragment,
    /// Block doesn't match any known shape. The test panics on these so
    /// a new fragment style added in a future doc forces a classifier
    /// update rather than silently parsing inside the wrong wrapper.
    Unknown,
}

/// Match a top-level `version = N` field, rejecting near-misses like
/// `versionless = 1` or `version_note = "…"`.
fn is_version_field(line: &str) -> bool {
    let key = line.split('=').next().unwrap_or("").trim();
    key == "version"
}

/// Match the `[gate]` table header exactly, allowing trailing whitespace
/// or an inline `# comment`. Excludes `[gate.subtable]` or `[gateway]`.
fn is_gate_table_header(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("[gate]") else {
        return false;
    };
    let rest = rest.trim_start();
    rest.is_empty() || rest.starts_with('#')
}

/// True when every non-blank, non-comment line is a simple
/// `key = "scalar"` assignment with no arrays or inline tables. Required
/// before wrapping inside `[checks.source]`; arrays at the top level
/// (e.g. `triggers = [...]`) would parse spuriously inside that table.
fn is_simple_bare_field(body: &str) -> bool {
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(eq_idx) = line.find('=') else {
            return false;
        };
        let value = line[eq_idx + 1..].trim();
        if value.starts_with('[') || value.starts_with('{') {
            return false;
        }
    }
    true
}

/// Classify a block by inspecting its non-comment, non-blank lines for
/// the `version` declaration and any TOML table headers (`[…]`).
fn classify(body: &str) -> BlockKind {
    let mut has_version = false;
    let mut has_gate_header = false;
    let mut has_other_header = false;

    for line in body.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if is_version_field(line) {
            has_version = true;
        }
        if line.starts_with('[') {
            if is_gate_table_header(line) {
                has_gate_header = true;
            } else {
                has_other_header = true;
            }
        }
    }

    if has_version {
        BlockKind::WholeConfig
    } else if has_gate_header && !has_other_header {
        BlockKind::GateFragment
    } else if has_other_header || has_gate_header {
        BlockKind::StructuredFragment
    } else if is_simple_bare_field(body) {
        BlockKind::BareFieldFragment
    } else {
        BlockKind::Unknown
    }
}

#[derive(Debug, Default)]
struct ParseStats {
    whole_configs: usize,
    structured_fragments: usize,
    gate_fragments: usize,
    bare_field_fragments: usize,
}

/// Smoke test: every TOML snippet in the audit recipes must parse
/// cleanly against the current `ConfigV1` schema.
///
/// Wave-1 reported ~29 snippets across the four single-stack docs.
/// Wave-2's job is to catch drift between those snippets and the
/// schema — a renamed field, a tightened `deny_unknown_fields`, or a
/// recipe variant rename will fail here loudly with file:line
/// coordinates so the doc author can pinpoint the bad block.
#[test]
fn audit_recipe_toml_snippets_parse() {
    let mut stats = ParseStats::default();
    let mut total_blocks: usize = 0;
    let mut failures: Vec<String> = Vec::new();

    for doc in AUDIT_DOCS {
        let blocks = extract_toml_blocks(doc.source);
        total_blocks += blocks.len();

        for block in &blocks {
            // Classify the block, then choose a prelude that lets the
            // snippet's own structure decide whether it parses. A
            // wrong-shaped prelude (e.g. appending `[gate]` to a `[gate]`
            // fragment) produces a duplicate-table error that hides
            // genuine doc bugs underneath, so the classification has to
            // be tight.
            let kind = classify(&block.body);
            let toml_str = match kind {
                BlockKind::WholeConfig => {
                    stats.whole_configs += 1;
                    block.body.clone()
                }
                BlockKind::GateFragment => {
                    stats.gate_fragments += 1;
                    format!("{}{}", GATE_FRAGMENT_PRELUDE, block.body)
                }
                BlockKind::StructuredFragment => {
                    stats.structured_fragments += 1;
                    format!("{}{}", FRAGMENT_PRELUDE, block.body)
                }
                BlockKind::BareFieldFragment => {
                    stats.bare_field_fragments += 1;
                    format!("{}{}", BARE_FIELD_PRELUDE, block.body)
                }
                BlockKind::Unknown => {
                    panic!(
                        "{path} block #{idx} (lines {start}-{end}) has a novel \
                         fragment shape the classifier doesn't recognise. \
                         Update classify() to handle it.\n\nsnippet:\n{snippet}",
                        path = doc.path,
                        idx = block.index,
                        start = block.start_line,
                        end = block.end_line,
                        snippet = block.body,
                    );
                }
            };

            if let Err(err) = ConfigV1::parse(&toml_str) {
                failures.push(format!(
                    "\n--- {path} block #{idx} (lines {start}-{end}, {kind:?}) failed to parse ---\n\
                     error: {err}\n\
                     snippet:\n{snippet}\n",
                    path = doc.path,
                    idx = block.index,
                    start = block.start_line,
                    end = block.end_line,
                    kind = kind,
                    err = err,
                    snippet = block.body,
                ));
            }
        }
    }

    // Sanity check: wave-1 reported ~29 snippets; if the count drops
    // by more than a third we've probably broken the fence extractor
    // rather than legitimately deleted half the recipes.
    assert!(
        total_blocks >= 20,
        "extracted only {total_blocks} TOML blocks across {} docs — expected ~29; \
         the fence-matching logic in `extract_toml_blocks` is probably broken",
        AUDIT_DOCS.len(),
    );

    if !failures.is_empty() {
        panic!(
            "{} of {} TOML snippets in docs/audits/ failed to parse against ConfigV1.\n\
             whole configs: {}, structured fragments: {}, gate fragments: {}, bare-field fragments: {}.\n\
             {}",
            failures.len(),
            total_blocks,
            stats.whole_configs,
            stats.structured_fragments,
            stats.gate_fragments,
            stats.bare_field_fragments,
            failures.join("\n"),
        );
    }
}

#[cfg(test)]
mod fence_extractor_tests {
    use super::*;

    /// Locks in the open/close fence detection. Without this, a refactor
    /// that accepts ` ```toml,custom ` info-strings (which TOML doesn't
    /// support) would silently widen the test's scope.
    #[test]
    fn extracts_only_toml_blocks() {
        let md = "\
intro\n\
```toml\n\
version = 1\n\
[gate]\n\
```\n\
between\n\
```bash\n\
echo skip me\n\
```\n\
```yaml\n\
skip: me\n\
```\n\
```toml\n\
[[checks]]\n\
name = \"x\"\n\
[checks.source]\n\
type = \"shell\"\n\
command = \"true\"\n\
```\n\
end\n";

        let blocks = extract_toml_blocks(md);
        assert_eq!(blocks.len(), 2, "expected 2 toml blocks, got {}", blocks.len());
        assert!(blocks[0].body.contains("version = 1"));
        assert!(blocks[1].body.contains("[[checks]]"));
        assert!(!blocks[1].body.contains("echo skip me"));
    }

    #[test]
    fn classify_recognises_each_block_shape() {
        // Whole config: starts with `version = 1`.
        assert_eq!(
            classify("version = 1\n[gate]\nagents = []\n"),
            BlockKind::WholeConfig
        );
        // Bare `[gate]`-only fragment.
        assert_eq!(
            classify("[gate]\nagents = [\"claude\"]\npolicy = \"all_fail\"\n"),
            BlockKind::GateFragment
        );
        // `[[checks]]` fragment — needs the full prelude.
        assert_eq!(
            classify("[[checks]]\nname = \"x\"\n[checks.source]\ntype = \"shell\"\ncommand = \"true\"\n"),
            BlockKind::StructuredFragment
        );
        // `[[trigger]]` fragment.
        assert_eq!(
            classify("[[trigger]]\nname = \"jj\"\npattern = \"^jj\""),
            BlockKind::StructuredFragment
        );
        // Single-line illustration of a check.source field.
        assert_eq!(
            classify("command = \"tsgo --noEmit --incremental\"\n"),
            BlockKind::BareFieldFragment
        );
        // Comments and blanks don't change the verdict.
        assert_eq!(
            classify("# just a hint\n\ncommand = \"foo\"\n"),
            BlockKind::BareFieldFragment
        );
        // Top-level array values are not safe to wrap as bare fields —
        // they would parse spuriously inside `[checks.source]`.
        assert_eq!(
            classify("triggers = [{ on = [\"push\"] }]\n"),
            BlockKind::Unknown
        );
        // Inline tables also count as Unknown.
        assert_eq!(
            classify("source = { type = \"shell\", command = \"x\" }\n"),
            BlockKind::Unknown
        );
        // `versionless = 1` must not be misclassified as a whole config.
        assert_eq!(
            classify("versionless = 1\ncommand = \"foo\"\n"),
            BlockKind::BareFieldFragment
        );
        // `[gate]` with a trailing comment still classifies as a gate
        // fragment.
        assert_eq!(
            classify("[gate]  # see recipes.md\nagents = [\"claude\"]\n"),
            BlockKind::GateFragment
        );
        // `[gate.subtable]` is not a top-level gate fragment.
        assert_eq!(
            classify("[gate.subtable]\nx = 1\n"),
            BlockKind::StructuredFragment
        );
    }
}
