# Final report — klasp code-review & enhancement program

Program artifact · Phase 7 · scope: **klasp + klasp.dev**, full program (comprehend → deliver).

## Thesis delivered

> **klasp is the agent quality-gate whose capability claims are compiler-checked.**

The signature workstream (WS-1) turned the conformance matrix from a human convention into a
generated, test-proven artifact: a `✓` can no longer be committed without a real, runnable, passing
test. The site renders the same source of truth, so the public promise and the code can't drift.

## Before → after

| Dimension | Before | After |
|---|---|---|
| pytest recipe | exit 5 blocked valid commits | no-op pass (B1) |
| git-arg security regression test | never ran in CI | runs every PR (B2) |
| Declared MSRV | `1.75` — **unbuildable** | `1.85` — verified + CI-gated (A2) |
| Conformance matrix | hand-maintained; CI checked only row presence | generated from `surfaces.json`; every `✓` proven by a runnable test (WS-1/A4) |
| Version sync (Cargo/npm/pypi) | two scripts, no invariant | atomic bump + CI invariant (WS-3/A9) |
| Gate on internal error | fail-open only | opt-in `KLASP_MODE=enforce` fail-closed (WS-5a) |
| Findings | could leak `/Users/<name>/…` | home path collapsed to `~` (WS-5b) |
| klasp.dev | hard-coded, drifted a full minor | version + matrix data-driven from vendored `surfaces.json` (WS-4) |
| Example crates | not fmt-checked, drifted | rustfmt-clean + gated (A8) |
| Stale docs | roadmap "v0.2 not started" | corrected (A6) |

## Bugs (Phase 2)

- **B1** P2 — pytest exit-5 → no-op pass. Fixed test-first (`b80c842`).
- **B2** P1 (process/security) — example security test now in CI (`85df24f`).
- **B3, B4, B6** P3 — static-analysis observations, **not reproduced**; in triage (no speculative fixes).
- **B5** upgraded to **confirmed**: no bundled hook exports `KLASP_AGENT_ID`, so agent-scoped
  `[[trigger]]` filters always no-op. The fix is a design decision tied to issue #91 (`--agent` is
  deliberately ignored) — left for the maintainer rather than fought with a band-aid lint.

Honest headline: the codebase was healthy. One real behavioural bug, one CI/security-coverage gap.

## Audit (Phase 3) dispositions

Remediated: A1 (examples in CI), A2 (MSRV), A6 (docs), A7 (site), A8 (example fmt), A9 (version
invariant), A4→WS-1. Deferred/accepted: A3 (`--ignored` continue-on-error split — awkward, deferred),
A5 (coverage gate / Windows test / darwin-x64 — accepted with rationale).

## Delivery (Phase 6)

10 commits on klasp (`claude/unruffled-mestorf-7b93fd` → PR
[#141](https://github.com/klasp-dev/klasp/pull/141); `claude/program-phase6` → PR
[#142](https://github.com/klasp-dev/klasp/pull/142)). 2 commits on klasp.dev (PR
[#5](https://github.com/klasp-dev/klasp.dev/pull/5), PR
[#6](https://github.com/klasp-dev/klasp.dev/pull/6)).

## Verification (Phase 7 — all green)

`cargo fmt --check` + `cargo clippy --all-targets --workspace -- -D warnings` clean;
**`cargo test --workspace` = 760 passed / 6 ignored**; `cargo +1.85 check --workspace` clean;
example crates fmt+clippy+test green (15 + 8); node guards (matrix-drift, proof, version-sync,
surface-rows) all pass; klasp.dev `pnpm build` clean. Every behaviour change was test-first.

## Residual backlog (with rationale)

| Item | Why deferred | Recommended next |
|---|---|---|
| B5 — `KLASP_AGENT_ID` not exported by hooks | Fix is a #91 design decision (env-export vs wire `--agent`) | Maintainer picks the mechanism; then thread it through all 3 surfaces |
| Enforce mode `[gate].mode` + hook-baking | Env-signalled MVP shipped; install-time propagation is its own change | Add `[gate].mode`, have `klasp install` bake `KLASP_MODE` into the hook |
| Secret-pattern redaction (S6 full) | False positives would hide real findings | Design an allow-listed, opt-out redaction set |
| B3/B4/B6 triage | P3, unreproduced | Reproduce before fixing |
| A3 — `--ignored` regression visibility | Splitting known-non-goals from real regressions is fiddly | Tag the 4 documented `#[ignore]`s and fail CI on any *other* ignored failure |
| klasp.dev CI build check | `.github/` is untracked maintainer WIP | Commit a `pnpm build` workflow alongside the existing supply-chain ones |
| Untracked `klasp.dev/klasp.toml` (stale comment) | Tracking is the maintainer's call | Track + fix, or remove |

## Retrospective

- **Worked:** scoping the program to one coherent product at the start (the workspace held ~27
  unrelated repos — a portfolio, not a product); test-first discipline catching the real bugs;
  verifying findings before claiming them (the MSRV "fix" became a "decision" once 1.75 proved
  unbuildable; B5 grew from footgun to confirmed gap once the hooks were checked).
- **Friction:** several klasp.dev files (`.github/`, `klasp.toml`, `pnpm-workspace.yaml`) are
  untracked WIP, which constrained what could land cleanly there.
- **Next program:** resolve the B5/#91 and enforce-mode-hook-baking design decisions, then ship
  them; add the klasp.dev CI build check; consider a coverage gate now that the shell-execution
  surface has grown.
