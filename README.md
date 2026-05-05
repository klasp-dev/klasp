# klasp

[![ci](https://github.com/klasp-dev/klasp/actions/workflows/ci.yml/badge.svg)](https://github.com/klasp-dev/klasp/actions/workflows/ci.yml)

[**klasp.dev**](https://klasp.dev) · [crates.io](https://crates.io/crates/klasp) · [npm](https://www.npmjs.com/package/@klasp-dev/klasp) · [PyPI](https://pypi.org/project/klasp/) · [GitHub](https://github.com/klasp-dev/klasp)

> Block AI coding agents (Claude Code today; Codex, Cursor, Aider next) on the same quality gates your humans hit at `git commit`.

## What klasp does

You write one `klasp.toml`. You run `klasp install`. Every AI agent on the repo (Claude Code today, more coming) is now blocked on the same `pre-commit`, `cargo clippy`, `pytest`, or any-shell-command gate your humans see at `git commit`. The agent gets a structured "blocked, here's why" reply at its tool-call surface (Claude Code's `PreToolUse` hook) so it self-corrects rather than retrying with `--no-verify`. That retry path is the failure mode burning every team running agents at scale.

## Why klasp

### 1. Stops the `--no-verify` escape hatch

Your agent runs the test suite, sees a red, decides the failure is "unrelated to my task", and commits with `--no-verify` (or amends past the hook). The bad path lands on `main`, CI catches it a few minutes later, and you're the one cleaning up.

klasp blocks at the Claude Code `PreToolUse` surface, before the agent's `Bash` call ever runs `git commit`. Failure detail is returned inline so the agent retries against the gate instead of around it.

### 2. CI parity at the agent surface

Agent ships a green-locally PR. CI runs the team's pre-commit, linter, and type-check. PR turns red 30 seconds later and you triage.

One `klasp.toml` declares the same checks. Pass-locally now means pass-CI.

```toml
# Typed recipe form (v0.2 W4) — preferred for pre-commit setups.
[[checks]]
name = "pre-commit"
triggers = [{ on = ["commit"] }]
[checks.source]
type = "pre_commit"           # optional: hook_stage, config_path
```

The typed `type = "pre_commit"` recipe handles the `--hook-stage`, `--from-ref`, `--to-ref` flags internally and parses pre-commit's per-hook output into structured findings the agent can act on. The v0.1 shell form still works:

```toml
[[checks]]
name = "pre-commit"
triggers = [{ on = ["commit"] }]
[checks.source]
type = "shell"
command = "pre-commit run --from-ref $KLASP_BASE_REF --to-ref HEAD"
```

### 3. Diff-scoped checks on big repos

Agent edits one file. The full test suite kicks off. 90 seconds later the agent has lost the thread and you're paying for tokens spent waiting.

Every shell check sees `KLASP_BASE_REF`, set to the merge-base of `HEAD` against the upstream tracking branch. Scope linters, formatters, and audits to the diff:

```toml
[[checks]]
name = "fallow-audit"
triggers = [{ on = ["commit", "push"] }]
[checks.source]
type = "shell"
command = "fallow audit --base $KLASP_BASE_REF"
```

### 4. Protected-path guards

Agent debugs a flaky test by reverting your schema migration. Or your i18n bundle. Or the generated API client. The PR looks clean and the regression surfaces in prod a week later.

Add a check that fails when a staged file lives in a path the agent shouldn't have touched without sign-off:

```toml
[[checks]]
name = "no-migration-edits-without-marker"
triggers = [{ on = ["commit"] }]
[checks.source]
type = "shell"
command = '''
  if git diff --cached --name-only | grep -q "^migrations/"; then
    test -f .agent-migration-allowed && exit 0
    echo "Migration files staged. If intentional, drop a .agent-migration-allowed marker and re-stage."
    exit 1
  fi
'''
```

### 5. Polyglot stacks

Agent onboards a repo with a TS frontend, Go API, and Python ML pipeline. Three test runners, three lint configs, three formatters. Agent guesses wrong and someone has to walk back its commits.

`klasp.toml` declares each surface once:

```toml
[[checks]]
name = "frontend"
triggers = [{ on = ["commit"] }]
[checks.source]
type = "shell"
command = "cd web && pnpm typecheck && pnpm lint"

[[checks]]
name = "api"
triggers = [{ on = ["commit"] }]
[checks.source]
type = "shell"
command = "cd api && go test ./... && go vet ./..."

[[checks]]
name = "ml"
triggers = [{ on = ["commit"] }]
[checks.source]
type = "shell"
command = "cd ml && uv run pytest && uv run ruff check"
```

Worked configs for pre-commit, fallow, pytest, cargo, ESLint/Biome, and ruff live in [`docs/recipes.md`](./docs/recipes.md).

## Install

Pick the package manager that matches your primary stack. All three ship the same binary:

```bash
cargo install klasp                        # Rust (also the right pick for x86 Macs — no prebuilt wheel)
npm i -g @klasp-dev/klasp                  # JS / TS (biome-style platform shim)
pip install klasp                          # Python (maturin wheel)
```

Prebuilt binaries cover `darwin-arm64`, `linux-x64-gnu`, `linux-arm64-gnu`, and `win32-x64`. On x86 Macs and other unsupported targets, `cargo install klasp` builds from source.

### Set up a repo

```bash
cd your-project
klasp init                                 # writes klasp.toml at repo root
$EDITOR klasp.toml                         # add your checks (see below)

# For Claude Code
klasp install --agent claude               # writes .claude/hooks/klasp-gate.sh + merges .claude/settings.json

# For Codex
klasp install --agent codex               # writes .codex/git-hooks/<gate>.sh

# Or both at once
klasp install --agent all                 # installs Claude Code + Codex in one step

klasp doctor                               # verify the install is healthy
```

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
| Codex via `AGENTS.md` + git hooks | v0.2 (W1-W3 shipped) |
| Named recipe: `type = "pre_commit"` | Shipped in v0.2 W4 |
| Named recipe: `type = "fallow"` | Shipped in v0.2 W5 |
| Named recipes: `type = "pytest"` / `"cargo"` | Shipped in v0.2 W6 |
| Per-surface contract (`install_with_warnings` + `doctor_check`) | [v0.2.5][m1] (#55) |
| Gate noop when cwd is outside the project root | [v0.2.5][m1] (#65) |
| Monorepo config discovery (nearest `klasp.toml` wins) | [v0.2.5][m1] (#38) |
| [Public agent-surface conformance matrix](https://github.com/klasp-dev/klasp/issues/68) | [v0.2.5][m1] |
| [Demo repo: same failing commit, three agents, three identical fix paths](https://github.com/klasp-dev/klasp/issues/69) | [v0.2.5][m1] |
| Aider as the third agent surface alongside Claude + Codex | v0.3 (#46) |
| Plugin protocol for third-party `klasp-plugin-*` binaries (experimental) | v0.3 → v1.0 stable |
| Cursor surface (go/no-go decision in v0.3) | v0.3 (#44) |
| Parallel check execution | v0.3+ (see roadmap) |

[m1]: https://github.com/klasp-dev/klasp/milestone/1 "v0.2.5 — surface reliability + repo correctness"

v0.2.5 lands the per-surface contract, fixes the cross-repo gate bug, and publishes a conformance matrix so "klasp supports agent X" means the same thing for every X. See [`docs/roadmap.md`](./docs/roadmap.md) for the full milestone plan.

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
