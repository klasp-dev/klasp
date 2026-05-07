# Cursor Hook-Surface Assessment — Week 5 Go/No-Go (#44)

**Status: NO-GO**

**Date:** 2026-05-07

**Cursor version assessed:** 3.3 (released 2026-05-06); hooks feature lineage traced from 1.7 (2025-09-29) through 2.0 and 3.0.

---

## What klasp would need from Cursor

For a `CursorSurface` implementation to be viable, at least one of the following must be true:

1. **Documented stable config file.** A first-party config path (analogous to `.aider.conf.yml` or `.claude/settings.json`) that Cursor reads to inject a pre-commit or pre-shell-execution gate, with an explicit stability contract (no breaking changes without a migration path).
2. **Documented stable git-hook integration.** Cursor installs or cooperates with git pre-commit/pre-push hooks in a way klasp can manage, analogous to Codex's `AGENTS.md` + git-hook write.
3. **Documented stable MCP/external hook surface.** An MCP or process-based hook that klasp registers against so `CursorSurface::install` writes one config entry and Cursor reliably calls it before every shell execution.
4. **Stability guarantee.** Whichever surface meets criteria 1–3, it must be explicitly marked stable (not beta, not experimental), with a documented commitment that it will not break in a Cursor patch or minor release.

---

## What Cursor offers today

### Criterion 1 — Documented stable config file

**Evidence:**

Cursor reads hooks from `.cursor/hooks.json` at the project level and `~/.cursor/hooks.json` globally. This is documented at https://cursor.com/docs/hooks (accessed 2026-05-07) and confirmed in multiple third-party sources (GitButler deep-dive, Skywork guide).

**Verdict on criterion 1: FAILS.** The config file path exists and is documented. The hooks system is marked `"(beta)"` in Cursor's own 1.7 changelog ("It's still in beta and we'd love to hear your feedback"). No subsequent changelog entry — through version 3.3, the current release as of 2026-05-07 — has promoted hooks to stable or GA. The schema of `hooks.json`, the names of lifecycle events, and the JSON input/output contract are all subject to change without a documented backward-compatibility commitment. The 3.0 changelog (2026-04-02) contains a hooks bug-fix entry with no stability language. The 2.0 changelog adds enterprise hook distribution with no stability language. Seven months of releases without a stability promotion is itself disqualifying.

### Criterion 2 — Documented stable git-hook integration

**Evidence:**

Cursor does not install or manage git hooks (`.git/hooks/pre-commit`, `.git/hooks/pre-push`). Cursor's hook system is agent-lifecycle hooks, not git-lifecycle hooks. This distinction is explicit in all documentation: Cursor hooks fire at points in the AI agent's execution loop (`beforeShellExecution`, `afterFileEdit`, `preToolUse`, etc.), not at `git commit` time in the traditional sense. Standard git hooks remain available to the developer independently — but Cursor does not document a mechanism to manage or cooperate with them programmatically via a config file.

Community forum threads (e.g., the Documenso issue "feat: Add block-no-verify to prevent Cursor from bypassing git hooks", 2026) discuss Cursor agents bypassing `--no-verify`; this confirms that Cursor does not natively enforce git hook execution and there is no stable API for klasp to target for git-specific gating.

**Verdict on criterion 2: FAILS.** Cursor has no documented stable git-hook integration that klasp could manage.

### Criterion 3 — Documented stable MCP/external hook surface

**Evidence:**

The `beforeShellExecution` and `preToolUse` hooks are the closest analogue to Claude Code's `PreToolUse` hook. A hook command registered in `.cursor/hooks.json` under `beforeShellExecution` receives the shell command being executed and can return `"permission": "deny"` to block it — the mechanism klasp would target.

Technically, this surface works: `CursorSurface::install` would write a `.cursor/hooks.json` entry for `beforeShellExecution` that runs `klasp gate`. However, the surface has active unresolved defects with security implications:

- **CVE-2026-26268** (February 2026): High-severity arbitrary code execution via git hook interaction, arising from Cursor's autonomous git operation model.
- **`beforeShellExecution` silent-allow bug** (Cursor 2.5.17, open): Malformed JSON from a hook silently permits the blocked command instead of blocking it — the opposite of correct behaviour for a security-adjacent gate. The Cursor team acknowledged the bug but has not shipped a fix as of 2026-05-07.
- **`preToolUse` path-formatting bug on Windows** (Cursor 2.6.18, open): `workspace_roots` are provided in Unix-style paths on Windows, causing hooks to silently fail on any path operation. No fix shipped.

A gate built on a surface with these open defects — particularly the silent-allow failure mode for `beforeShellExecution` — does not meet klasp's correctness bar. A gate that can silently fail open due to hook output encoding is indistinguishable from no gate.

**Verdict on criterion 3: FAILS.** The hook surface exists but is not stable: it is explicitly beta, has active unresolved correctness bugs, and no breaking-change policy.

