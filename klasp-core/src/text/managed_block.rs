//! Generic managed-block writer: insert/update/remove a delimited region
//! inside an existing text file, idempotently, while preserving sibling
//! content.
//!
//! Multiple klasp surfaces write a "managed block" — a region bracketed by
//! a [`Markers::start`] / [`Markers::end`] line pair that other tools must
//! leave alone. The AGENTS.md writer ([`crate`]'s `klasp-agents-codex`
//! sibling, markdown), the git-hook writer (shell, with a shebang prelude),
//! and future YAML config writers all share the same insert/update/remove
//! algorithm; only their marker constants and an optional file-format
//! prelude differ. This module is that shared algorithm; callers map
//! [`BlockError`] onto their own crate-local error type and supply the
//! file-format framing via [`Prelude`].
//!
//! ## Contract
//!
//! - **Idempotency.** `install(install(input))` == `install(input)`. The
//!   block contents are anchored by the marker lines; re-running install
//!   replaces only what's between them.
//! - **Preservation.** Bytes outside the managed block are returned
//!   unchanged, with one tolerated normalisation: trailing-newline state is
//!   canonicalised to a single `\n` when install appended (or uninstall
//!   stripped) the block.
//! - **Round-trip.** `uninstall(install(input))` is `input` after
//!   normalising the trailing-newline state to a single `\n` (or the empty
//!   string when `input` was empty or whitespace-only, modulo a prelude
//!   that owned the whole file).

use thiserror::Error;

/// The marker line pair that brackets a managed block.
///
/// `start` and `end` are matched as exact substrings (the writer greps for
/// them verbatim), so callers pass their stable, namespaced marker
/// constants — e.g. `<!-- klasp:managed:start -->` for markdown or
/// `# >>> klasp managed start <<<` for shell.
#[derive(Debug, Clone, Copy)]
pub struct Markers<'a> {
    /// Opening marker line.
    pub start: &'a str,
    /// Closing marker line.
    pub end: &'a str,
}

/// Byte span of the managed block within the host string, including both
/// markers and the trailing `\n` after the end marker (when present). The
/// span is a clean cut: `text[..span.start] + new_block + text[span.end..]`
/// replaces the block while preserving everything around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Byte offset of the first byte of the start marker.
    pub start: usize,
    /// Byte offset one past the block (after the end marker's trailing
    /// newline, if any).
    pub end: usize,
}

/// Errors the managed-block writer can raise.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BlockError {
    /// The host text contains an unmatched marker pair (start without end,
    /// end without start, duplicate markers, or end-before-start). The
    /// writer refuses to coerce the file because the "safe" action —
    /// overwriting from the first marker to EOF — could nuke hand-written
    /// content the user intended to keep. Callers map this onto their own
    /// error variant with a file-format-specific message.
    #[error(
        "managed-block markers are malformed (expected exactly one start followed by one end)"
    )]
    MalformedMarkers,
}

/// Optional file-format prelude prepended ahead of the block when install
/// has to fresh-create a file (or when appending to a file that lacks it).
///
/// Markdown surfaces pass `None`. The git-hook surface passes
/// `Some(Prelude { line: SHEBANG })` so a fresh hook starts with an
/// interpreter line and a hook authored without one gains it.
#[derive(Debug, Clone, Copy)]
pub struct Prelude<'a> {
    /// The prelude line (e.g. a shebang), inserted without a trailing
    /// newline — [`install_block`] adds the separator.
    pub line: &'a str,
}

/// Locate the managed block bracketed by `markers` within `existing`.
///
/// Returns `Ok(None)` when neither marker is present, `Ok(Some(span))` for
/// a single well-formed pair, and `Err(BlockError::MalformedMarkers)` for a
/// lone marker, duplicate markers, or an end-before-start pair.
pub fn find_block(existing: &str, markers: &Markers<'_>) -> Result<Option<Span>, BlockError> {
    let (Some(start), Some(end_marker_start)) =
        (existing.find(markers.start), existing.find(markers.end))
    else {
        // Either marker present without the other → malformed; both absent → no block.
        return if existing.contains(markers.start) || existing.contains(markers.end) {
            Err(BlockError::MalformedMarkers)
        } else {
            Ok(None)
        };
    };

    // Reject duplicates and crossed pairs in one pass: a well-formed block
    // has `find == rfind` for both markers, with the start before the end.
    if existing.rfind(markers.start) != Some(start)
        || existing.rfind(markers.end) != Some(end_marker_start)
        || end_marker_start < start
    {
        return Err(BlockError::MalformedMarkers);
    }

    // Span end = end of the end-marker line, including the trailing newline
    // if there is one. This makes the replace operation a clean cut.
    let after_marker = end_marker_start + markers.end.len();
    let end = if existing.as_bytes().get(after_marker) == Some(&b'\n') {
        after_marker + 1
    } else {
        after_marker
    };
    Ok(Some(Span { start, end }))
}

