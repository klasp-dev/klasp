# Bug register — klasp

Program artifact · Phase 2. Discipline: failing test first → minimal fix → full suite green; no
suppression; suspected-but-unreproduced items go to triage, not speculative fixes.

**Headline (truth over optimism):** klasp is a healthy codebase. The sweep produced **one** clear
behavioral bug (B1) and **one** security-test-coverage gap (B2), both fixed this program. B3–B6 are
P3 static-analysis observations, **not reproduced**, parked in triage.

## Fixed

### B1 — pytest exit 5 ("no tests collected") blocked the commit · P2 · CONFIRMED
- **Location:** `klasp/src/sources/pytest/verdict.rs:63-66` (generic non-zero arm).
- **Root cause:** exit 5 fell through `Some(other) => fail_with_optional_warning`, mapping the benign
  "no tests collected" case to `Verdict::Fail` (exit 2). A diff-scoped commit that staged no Python
  (e.g. a Rust-only change in a polyglot repo) was blocked.
- **Evidence it was a real defect, not intent:** the maintainer's own `klasp.toml` dogfood note
  ("v0.2.x will fix … treat pytest exit 5 … as a no-op pass") and the unit/integration tests that
  *pinned the wrong behavior* (`verdict.rs` `collection_error_exit_5_*`, `pytest_recipe.rs:322`).
- **Fix:** merged exit 5 into the exit-0 pass arm (`Some(0) | Some(5)`); Warn when a version warning
  is present, mirroring exit 0. Removed the now-unreachable exit-5 detail string.
- **Tests:** rewrote the two unit tests (`collection_error_exit_5_is_pass`,
  `…_with_version_warning_is_warn`) and the integration test
  (`pytest_collection_error_exit_5_passes_through`) — captured RED first, then GREEN.
- **Verification:** `cargo fmt`/`clippy -D warnings` clean; **756 passed / 6 ignored**.
- **Commit:** `b80c842` (also updated `docs/recipes.md` exit-code table + `klasp.toml` note).
- **Trade-off recorded:** silent Pass (not Warn) avoids warn-spam on every non-Python commit; a
  genuinely vanished Python suite is better caught by CI's full run than the diff-scoped commit gate.

### B2 — security regression test never ran in CI · P1 (process/security) · CONFIRMED
- **Location:** `examples/klasp-plugin-agentic-flow/tests/integration.rs`
  (`gate_flag_smuggling_base_ref_is_rejected`); `Cargo.toml:13` `exclude = ["examples/*"]`; `ci.yml`.
- **Root cause:** `examples/` is outside the workspace, so `cargo test --workspace` never ran the
  regression test guarding the git-arg flag-smuggling fix (commit 53231c4). A security fix with no CI.
- **Fix:** added a `check`-job step running `cargo clippy -D warnings` + `cargo test` over every
  `examples/*/Cargo.toml` (not continue-on-error). Both example crates pass today (agentic-flow 15,
  pre-commit 8); the smuggling test now gates every PR.
- **Verification:** ran the exact CI loop locally — green.
- **Commit:** `85df24f`.

## Triage — located via static analysis, NOT reproduced (no speculative fixes)

| ID | Sev | Location | Concern | Recommended next step |
|---|---|---|---|---|
| B3 | P3 | `klasp/src/sources/shell.rs:309-313` | `unwrap_or_default()` on stdout/stderr drain → empty string on capture failure. Verdict is exit-code-driven so this is likely *lost finding detail*, not a verdict flip — but recipes that parse stdout (fallow/cargo JSON) could be affected. | Write a repro with a non-UTF-8-emitting check; confirm whether a finding-bearing verdict can be silently lost. Fix only if reproduced. |
| B4 | P3 | `klasp/src/cmd/gate.rs:483` | `let _ = write!(io::stdout(), …)` can silently drop `--format json`/sarif output yet still exit 0. | Decide: propagate the stdout write error to a non-zero/notice for machine-output formats. Cheap if confirmed worth it. |
| B5 | P3/DX | `klasp/src/cmd/gate.rs:122` | `KLASP_AGENT_ID` unset → `agent_id=""` → agent-scoped `[[trigger]]` filters silently no-op. | Surface via `klasp doctor` lint (idea S5) rather than a code change in the hot path. |
| B6 | P3 | `klasp-core/src/config.rs:342` | `.expect("triggers already validated…")` panics if a `ConfigV1` is ever built outside `parse()`. No such path today. | Defensive: lazily validate or return `Result` in `compiled_triggers()`. Leave until a non-`parse()` construction path is introduced. |

None of B3–B6 block; all are low-severity hardening. Carry into Phase 5 backlog, not Phase 2 fixes.
