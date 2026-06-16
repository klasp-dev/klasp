# Best-practice audit — klasp + klasp.dev

Program artifact · Phase 3. Disposition: **Remediate-Now** (done this program) / **Remediate-Planned**
(Phase 5/6 workstream) / **Accept** (documented trade-off).

| ID | Area | Disposition | Status / notes |
|---|---|---|---|
| A1 | `examples/` excluded from `cargo test --workspace` (no CI on reference plugins) | Remediate-Now | **DONE** — CI step runs clippy+test over example crates (commit `85df24f`, with B2). |
| A2 | **MSRV is wrong.** Declared `rust-version = "1.75"` but `cargo +1.75 check` fails: `serde_spanned 1.1.1` (via `toml = "1.1"`) needs `edition2024` (Cargo ≥1.85). Verified real floor = **1.85** (`cargo +1.85 check --workspace` clean). | **Decision @ Phase 5** | Two options, both verified-feasible: **(a)** set `rust-version = "1.85"` + add a 1.85 CI leg (documents reality; the 1.75–1.84 "support" was already fictional); **(b)** pin `toml`/`serde_spanned` down to hold 1.75 (keeps low MSRV as a feature; dep-downgrade cost). Changing advertised MSRV is a crates.io contract decision — not made unilaterally. |
| A3 | `cargo test --ignored` is `continue-on-error` (`ci.yml:91`) — a real regression in a non-goal test would pass silently | Remediate-Planned | Split documented-non-goal `#[ignore]`s from would-be-real regressions so the latter can fail CI. Folds into S-work. |
| A4 | Conformance-matrix CI guard (`scripts/check-agent-surfaces.mjs`) checks **row presence only**, not ✓→test linkage | Remediate-Planned → **S1** | The contract is human-enforced; this is the seam the signature idea closes. |
| A5 | No coverage gate; no Windows `cargo test` (build only); no darwin-x64 | Accept (documented) | darwin-x64 already won't-fix (#9). Coverage gate optional; Windows-test is a real-but-bounded gap given the `sh -c` shell surface — note as low-priority Planned, accept for now. |
| A6 | Stale docs: `roadmap.md:3` ("v0.2 active … not started"), CHANGELOG missing this program's entries | Remediate-Now | **DONE** — commit `1600092`. (design.md §14 left as-is: lower-value prose, no false claim.) |
| A7 | klasp.dev one minor behind; site `klasp.toml` comment stale; no CI build check | Remediate-Now (partial) + **S4** | **DONE (version sync)** commit `c943ae3`. Site `klasp.toml` is untracked → left to maintainer. CI build check + auto-sync = S4. |
| A8 | Example crates are **not rustfmt-compliant** (`fmt --all` skips excluded crates; `fmt --check` shows large drift) | Remediate-Planned | Either add `cargo fmt --check` per example to CI *and* reformat once, or accept drift. Mechanical; bundle with any example-crate touch to avoid standalone churn. |
| A9 | Version sync across Cargo/npm/pypi is two scripts + convention, no asserted invariant | Remediate-Planned → **S3** | A drifted release (wrapper vs platform pkg) would ship broken. |

## Summary

The audit confirms a mature project. The only **must-decide** item is **A2 (MSRV)** — a genuine
contract fork raised at the Phase 5 gate. A1/A6/A7 are done. A3/A4/A8/A9 are clean Planned items that
fold naturally into the signature workstreams (S1, S3, S4) or a small hygiene workstream.