/// `true` when `existing` already contains a (well-formed) managed block.
pub fn contains_block(existing: &str, markers: &Markers<'_>) -> Result<bool, BlockError> {
    Ok(find_block(existing, markers)?.is_some())
}

/// Render the full managed block (markers + body) for embedding in a host
/// file.
///
/// The output starts with `markers.start` on its own line, ends with
/// `markers.end` on its own line, and the body is sandwiched with single
/// newline separators. The body is normalised to end in a single `\n` so
/// the closing marker sits on its own line regardless of caller hygiene.
pub fn render_block(markers: &Markers<'_>, body: &str) -> String {
    let trimmed = body.trim_end_matches('\n');
    format!("{}\n{}\n{}\n", markers.start, trimmed, markers.end)
}

/// Insert (or update) the managed block in `existing`, returning the new
/// file body.
///
/// Behaviour matrix (with `prelude = None`):
///
/// | Input shape                  | Output shape                                    |
/// |------------------------------|-------------------------------------------------|
/// | empty / all-whitespace       | the rendered block, no leading/trailing padding |
/// | contains a managed block     | block contents replaced in-place                |
/// | non-empty, no managed block  | original bytes + blank line + appended block    |
///
/// With `prelude = Some(p)` the fresh-create and no-shebang-append paths
/// gain a `p.line\n\n` prefix so the file always opens with the prelude:
///
/// | Input shape                       | Output shape                                |
/// |-----------------------------------|---------------------------------------------|
/// | empty / all-whitespace            | `p.line\n\n<block>`                          |
/// | non-empty, starts with `p.line`*  | `<existing>\n\n<block>`                      |
/// | non-empty, missing prelude        | `p.line\n\n<existing>\n\n<block>`            |
/// | contains a managed block          | block contents replaced in-place            |
///
/// *The "already has prelude" test is a generic `starts_with("#!")` shebang
/// check — the git-hook caller's only prelude use today.
///
/// Idempotent: when the existing block already matches the rendered block
/// byte-for-byte and no prelude prepending was needed, the input is
/// returned unchanged.
pub fn install_block(
    existing: &str,
    markers: &Markers<'_>,
    body: &str,
    prelude: Option<Prelude<'_>>,
) -> Result<String, BlockError> {
    let block = render_block(markers, body);

    if let Some(span) = find_block(existing, markers)? {
        // Replace in-place. Preserve everything outside [start, end).
        let mut out = String::with_capacity(existing.len() + block.len());
        out.push_str(&existing[..span.start]);
        out.push_str(&block);
        out.push_str(&existing[span.end..]);
        return Ok(out);
    }

    // No existing block. Decide on the prelude prefix + spacing.
    if existing.trim().is_empty() {
        // Fresh-create / empty file.
        return Ok(match prelude {
            None => block,
            Some(p) => {
                let mut out = String::with_capacity(p.line.len() + block.len() + 2);
                out.push_str(p.line);
                out.push_str("\n\n");
                out.push_str(&block);
                out
            }
        });
    }

    // Existing user content, no block. Append after it with a blank-line
    // separator, prepending the prelude first if one is required and the
    // file doesn't already open with a shebang.
    let needs_prelude = match prelude {
        Some(_) => !has_shebang(existing),
        None => false,
    };
    let prelude_line = prelude.map(|p| p.line).unwrap_or("");
    let mut out = String::with_capacity(existing.len() + prelude_line.len() + block.len() + 4);
    if needs_prelude {
        out.push_str(prelude_line);
        out.push_str("\n\n");
    }
    out.push_str(existing.trim_end_matches('\n'));
    out.push_str("\n\n");
    out.push_str(&block);
    Ok(out)
}

