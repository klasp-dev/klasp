# Master plan — klasp (Phase 5, convergent)

Program artifact · Phase 5 · **GATE: awaiting user sign-off before Phase 6 delivery.**

## Thesis

> **klasp is the agent quality-gate whose capability claims are compiler-checked.**
> Its wedge is "one config, many agents," and its proof of trust is the agent-surface conformance
> matrix. Today that matrix is a *human* convention (CI checks a row exists, not that each `✓` is
> backed by a passing test). The signature move of this program turns the matrix into a *generated,
> test-proven* artifact — a `✓` that cannot lie — and lets the marketing site render the same proven
> data so the public promise and the code can never drift. Everything else is correctness hygiene that
> protects that promise (true MSRV, a security test that actually runs, version-sync invariants).

## What already landed (Phases 2–3, this program)

| Change | Commit | Repo |
|---|---|---|
| B1 — pytest exit 5 = no-op pass (test-first) | `b80c842` | klasp |
| B2/A1 — example-plugin tests (incl. security regression) run in CI | `85df24f` | klasp |
| A6 — roadmap status + CHANGELOG truth | `1600092` | klasp |
| A7 — klasp.dev v0.4→v0.5 sync | `c943ae3` | klasp.dev (`claude/site-v0.5-sync`) |

All on branch `claude/unruffled-mestorf-7b93fd` (klasp). Suite: **756 pass / 6 ignored**, clippy clean,
fmt clean, site builds clean.

## Deliberation — devil's-advocate on the record

| Idea | Challenge | Verdict |
|---|---|---|
| **S1 self-proving matrix** | "Generation could be brittle vs. a working human convention." | **IN.** Keep the registry a thin (cell→existing-test-name) map the tests already own; brittleness stays bounded and the payoff *is* the brand. |
| **S3 version invariant** | "Two scripts work today." | **IN.** Cheap (S); a drifted wrapper/platform release ships broken. Pure downside-protection. |
| **S4 site can't-go-stale** | "We just hand-fixed it." | **IN**, but only as a *by-product* of S1 (render the matrix JSON S1 emits) + an Astro CI check. Don't build a bespoke sync. |
| **S5 doctor lint / S8 MSRV CI / A3 / A8** | "Not features." | **IN as one small hygiene WS.** Correctness, not headline — grouped so they don't masquerade as signature. |
| **S2 enforce mode** | "Invites 'klasp broke my commit' support load; not needed for the thesis." | **DEFER.** Real value, but opt-in fail-closed is its own design + test surface; out of scope for this program unless you want it. |
| **S6 finding redaction** | "Over-redaction hurts signal; thesis doesn't need it." | **DEFER.** Track as security follow-up. |
| **S7 new surfaces** | "Cursor was NO-GO; large." | **DEFER.** |

## Decision required from you (blocks WS-2)

**A2 / MSRV.** Declared `rust-version = "1.75"` is unbuildable; verified real floor = **1.85**
(`edition2024` via `toml`/`serde_spanned`). Pick:
- **(a) Adopt 1.85** — set `rust-version = "1.85"` + add a 1.85 CI leg. Documents reality; the
  1.75–1.84 "support" was already fictional. *Recommended* unless low MSRV is a deliberate selling point.
- **(b) Hold 1.75** — pin `toml`/`serde_spanned` down to edition-2021-compatible versions + add a 1.75
  CI leg. Keeps low MSRV; costs a dependency downgrade and ongoing pin maintenance.

## Workstreams (dependency-ordered) — proposed Phase 6 scope

| WS | Scope | Closes | Effort | Depends on |
|---|---|---|---|---|
| **WS-1 (signature)** | Generate `docs/agent-surfaces.md` + a `surfaces.json` from a test-backed registry; CI fails on drift between generated and committed. | A4 / S1 | M | — |
| **WS-2 (hygiene)** | MSRV fix per your A2 choice + CI leg (S8); `klasp doctor` agent-id/trigger footgun lint (S5/B5); make `--ignored` regressions visible (A3); fmt-check example crates + one reformat (A8). | A2/A3/A8/B5 | S–M | WS-0 decision |
| **WS-3** | Atomic version bump command + CI invariant that Cargo/npm/pypi agree. | A9 / S3 | S | — |
| **WS-4 (cross-repo)** | klasp.dev renders `surfaces.json` from WS-1 + a `version` value; add an Astro build check to CI so the site can't break or lag. | A7-rest / S4 | S–M | WS-1 |
| **WS-5 (deferred)** | Enforce mode (S2); finding redaction (S6). | — | M each | — — only if you opt in |

**Recommended this program:** WS-1 → WS-2 → WS-3 → WS-4 (defer WS-5, S7). Each lands on its own
branch off `claude/unruffled-mestorf-7b93fd` with conventional commits, tests-first, full gauntlet
green before "done." WS-4 touches klasp.dev in the same change-set as the WS-1 JSON it consumes.

## Verification per workstream
`cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` + `cargo test --workspace` +
example-crate tests + `node scripts/check-agent-surfaces.mjs` (and, post-WS-1, the new drift check);
klasp.dev `pnpm build`. klasp dogfoods itself, so each commit is gated by `klasp.toml`.

## Rollback / risk
Every WS is independently revertable. WS-1's generator is additive (the committed matrix stays the
source of record; the check only asserts equality). WS-2's MSRV change is a one-line manifest edit +
CI leg. No data migrations, no destructive ops.
