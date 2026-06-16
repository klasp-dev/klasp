# Ideation (divergent) — klasp

Program artifact · Phase 4. Unfiltered; tagged by persona / value / effort (S/M/L/XL) / risk / repos.
Convergence + devil's-advocate happens in `05-plan/`.

## Signature (one-of-a-kind)

- **S1 — Self-proving conformance matrix.** *(Architect/Strategist)* Generate `docs/agent-surfaces.md`
  from a test-backed registry: each surface×capability cell is emitted `✓` only when its named proving
  test exists and passes; CI fails if the committed matrix differs from the generated one. A `✓`
  becomes impossible to fake. **Value: high (this is the brand: "capability claims that can't lie").
  Effort: M. Risk: med (must map cells→tests without becoming brittle). Repos: klasp.** Closes A4.

## Hardening / core

- **S2 — Enforce mode (opt-in fail-closed).** `[gate].mode = "enforce"` blocks on internal errors
  instead of fail-open, for teams wanting hard enforcement. Already tracked in SECURITY.md.
  *Value: med-high. Effort: M. Risk: med (must stay opt-in, well-tested, never the default). klasp.*
- **S6 — Finding redaction.** Redact absolute paths + obvious secrets from findings before they enter
  the agent's context (deferred follow-up from commit 53231c4). *Value: med. Effort: M. Risk: med
  (over-redaction hurts signal). klasp.*

## DX / supply-chain integrity

- **S3 — Atomic version bump + invariant.** One `klasp release`/script that bumps Cargo+npm+pypi
  together, plus a CI assertion that all version strings agree. *Value: med. Effort: S. Risk: low.
  klasp.* Closes A9.
- **S5 — `klasp doctor` footgun lints.** Warn when an agent-scoped `[[trigger]]` is used on a surface
  that won't set `KLASP_AGENT_ID` (covers B5); optionally flag other silent no-ops. *Value: med.
  Effort: S. Risk: low. klasp.*
- **S8 — MSRV correctness + CI leg.** Resolve A2: pick the true MSRV (verified 1.85) or pin to hold
  1.75, then add a CI job that builds on the declared MSRV so it can never drift again. *Value: med.
  Effort: S. Risk: low once the target is chosen. klasp.*
- **A3/A8 hygiene** — make `--ignored` regressions visible; fmt-check example crates. *Effort: S each.*

## Site / growth

- **S4 — klasp.dev can't-go-stale sync.** Emit a `version + capability matrix` JSON from the klasp
  repo (a by-product of S1); the Astro site renders it, so the site cannot lag a release. Add a CI
  Astro build check. *Value: med. Effort: S-M. Risk: low. Both repos.* Closes A7 fully.

## Deferred / lower-confidence

- **S7 — New agent surfaces** (Cursor/Windsurf/Cline). Cursor was previously NO-GO. *Effort: L.
  Risk: higher. klasp.* Likely not this program.

## Devil's-advocate seeds (for Phase 5)

- S1: does generating the matrix add brittleness that costs more than the human convention? Mitigant:
  keep the registry a thin list of (cell → test name) the tests already own.
- S2: enforce mode invites "klasp broke my commit" support load. Mitigant: opt-in + loud docs.
- S6: redaction false-positives could hide real findings. Mitigant: conservative patterns, opt-out.
- Everything must serve the thesis, not just be clean. S5/S8/A3/A8 are cheap correctness, not features —
  group them so they don't masquerade as the headline.