/// Inverse of [`install_block`]: remove the managed block and the
/// blank-line separator install inserted when it appended the block.
///
/// Idempotent: a file with no managed block is returned unchanged. A file
/// that contained *only* the block (or, with a prelude, only the prelude +
/// block) collapses to the empty string. When install *appended* the block
/// to user content, uninstall restores the canonical `<content>\n` shape.
pub fn uninstall_block(
    existing: &str,
    markers: &Markers<'_>,
    prelude: Option<Prelude<'_>>,
) -> Result<String, BlockError> {
    let Some(span) = find_block(existing, markers)? else {
        return Ok(existing.to_string());
    };

    let before = &existing[..span.start];
    let after = &existing[span.end..];

    // Shapes possible after stripping the block:
    //
    // 1. `before` is empty (block at byte 0): collapse to `after`.
    // 2. (prelude only) `before` is just the prelude/shebang + whitespace
    //    and `after` is empty: the round-trip from a fresh-created file
    //    klasp owned outright — collapse to empty so the caller can `rm`.
    // 3. `before` has real content: strip the trailing `\n\n` install
    //    inserted as a separator, restoring canonical `<content>\n`. If
    //    `after` is non-empty, leave it verbatim.
    let mut out = String::with_capacity(before.len() + after.len() + 1);
    if before.is_empty() {
        out.push_str(after);
    } else if after.is_empty() && prelude.is_some() && is_only_shebang_or_whitespace(before) {
        // Prelude-only prefix means klasp was the sole content. Collapse
        // to empty so the caller deletes the file.
    } else if after.is_empty() {
        out.push_str(before.trim_end_matches('\n'));
        out.push('\n');
    } else {
        out.push_str(before);
        out.push_str(after);
    }
    Ok(out)
}

fn has_shebang(s: &str) -> bool {
    s.starts_with("#!")
}