### Criterion 4 — Stability guarantee

**Evidence:**

The Cursor 1.7 changelog (2025-09-29) explicitly labels hooks `"(beta)"` and requests feedback. No Cursor release note from 1.7 through 3.3 (2026-05-06) promotes hooks to stable, generally available, or releases a stability commitment. The GitButler deep-dive article (authored in late 2025) states: "Technically, the Cursor hooks system is currently in beta, so the APIs may change." The Skywork guide cautions: "Because Hooks are beta, re-check the changelog when upgrading." No counter-evidence was found.

**Verdict on criterion 4: FAILS.** No stability guarantee exists for any part of Cursor's hook surface.

---

## Verdict reasoning

All four criteria fail. The closest candidate — `beforeShellExecution` in `.cursor/hooks.json` — is technically capable of blocking shell commands, but is:

1. Explicitly beta in Cursor's own changelog for seven months and counting (no promotion to stable in 3.3, the current release).
2. Affected by an open silent-allow correctness bug that makes the gate unreliable in exactly the failure mode that matters: a hook intending to block a command fails silently and the command runs.
3. Unsupported by any breaking-change policy or migration commitment.

A `CursorSurface` built on this surface today would: (a) break on any Cursor version that changes the `hooks.json` schema; (b) silently fail to block commits when hook output is malformed (due to the open bug); (c) silently fail on Windows due to path-formatting bugs. That is a worse outcome than no support — it would give users a false sense of protection.

The NO-GO criterion in the risk register is exactly this case: Cursor's hook surface is still moving, the APIs may change, and shipping against an unstable beta surface would be worse than deferral.

**Decision: NO-GO. Ship v0.3 without `CursorSurface`. Defer to v0.3.x or v1.0.**

---

## What changes the verdict

A future reassessment should trigger a GO vote if **all** of the following appear:

1. **Hooks promoted from beta.** A Cursor changelog entry explicitly marks hooks (or at minimum `beforeShellExecution` + `preToolUse`) as stable / generally available, with a documented backward-compatibility commitment.
2. **Silent-allow bug fixed.** The `beforeShellExecution` malformed-JSON silent-allow bug (filed against Cursor 2.5.17) is closed with a confirmed fix in a released version. A gate that silently fails open is not acceptable regardless of stability label.
3. **Stable `hooks.json` schema version field.** A `version` field in `hooks.json` (analogous to klasp's `KLASP_GATE_SCHEMA`) so a written hook entry can declare which schema it targets and Cursor can detect mismatch — enabling klasp's idempotency and schema-mismatch-with-notice behaviour.
4. **Windows path bug resolved.** The `workspace_roots` Unix-path-on-Windows bug closed, so klasp installs correctly on all four supported platforms.

Items 1 and 2 are hard blockers. Items 3 and 4 are high-confidence requirements; their absence would require careful mitigation in the implementation.

---

## References

- Cursor Docs — Hooks: https://cursor.com/docs/hooks (accessed 2026-05-07)
- Cursor 1.7 Changelog (hooks introduced, beta label): https://cursor.com/changelog/1-7 (2025-09-29)
- Cursor 2.0 Changelog (enterprise hook distribution): https://cursor.com/changelog/2-0
- Cursor 3.0 Changelog (hooks bug fix, no stability promotion): https://cursor.com/changelog/3-0 (2026-04-02)
- Cursor 3.3 Changelog (current release, no hooks stability update): https://cursor.com/changelog (2026-05-06)
- Cursor Blog — Hooks for security and platform teams: https://cursor.com/blog/hooks-partners (2025-12-22)
- GitButler — Deep Dive into the new Cursor Hooks: https://blog.gitbutler.com/cursor-hooks-deep-dive
- GitButler — Using Cursor Hooks for automatic version control: https://blog.gitbutler.com/cursor-hooks-integration
- InfoQ — Cursor 1.7 Adds Hooks for Agent Lifecycle Control: https://www.infoq.com/news/2025/10/cursor-hooks/ (2025-10-XX)
- Skywork — How to Use Cursor 1.7 Hooks: https://skywork.ai/blog/how-to-cursor-1-7-hooks-guide/
- Forum bug — beforeShellExecution silent-allow (Cursor 2.5.17, open): https://forum.cursor.com/t/beforeshellexecution-hook-malformed-json-response-silently-allows-command-instead-of-blocking/152669
- Forum bug — preToolUse Windows path formatting (Cursor 2.6.18, open): https://forum.cursor.com/t/hooks-intermittently-non-functional-on-windows-pretooluse-worked-then-stopped-after-hooks-json-edit/154608
- CVE-2026-26268 (git hook / agent interaction): https://novee.security/blog/cursor-ide-cve-2026-26268-git-hook-arbitrary-code-execution/
- Documenso issue — block-no-verify for Cursor: https://github.com/documenso/documenso/issues/2372
