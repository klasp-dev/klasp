# Cross-repo / cross-package contract inventory

Program artifact · Phase 1.

## Contracts

| Contract | Between | Versioned | Contract test | Notes |
|---|---|---|---|---|
| Gate wire protocol | hook shim ↔ `klasp gate` | `GATE_SCHEMA_VERSION = 2` (typed const) | `protocol_contract.rs`, `installed_hook_runs.rs` | Independent of semver; mismatch fails open with notice |
| Plugin protocol | `klasp gate` ↔ plugin subprocess | `PLUGIN_PROTOCOL_VERSION = 0` (experimental) | `plugin_protocol.rs`, `plugin_smoke*.rs` | May break before v1.0 |
| `klasp.toml` schema | user repo ↔ klasp | `version = 1` + `CONFIG_VERSION` const | `klasp-core/src/config.rs` tests (extensive) | `deny_unknown_fields` everywhere |
| Output JSON schema | `klasp gate --format json` ↔ consumers | `KLASP_OUTPUT_SCHEMA = 1` | `output_json.rs` golden files | Stable within v0.x minor |
| `AgentSurface` trait | klasp-core ↔ adapters | doc-only (additive convention) | conformance-matrix integration tests | Not machine-versioned |
| Conformance matrix | `docs/agent-surfaces.md` ↔ surface crates | n/a | `scripts/check-agent-surfaces.mjs` (CI) | **Guard checks row presence only, NOT ✓→test linkage** → idea S1 |
| npm wrapper ↔ binary | `@klasp-dev/klasp` ↔ platform pkgs | identical semver (locked by bump script) | none in-repo | release-time only |
| pypi shim ↔ binary | maturin `bindings="bin"` | semver + PEP 440 transform | none in-repo | release-time only |
| Version sync | Cargo ↔ npm ↔ pypi | two separate bump scripts | none | **not an enforced invariant** → idea S3 |
| Site ↔ feature set | klasp.dev ↔ klasp | manual | none | drifted a full minor → idea S4 |

## Weak seams (drive Phase 4–5 ideas)

1. **Conformance matrix is human-enforced** — the CI guard proves a row *exists*, not that each `✓`
   maps to a passing test. A `✓` can lie. → **S1 (signature): self-proving matrix.**
2. **Version sync is convention, not invariant** — three sources, two scripts, no assertion they
   agree. → **S3: atomic bump + CI invariant.**
3. **Site drift** — no mechanism ties site copy to real version/matrix. → **S4: generated sync.**
4. **MSRV is unverified** — declared `1.75`, real floor `1.85`; no CI leg caught it. → **A2 decision.**
