# klasp

[**klasp.dev**](https://klasp.dev) · [crates.io](https://crates.io/crates/klasp) · [npm](https://www.npmjs.com/package/@klasp-dev/klasp) · [PyPI](https://pypi.org/project/klasp/) · [GitHub](https://github.com/klasp-dev/klasp)

> Block AI coding agents (Claude Code, Cursor, Codex, Aider) on the same quality gates your humans hit at `git commit`.

**Status: name-reservation placeholder.** The `0.0.0` publish on each registry exists only to claim the name. The v0.1 implementation is in design. Star to follow.

## What klasp will be

One `klasp.toml`, one `klasp install`, and every agent-initiated `git commit` / `git push` runs through `pre-commit`, `fallow`, your test suite, or any shell command — blocking on failure exactly like a human's git hook would.

```toml
# klasp.toml
version = 1

[gate]
agents = ["claude_code"]   # v0.1; Codex in v0.2; Cursor + Aider in v0.3
policy = "any_fail"

[[checks]]
name = "pre-commit"
triggers = [{ on = ["commit"] }]
[checks.source]
type = "shell"
command = "pre-commit run --hook-stage pre-commit --from-ref ${KLASP_BASE_REF} --to-ref HEAD"

[[checks]]
name = "fallow"
triggers = [{ on = ["commit", "push"] }]
[checks.source]
type = "shell"
command = "fallow audit --base ${KLASP_BASE_REF} --quiet"
```

## Documentation

- [`docs/design.md`](./docs/design.md) — v0.1 architecture, abstractions, and rationale
- [`docs/roadmap.md`](./docs/roadmap.md) — milestones from v0.1 → v1.0

## Repository layout

| Path | Purpose |
|---|---|
| `klasp-core/` *(planned)* | Library crate — public traits, types, gate protocol |
| `klasp-agents-claude/` *(planned)* | `AgentSurface` impl for Claude Code |
| `klasp/` *(planned)* | Binary crate — the CLI |
| `npm/` | Biome-style npm distribution shim |
| `pypi/` | maturin-based PyPI distribution wrapper |
| `docs/` | Architecture docs and roadmap |

The `klasp-core` / `klasp-agents-claude` / `klasp` workspace split lands during the v0.1 implementation work; the placeholder `Cargo.toml` at the repo root is the single-crate `0.0.0` publish that reserves the crates.io name.

## License

Apache-2.0
