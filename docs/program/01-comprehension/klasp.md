# Comprehension — klasp (core product)

Program artifact · Phase 1 · scope confirmed = klasp + klasp.dev.

## Intent

klasp blocks AI coding agents (Claude Code, Codex, Aider) on the same quality gates
humans hit at `git commit`/`git push`. One `klasp.toml` declares checks once; klasp wires
each agent's tool-call surface so a failing check returns a structured "blocked, here's why"
the agent self-corrects against — instead of escaping via `--no-verify`. Wedge: **one config,
many agents**, with the agent-surface conformance matrix as a public, test-backed contract.

Intent is **coherent and unambiguous** across code, README, SECURITY.md, design.md, and commit
history. No product-intent fork surfaced.

## Capability map (maturity)

| Capability | Maturity | Notes |
|---|---|---|
| Gate engine (trigger→checks→verdict→exit) | solid | `klasp/src/cmd/gate.rs`; fail-open by design |
| Recipes: shell, pre_commit, fallow, pytest, cargo | solid | `klasp/src/sources/`; pytest exit-5 fixed this program |
| Config model `klasp.toml` v1 + monorepo discovery | solid | `klasp-core/src/config.rs`; nearest-ancestor wins |
| Verdict policies any/all/majority + cross-group AnyFail | solid | `klasp-core/src/verdict.rs` |
| Claude Code surface (PreToolUse[Bash]) | solid | `klasp-agents-claude`; surgical settings.json merge |
| Codex surface (AGENTS.md + git hooks) | solid | `klasp-agents-codex`; conflict-skip |
| Aider surface (`.aider.conf.yml commit-cmd-pre`) | solid (limited) | no push gate, no conflict handling (intentional `—`) |
| Plugin protocol v0 (subprocess) | fragile (experimental) | `PLUGIN_PROTOCOL_VERSION = 0`; may break before v1 |
| Output formats: terminal/JSON(v1)/JUnit/SARIF | solid | `klasp/src/output/` |
| Distribution: npm platform-split + pypi (maturin) | solid | two non-atomic version-bump scripts |
| `klasp init --adopt` / `setup` / `doctor` / `demo` | solid | v0.4–v0.5 |
| Conformance matrix CI guard | partial | row-presence only, not ✓→test linkage (see audit A4 / idea S1) |

## Architecture snapshot

Cargo workspace, 5 members + 2 excluded example plugins:

```
klasp (bin)            CLI, gate runtime, sources/, output/, adopt/
  ├─ klasp-core        traits/types/protocol: config, verdict, trigger, surface, plugin, protocol
  ├─ klasp-agents-claude ─┐
  ├─ klasp-agents-codex   ├─→ klasp-core   (AgentSurface trait impls)
  └─ klasp-agents-aider  ─┘
examples/klasp-plugin-{pre-commit,agentic-flow}  (excluded; speak plugin JSON protocol, no core dep)
```

**Gate execution path** (`klasp/src/cmd/gate.rs:59`): schema handshake (`KLASP_GATE_SCHEMA=2`) →
parse stdin PreToolUse JSON → classify trigger (`klasp-core/src/trigger.rs:39` regex, then user
`[[trigger]]`) → repo root → `KLASP_BASE_REF` (`klasp/src/git.rs:75` merge-base) → staged files →
monorepo group-by-nearest-config → per-group checks via `SourceRegistry` → `Verdict::merge(policy)`
per group, cross-group always `AnyFail` → output dispatch → exit 0 (Pass/Warn) or 2 (Fail).

**Build/test/lint:** `cargo check --all-targets`, `cargo fmt --all -- --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`. Examples are outside the
workspace — run `cargo test --manifest-path examples/*/Cargo.toml` (now wired into CI this program).
MSRV declared `1.75` but **actual floor is 1.85** (see audit; edition2024 via `toml`/`serde_spanned`).

**Tests:** ~383 in-tree (756 assertions pass at HEAD of this branch incl. doctests); unit `#[cfg(test)]`
+ integration `klasp/tests/*` + per-adapter `tests/`. Snapshot tests via `insta` (CI drift guard).

## Open/deferred (maintainer-flagged, pre-program)

- `verdict_path` deferred (design.md §14); enforce/fail-closed mode tracked in SECURITY.md.
- Cursor surface = NO-GO (CHANGELOG); klasp.dev conformance-matrix mirror deferred.
- 4 `#[ignore]` trigger-classifier known-limitation tests (`git -c …`, `bash -c`, `eval`, env-prefix).
