# Agent Surface Conformance Matrix

> **One `klasp.toml`. Three surfaces. Identical gate contract.**

This matrix tracks what works, per agent surface, across the install / gate / doctor lifecycle.
A cell is only `✓` when all three of these are true:

1. An integration test exercises the install/uninstall path against a temp repo and asserts on-disk state.
2. A captured-session test replays a failing commit and verifies the structured verdict shape.
3. `klasp doctor` surfaces `MissingHook` / `StaleConfig` / `WrongVersion` findings when the underlying state is wrong.

Anything weaker is `?` — feature claimed but not load-bearing.

This file is a **tracked contract**, not a marketing table. Two guards keep it honest:

- Every agent surface crate (`klasp-agents-*`) must have a row here. CI runs
  [`scripts/check-agent-surfaces.mjs`](../scripts/check-agent-surfaces.mjs) on
  every PR and fails if a surface crate lands without a matrix row.
- Every `✓` is backed by a committed test, linked from the proof table in the
  [What `✓` means](#what--means) section below.

See [issue #68](https://github.com/klasp-dev/klasp/issues/68) for the tracking discussion.

## v0.3.0

| Surface | Install | Uninstall | Doctor | Commit gate | Push gate | Structured verdict | Conflict handling | Captured-session test | Notes |
|---|---|---|---|---|---|---|---|---|---|
| Claude Code | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ (husky / lefthook / pre-commit framework) | ✓ | Conflict handling is advisory: Claude installs via `.claude/settings.json`, not `.git/hooks/`, so it warns (rather than skips) when a co-resident manager is detected. See [#92](https://github.com/klasp-dev/klasp/issues/92). |
| Codex CLI | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Aider | ✓ | ✓ | ✓ | ✓ | — | ✓ | — | ✓ | v0.3 W1 (#40, #46). Aider has no push-time hook (`.aider.conf.yml` exposes `commit-cmd-pre` only) and no conflicting hook-manager surface — both columns are intentional `—`, not regressions. |
| Cursor | — | — | — | — | — | — | — | — | Not supported in v0.3 (see [cursor-assessment.md](./cursor-assessment.md)); hook surface is beta with open correctness bugs |
| Windsurf | — | — | — | — | — | — | — | — | Not surveyed |
| Cline | — | — | — | — | — | — | — | — | Not surveyed |

## What `✓` means

A row claims `✓` only when a committed integration test in the test suite proves it.
Every `✓`-bearing surface has a proof row below; each test is mapped to the columns
it backs so a reviewer can trace any `✓` to the file that proves it:

| Surface | Columns proven | Test file(s) |
|---|---|---|
| Claude Code | Install / Uninstall / Doctor | [`klasp/tests/install_claude_code.rs`](../klasp/tests/install_claude_code.rs) |
| Claude Code | Commit gate / Push gate / Structured verdict / Captured-session | [`klasp/tests/gate_flow.rs`](../klasp/tests/gate_flow.rs) |
| Claude Code | Conflict handling | [`klasp-agents-claude/tests/conflict_detection.rs`](../klasp-agents-claude/tests/conflict_detection.rs) |
| Codex CLI | Install / Uninstall / Doctor / Conflict handling | [`klasp/tests/install_codex_cli.rs`](../klasp/tests/install_codex_cli.rs) |
| Codex CLI | Commit gate / Push gate / Structured verdict / Captured-session | [`klasp/tests/codex_captured_session.rs`](../klasp/tests/codex_captured_session.rs) |
| Aider | Install / Uninstall / Doctor | [`klasp-agents-aider/tests/aider_conf_install.rs`](../klasp-agents-aider/tests/aider_conf_install.rs) |
| Aider | Commit gate / Structured verdict / Captured-session | [`klasp/tests/aider_captured_session.rs`](../klasp/tests/aider_captured_session.rs) |

## Plugin protocol

The `type = "plugin"` check source enables third-party extensibility without a new surface.
See [`docs/plugins.md`](./plugins.md) for the authoring guide and
[`examples/klasp-plugin-pre-commit/`](../examples/klasp-plugin-pre-commit/) for the reference implementation.

End-to-end plugin smoke tests live in [`klasp/tests/plugin_smoke.rs`](../klasp/tests/plugin_smoke.rs).

## Cursor — NO-GO for v0.3

W5 assessment (#44) concluded NO-GO. Cursor's `hooks.json` surface:

- Is explicitly marked **beta** in Cursor's own changelog from 1.7 through 3.3 (no stability promotion in seven months).
- Has an open **silent-allow correctness bug** (`beforeShellExecution` malformed-JSON response silently permits the blocked command instead of blocking it).
- Has an open **Windows path-formatting bug** causing hooks to silently fail on Windows.
- Carries no breaking-change policy or migration commitment.

A gate built on this surface today would silently fail to block commits in exactly the failure mode that matters. The NO-GO criterion is: Cursor's hook surface is still moving, and shipping against an unstable beta surface would be worse than deferral.

**What changes the verdict:** hooks promoted from beta, silent-allow bug fixed, stable `hooks.json` schema version field, Windows path bug resolved. See [`docs/cursor-assessment.md`](./cursor-assessment.md) for the full analysis.