/// Returns `true` when `s` consists of only a shebang line plus whitespace
/// (and nothing else). Used by [`uninstall_block`] to detect the
/// round-trip-from-fresh-create case where klasp owns the entire file.
fn is_only_shebang_or_whitespace(s: &str) -> bool {
    let trimmed = s.trim();
    trimmed.is_empty() || (trimmed.starts_with("#!") && !trimmed.contains('\n'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: Markers<'static> = Markers {
        start: "<!-- start -->",
        end: "<!-- end -->",
    };

    const SH: Markers<'static> = Markers {
        start: "# >>> start <<<",
        end: "# >>> end <<<",
    };

    const SHEBANG: Prelude<'static> = Prelude {
        line: "#!/usr/bin/env sh",
    };

    #[test]
    fn render_block_wraps_body_in_markers() {
        let s = render_block(&MD, "hello");
        assert!(s.starts_with(MD.start));
        assert!(s.contains("hello"));
        assert!(s.trim_end().ends_with(MD.end));
        assert!(s.ends_with('\n'));
    }

    #[test]
    fn render_block_normalises_trailing_newlines_in_body() {
        let s = render_block(&MD, "hello\n\n\n");
        assert_eq!(s, format!("{}\nhello\n{}\n", MD.start, MD.end));
    }

    #[test]
    fn find_block_none_when_absent() {
        assert_eq!(find_block("# Project\nNotes.\n", &MD).unwrap(), None);
    }

    #[test]
    fn find_block_rejects_lone_marker() {
        let pre = format!("{}\nbody\n", MD.start);
        assert_eq!(find_block(&pre, &MD), Err(BlockError::MalformedMarkers));
    }

    #[test]
    fn find_block_rejects_end_before_start() {
        let pre = format!("{}\nbody\n{}\n", MD.end, MD.start);
        assert_eq!(find_block(&pre, &MD), Err(BlockError::MalformedMarkers));
    }

    #[test]
    fn find_block_rejects_duplicates() {
        let pre = format!("{s}\none\n{e}\n{s}\ntwo\n{e}\n", s = MD.start, e = MD.end);
        assert_eq!(find_block(&pre, &MD), Err(BlockError::MalformedMarkers));
    }

    // --- No-prelude (markdown-shaped) path ---

    #[test]
    fn install_no_prelude_into_empty_emits_just_the_block() {
        let out = install_block("", &MD, "body", None).unwrap();
        assert!(out.starts_with(MD.start));
        assert!(out.trim_end().ends_with(MD.end));
    }

    #[test]
    fn install_no_prelude_appends_with_blank_line_separator() {
        let pre = "# Project\n\nNotes.\n";
        let out = install_block(pre, &MD, "body", None).unwrap();
        assert!(out.starts_with(pre));
        let after_pre = &out[pre.len()..];
        assert!(after_pre.starts_with('\n'));
        assert!(after_pre[1..].starts_with(MD.start));
    }

    #[test]
    fn install_no_prelude_replaces_in_place() {
        let stale = render_block(&MD, "OLD");
        let pre = format!("# Top\n\n{stale}\nbottom\n");
        let out = install_block(&pre, &MD, "NEW", None).unwrap();
        assert!(out.contains("NEW"));
        assert!(!out.contains("OLD"));
        assert!(out.starts_with("# Top\n\n"));
        assert!(out.ends_with("bottom\n"));
    }

    #[test]
    fn install_no_prelude_is_idempotent() {
        let pre = "# Project\n\nNotes.\n";
        let once = install_block(pre, &MD, "body", None).unwrap();
        let twice = install_block(&once, &MD, "body", None).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn round_trip_no_prelude_restores_original() {
        let pre = "# Project\n\nNotes.\n";
        let installed = install_block(pre, &MD, "body", None).unwrap();
        let restored = uninstall_block(&installed, &MD, None).unwrap();
        assert_eq!(restored, pre);
    }

    #[test]
    fn round_trip_no_prelude_on_empty_returns_empty() {
        let installed = install_block("", &MD, "body", None).unwrap();
        let restored = uninstall_block(&installed, &MD, None).unwrap();
        assert_eq!(restored, "");
    }

    // --- Prelude (shell-shaped) path ---

    #[test]
    fn install_prelude_into_empty_emits_shebang_and_block() {
        let out = install_block("", &SH, "body", Some(SHEBANG)).unwrap();
        assert!(out.starts_with(SHEBANG.line));
        assert!(out.contains(SH.start));
        assert!(out.trim_end().ends_with(SH.end));
    }

    #[test]
    fn install_prelude_into_hook_with_shebang_appends() {
        let pre = "#!/bin/bash\n\necho 'user lint'\n";
        let out = install_block(pre, &SH, "body", Some(SHEBANG)).unwrap();
        assert!(out.starts_with(pre));
        let after_pre = &out[pre.len()..];
        assert!(after_pre.starts_with('\n'));
        assert!(after_pre[1..].starts_with(SH.start));
    }

    #[test]
    fn install_prelude_into_hook_without_shebang_prepends_one() {
        let pre = "echo lint\n";
        let out = install_block(pre, &SH, "body", Some(SHEBANG)).unwrap();
        assert!(out.starts_with(SHEBANG.line));
        assert!(out.contains("echo lint"));
        assert!(out.contains(SH.start));
    }

    #[test]
    fn install_prelude_is_idempotent() {
        let pre = "#!/bin/bash\n\necho 'user lint'\n";
        let once = install_block(pre, &SH, "body", Some(SHEBANG)).unwrap();
        let twice = install_block(&once, &SH, "body", Some(SHEBANG)).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn round_trip_prelude_on_user_hook_restores_input() {
        let pre = "#!/bin/bash\n\necho 'user lint'\n";
        let installed = install_block(pre, &SH, "body", Some(SHEBANG)).unwrap();
        let restored = uninstall_block(&installed, &SH, Some(SHEBANG)).unwrap();
        assert_eq!(restored, pre);
    }

    #[test]
    fn round_trip_prelude_on_empty_collapses_to_empty() {
        let installed = install_block("", &SH, "body", Some(SHEBANG)).unwrap();
        let restored = uninstall_block(&installed, &SH, Some(SHEBANG)).unwrap();
        assert_eq!(restored, "");
    }

    #[test]
    fn uninstall_is_noop_when_no_block_present() {
        let pre = "#!/bin/sh\necho lint\n";
        assert_eq!(uninstall_block(pre, &SH, Some(SHEBANG)).unwrap(), pre);
    }

    #[test]
    fn contains_block_true_after_install() {
        let installed = install_block("", &MD, "body", None).unwrap();
        assert!(contains_block(&installed, &MD).unwrap());
    }

    #[test]
    fn contains_block_false_for_unrelated_text() {
        assert!(!contains_block("<!-- some other tool -->\n", &MD).unwrap());
    }
}
