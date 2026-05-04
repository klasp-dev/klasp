# klasp

[![ci](https://github.com/klasp-dev/klasp/actions/workflows/ci.yml/badge.svg)](https://github.com/klasp-dev/klasp/actions/workflows/ci.yml)

[**klasp.dev**](https://klasp.dev) · [crates.io](https://crates.io/crates/klasp) · [npm](https://www.npmjs.com/package/@klasp-dev/klasp) · [PyPI](https://pypi.org/project/klasp/) · [GitHub](https://github.com/klasp-dev/klasp)

> Block AI coding agents (Claude Code, Cursor, Codex, Aider) on the same quality gates your humans hit at `git commit`.

**Status: v0.1 ships when the `v0.1.0` tag is pushed.** Implementation is complete and dogfooded on this repo (see [`/klasp.toml`](./klasp.toml)). Until each registry has been re-published past the original `0.0.0` placeholder, `cargo install klasp` / `npm i -g @klasp-dev/klasp` / `pip install klasp` may still resolve to the name-reservation publish — verify with `klasp --version` after install.

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
- [`docs/recipes.md`](./docs/recipes.md) — worked `klasp.toml` examples for pre-commit, fallow, pytest, cargo, ESLint/Biome, ruff
- [`docs/roadmap.md`](./docs/roadmap.md) — milestones from v0.1 → v1.0
- [`CHANGELOG.md`](./CHANGELOG.md) — release notes

## Repository layout

| Path | Purpose |
|---|---|
| `klasp-core/` | Library crate — public traits, types, gate protocol |
| `klasp-agents-claude/` | `AgentSurface` impl for Claude Code |
| `klasp/` | Binary crate — the CLI |
| `npm/` | Biome-style npm distribution shim |
| `pypi/` | maturin-based PyPI distribution wrapper |
| `docs/` | Architecture docs, recipes, roadmap |

## License

Apache-2.0
