# Audit recipes

Copy-pasteable `klasp.toml` configurations for real codebases. Each recipe
is a tiered starting point — pick the smallest one that covers your
quality bar today, graduate up as the team's tolerance for blocked
commits grows.

> klasp is the agent gate and verdict normalizer; users bring the checks.
> The recipes show how to wire those checks into an agent-safe gate
> without designing the gate from scratch.

## What an audit recipe is

A curated set of tier-stratified `klasp.toml` configurations plus the
tooling guidance that goes with them, scoped to one stack. Every recipe
includes:

- target audience and repo shape
- required tools and install commands
- complete `klasp.toml` snippets for each tier
- a recommended `commit` vs `push` split
- hard-block vs warn-only guidance per check
- baselining patterns for legacy repos
- `KLASP_BASE_REF` patterns for diff-scoping
- expected agent-visible output for each tool
- false-positive escape hatches
- a graduation path to a first-party plugin

Each recipe is shell glue. They don't extend klasp — they exercise it.

## What an audit recipe is not

| Not                                  | Because                                                 |
|--------------------------------------|---------------------------------------------------------|
| A klasp plugin (`klasp-plugin-*`)    | Recipes are config + docs only. Plugins are binaries. |
| A built-in check source              | Recipes use `type = "shell"` and the existing typed recipes (`pre_commit`, `fallow`, `pytest`, `cargo`). |
| A security boundary                  | klasp blocks at the agent's tool-call surface, not at OS or filesystem level. The recipe's bandit/cargo-deny/govulncheck checks are quality gates, not sandboxes. |
| A replacement for ruff / cargo / pytest / etc. | klasp wraps the tools you already use. The recipe is glue, not a fork. |
| Auto-fix                              | Every linter call uses `--check` / `--no-fix`. The gate surfaces findings; the agent fixes them. |

For the surrounding klasp config shape (`version = 1` header, `[gate]`
block, verdict policies), see [`../recipes.md`](../recipes.md). For
plugin authoring, see [`../plugins.md`](../plugins.md).

## The three-tier model

Most recipes ship three tiers. The dividing lines are runtime cost and
false-positive rate, not feature count.

| Tier      | What it catches                                                | When to adopt |
|-----------|----------------------------------------------------------------|---------------|
| Minimal   | Broken builds, formatting drift, obvious lint errors, broken tests | Day one on any repo. Below this floor an agent can land code that doesn't compile. |
| Serious   | Type errors, dependency hygiene, dead code, deeper lint rule sets | Once Tier 1 lands green and the team wants to catch mistyped signatures, abandoned imports, and unused deps. |
| Strict    | Supply-chain CVEs, coverage thresholds, complexity ceilings, feature-matrix builds | Production-ship repos, compliance-bound codebases, anything where a regressed CVE is release-blocking. |

Tiers are additive — Tier 2 is Tier 1 plus more checks; Tier 3 is Tier 2
plus more. Drop in the smallest tier the team can ship green, leave it
for a sprint, then promote.

## Available recipes

Single-stack recipes:

| Recipe                              | Stack covers                                              |
|-------------------------------------|-----------------------------------------------------------|
| [`./python.md`](./python.md)        | ruff, pytest, mypy/pyright, deptry, vulture, bandit, pip-audit, radon |
| [`./typescript.md`](./typescript.md)| tsc, biome / eslint+prettier, vitest/jest, knip, fallow   |
| [`./rust.md`](./rust.md)            | cargo fmt/check/clippy/test, cargo-deny, machete, hack, msrv |
| [`./go.md`](./go.md)                | gofmt, go vet, staticcheck, golangci-lint, govulncheck, nilaway |

Composition recipes:

| Recipe                                  | What it covers                                              |
|-----------------------------------------|-------------------------------------------------------------|
| [`./polyglot.md`](./polyglot.md)        | Multiple languages in one repo (e.g. TS frontend + Python service + Go CLI). Strategies for one root config vs per-language configs. |
| [`./monorepo.md`](./monorepo.md)        | Many packages in one stack. pnpm/yarn/turbo TS monorepos, Cargo workspaces, Go workspaces. Per-package configs + root coordination, affected-package detection. |

If your repo is single-language, single-package, start with the
language recipe. If it's polyglot or monorepo, read the language recipe
for each stack first, then compose via [`./polyglot.md`](./polyglot.md)
or [`./monorepo.md`](./monorepo.md).

## Reading order

For a new user landing on a real repo:

1. Read this file (you're here).
2. Open the language recipe matching the repo's primary stack
   ([`./python.md`](./python.md), [`./typescript.md`](./typescript.md),
   [`./rust.md`](./rust.md), or [`./go.md`](./go.md)).
3. Copy Tier 1's `klasp.toml` snippet into the repo root. Run
   `klasp install --agent all`. Attempt a commit. Iterate until green.
4. If the repo has multiple languages, jump to
   [`./polyglot.md`](./polyglot.md) for composition guidance.
5. If the repo has many packages in one stack, jump to
   [`./monorepo.md`](./monorepo.md) for per-package layout.
6. Promote Tier 1 → 2 → 3 as the team's tolerance grows.

Skip the strict tier on day one of a legacy repo — it will block every
commit on inherited debt the team has no plan to fix this sprint.

## Graduation path

A recipe is shell glue. Once enough teams run the same Tier 2 or Tier 3
configuration with the same `xargs` and `git diff` plumbing, the recipe
is a candidate for a first-party plugin:

| Recipe        | Graduation candidate          |
|---------------|-------------------------------|
| python.md     | `klasp-plugin-python`         |
| typescript.md | `klasp-plugin-typescript`     |
| rust.md       | `klasp-plugin-rust`           |
| go.md         | `klasp-plugin-go`             |

The plugin would consolidate per-tool `[[checks]]` blocks under one
`type = "plugin"` config that parses each tool's structured output
(ruff JSON, mypy `--show-error-codes`, cargo `--message-format=json`,
go vet `-json`, …) into klasp `Finding` rows the agent can navigate to
directly.

Until those plugins exist, the recipes are the canonical reference. If
you adopt one on a real repo, file the rough edges as issues against
[klasp-dev/klasp](https://github.com/klasp-dev/klasp/issues) — that's
the demand signal the plugin's prioritisation runs on.

## See also

- [`../recipes.md`](../recipes.md) — per-tool typed recipe reference and
  verdict-policy guidance.
- [`../plugins.md`](../plugins.md) — plugin authoring guide, fork
  `examples/klasp-plugin-pre-commit/`.
- [`../../klasp.toml`](../../klasp.toml) — klasp's own dogfood config
  (a real Rust workspace gated by klasp).
