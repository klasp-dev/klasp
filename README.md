# klasp

[![ci](https://github.com/klasp-dev/klasp/actions/workflows/ci.yml/badge.svg)](https://github.com/klasp-dev/klasp/actions/workflows/ci.yml)

[**klasp.dev**](https://klasp.dev) · [crates.io](https://crates.io/crates/klasp) · [npm](https://www.npmjs.com/package/@klasp-dev/klasp) · [PyPI](https://pypi.org/project/klasp/) · [GitHub](https://github.com/klasp-dev/klasp)

> Block AI coding agents (Claude Code today; Codex, Cursor, Aider next) on the same quality gates your humans hit at `git commit`.

**Status: v0.1 implementation complete; awaiting `v0.1.0` tag push to publish to registries.** All v0.1 work (W1-W7) is merged on `main`, the test suite is green (119 tests, CI 7/7), and klasp gates its own commits via [`/klasp.toml`](./klasp.toml) and [`.claude/`](./.claude). Until the maintainer pushes `v0.1.0`, `cargo install klasp` / `npm i -g @klasp-dev/klasp` / `pip install klasp` still resolve to the original `0.0.0` name-reservation publishes. Install from this repo directly (see [Quickstart](#quickstart) below) or wait for the tag push.

## What klasp does

You write one `klasp.toml`. You run `klasp install`. Every AI agent on the repo (Claude Code today, more coming) is now blocked on the same `pre-commit`, `cargo clippy`, `pytest`, or any-shell-command gate your humans see at `git commit`. The agent gets a structured "blocked, here's why" reply at its tool-call surface (Claude Code's `PreToolUse` hook) so it self-corrects rather than retrying with `--no-verify`. That retry path is the failure mode burning every team running agents at scale.

## Quickstart

### Today (pre-tag, install from this repo)

```bash
cargo install --git https://github.com/klasp-dev/klasp klasp
cd your-project
klasp init                                 # writes klasp.toml at repo root
$EDITOR klasp.toml                         # add your checks (see below)
klasp install --agent claude_code          # writes .claude/hooks/klasp-gate.sh + merges .claude/settings.json
klasp doctor                               # verify the install is healthy
```

### Post-tag (once `v0.1.0` ships to registries)

```bash
cargo install klasp                        # Rust
npm i -g @klasp-dev/klasp                  # Node (biome-style platform shim)
pip install klasp                          # Python (maturin wheel)
```

Then the same `klasp init` / edit / `klasp install` / `klasp doctor` flow.

### Uninstall

```bash
klasp uninstall --agent claude_code        # removes the hook + settings entry, preserves siblings
```

## What works today

| Feature | Status |
|---|---|
| Claude Code agent gate (`PreToolUse`) | Shipped in v0.1 |
| Shell-command checks via `klasp.toml` | Shipped in v0.1 |
| `klasp init` / `install` / `uninstall` / `gate` / `doctor` | Shipped in v0.1 |
| `KLASP_BASE_REF` env var for diff-aware checks | Shipped in v0.1 |
| Four-platform binary distribution (cargo / npm / PyPI) | Shipped in v0.1 (live post-tag); darwin-arm64, linux-x64-gnu, linux-arm64-gnu, win-x64. x86 mac → `cargo install klasp` from source. |
| Codex via `AGENTS.md` + git hooks | v0.2 |
| Named recipes (`type = "pre_commit"` / `"fallow"` / `"pytest"` / `"cargo"`) | v0.2 |
| Parallel check execution | v0.2.5 |
| Cursor / Aider surfaces | v0.3 |
| Plugin protocol | v0.3 (experimental) → v1.0 (stable) |

See [`docs/roadmap.md`](./docs/roadmap.md) for the full milestone plan.

## Example `klasp.toml`

This is klasp's own dogfood config (also at [`/klasp.toml`](./klasp.toml)):

```toml
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"

# Fast type/borrow check on every commit attempt.
[[checks]]
name = "cargo-check"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "cargo check --all-targets --workspace"

# Lint with -D warnings on commit and push.
[[checks]]
name = "cargo-clippy"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "cargo clippy --all-targets --workspace -- -D warnings"

# Full workspace tests on push only (slower than clippy).
[[checks]]
name = "cargo-test"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "shell"
command = "cargo test --workspace"
```

Every shell check sees `KLASP_BASE_REF` in its environment, set to the merge-base of `HEAD` against the upstream tracking branch (falling back to `origin/main`, `origin/master`, then `HEAD~1`). Diff-aware tools (`pre-commit run --from-ref`, `fallow audit --base`) can scope themselves to the diff without an agent-side wrapper. See [`docs/recipes.md`](./docs/recipes.md) for worked examples in pre-commit, fallow, pytest, ESLint/Biome, ruff.

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
| `klasp.toml` | klasp's own dogfood config (the canonical v0.1 example) |

## License

Apache-2.0. See [`LICENSE`](./LICENSE).
