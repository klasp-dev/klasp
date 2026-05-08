# Monorepo audit recipe

A recipe for **many packages in one stack, sharing tooling** — pnpm /
yarn / turbo / nx TypeScript monorepos, Cargo workspaces, Go workspaces.
The single-stack recipes ([`./python.md`](./python.md),
[`./typescript.md`](./typescript.md), [`./rust.md`](./rust.md),
[`./go.md`](./go.md)) describe per-stack tooling; this file shows how
to layer per-package configs on top of root coordination so the agent
gets fast feedback on the package it touched without billing the rest
of the workspace.

> If the repo has multiple languages, read [`./polyglot.md`](./polyglot.md)
> first. If it's both polyglot and monorepo (TS workspace + Python
> service + Go workspace), use polyglot's per-language strategy at the
> top level, then this recipe within each language's subtree.

## Audience

This recipe targets one of:

- **TS monorepo**: pnpm / yarn / npm workspaces, often with turbo or
  nx. `apps/*` (deployable apps) plus `packages/*` (shared libraries).
  The most common shape — examples below default here.
- **Cargo workspace**: a `Cargo.toml` `[workspace]` block at the root
  with N member crates. Shared `target/`, shared lockfile.
- **Go workspace**: a `go.work` file at the root with several `go.mod`
  siblings. Shared module graph.
- **Python uv workspace**: a top-level `pyproject.toml` with
  `[tool.uv.workspace]` and member packages.

The TS case is the densest worked example below. The bottom of the
file covers Cargo and Go workspace adaptations.

## Per-package `klasp.toml` + root coordination

Same primitive as polyglot: klasp walks up from each staged file to the
nearest `klasp.toml`. (See
[`klasp-core/src/config.rs`](../../klasp-core/src/config.rs#L218) —
`discover_config_for_path`.) In a monorepo this means:

- A staged file under `packages/shared/src/util.ts` resolves
  `packages/shared/klasp.toml` if one exists; otherwise falls through
  to root.
- A staged file under `apps/web/app/page.tsx` resolves
  `apps/web/klasp.toml`; otherwise root.
- A commit touching files in two packages runs each leaf's gate per
  affected file path.

Two coordination patterns:

1. **Root config does the heavy lifting** — one `klasp.toml` at the
   workspace root runs install, lint, typecheck, and test for the
   whole workspace. No per-package configs. Simplest; right answer
   for small workspaces (<10 packages, <50k LOC total).
2. **Per-package configs override root for affected packages** — root
   handles cross-cutting (install integrity, dependency drift); each
   package owns its own checks. Right answer once the workspace is
   large enough that running `vitest --run` across every package on
   every commit hurts.

Pattern 1 → Pattern 2 is the graduation path; both ship below.

## Pattern 1 — Root-only, scoped via affected packages

For a TS pnpm monorepo with `apps/`, `services/`, and `packages/`. Uses
`pnpm` filters to scope each check to changed packages, with
`KLASP_BASE_REF` driving the diff.

```toml
# klasp.toml — root config, root-only pattern
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

# Lockfile integrity — runs once at the workspace root, applies to all.
[[checks]]
name = "install"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 90
[checks.source]
type = "shell"
command = "pnpm install --frozen-lockfile --prefer-offline"

# Lint affected packages only. `--filter "...[<ref>]"` selects packages
# whose source changed since <ref>, including transitive consumers.
[[checks]]
name = "biome-affected"
triggers = [{ on = ["commit"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "pnpm --filter \"...[${KLASP_BASE_REF}]\" run lint"

# Typecheck affected packages only.
[[checks]]
name = "tsc-affected"
triggers = [{ on = ["commit"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "pnpm --filter \"...[${KLASP_BASE_REF}]\" run typecheck"

# Test affected packages only on commit; full workspace on push.
[[checks]]
name = "vitest-affected"
triggers = [{ on = ["commit"] }]
timeout_secs = 300
[checks.source]
type = "shell"
command = "pnpm --filter \"...[${KLASP_BASE_REF}]\" run test"

[[checks]]
name = "vitest-all"
triggers = [{ on = ["push"] }]
timeout_secs = 600
[checks.source]
type = "shell"
command = "pnpm -r run test"

# Audit the lockfile on push. Workspace-wide; not affected-scoped.
[[checks]]
name = "audit"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "pnpm audit --audit-level=high --prod"
```

`pnpm --filter "...[<ref>]"` is the load-bearing trick. The filter
syntax `...[<ref>]` selects packages with changes since `<ref>` plus
all packages that depend on them — exactly the affected-graph closure.
For yarn berry, swap to `yarn workspaces foreach --since <ref> run
<task>`. For npm workspaces, see the workaround in the affected-package
detection table below.

This pattern requires per-package `package.json` scripts (`lint`,
`typecheck`, `test`) so `pnpm --filter` has something to dispatch.

## Pattern 2 — Root + per-package configs

For a TS pnpm monorepo with `apps/web`, `apps/admin`, `services/api`,
and `packages/shared`. Root handles cross-cutting; each package owns
its checks.

Layout:

```text
repo/
  klasp.toml                     # cross-cutting: install, audit, dep-drift
  pnpm-workspace.yaml
  package.json
  apps/
    web/
      klasp.toml                 # web's own commit gate
      package.json
      src/...
    admin/
      klasp.toml                 # admin's own commit gate
      package.json
      src/...
  services/
    api/
      klasp.toml                 # api's own commit gate
      package.json
      src/...
  packages/
    shared/
      klasp.toml                 # shared lib's own commit gate
      package.json
      src/...
```

Root `klasp.toml`:

```toml
# klasp.toml — root, cross-cutting only
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

# Workspace install — fires for every commit because lockfile drift
# affects every package.
[[checks]]
name = "install"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 90
[checks.source]
type = "shell"
command = "pnpm install --frozen-lockfile --prefer-offline"

# Dependency-drift detection across the workspace. Runs on push only.
[[checks]]
name = "knip-workspace"
triggers = [{ on = ["push"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "knip --no-progress --reporter compact"

# Audit on push, workspace-wide.
[[checks]]
name = "audit"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "pnpm audit --audit-level=high --prod"

# Block edits to the lockfile without an explicit allow marker.
[[checks]]
name = "lockfile-guard"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = '''
  if git diff --cached --name-only | grep -qE "^(pnpm-lock\\.yaml|package-lock\\.json|yarn\\.lock)$"; then
    test -f .agent-lockfile-allowed && exit 0
    echo "Lockfile staged. Drop a .agent-lockfile-allowed marker if intentional and re-stage."
    exit 1
  fi
'''
```

`apps/web/klasp.toml`:

```toml
# apps/web/klasp.toml
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

[[checks]]
name = "biome"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "biome check ."

[[checks]]
name = "tsc"
triggers = [{ on = ["commit"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = "tsc --noEmit --incremental"

[[checks]]
name = "vitest"
triggers = [{ on = ["commit"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "vitest run --reporter=verbose"
```

`apps/admin/klasp.toml`, `services/api/klasp.toml`, and
`packages/shared/klasp.toml` follow the same shape — pull the relevant
tier from [`./typescript.md`](./typescript.md). Cross-package commits
fire each affected leaf's gate; root-level checks run regardless.

## Affected-package detection by stack

`KLASP_BASE_REF` plus `git diff --name-only` is the foundation. Each
ecosystem has a native filter on top:

| Ecosystem            | Affected-package command                                   |
|----------------------|------------------------------------------------------------|
| pnpm                 | `pnpm --filter "...[${KLASP_BASE_REF}]" run <task>`        |
| yarn berry           | `yarn workspaces foreach --since ${KLASP_BASE_REF} run <task>` |
| yarn classic         | No native filter; pipe `git diff --name-only` to `awk` to extract package paths, then `yarn workspace <pkg> run <task>` per match. |
| npm workspaces       | No native filter; use the `git diff` extraction pattern below. |
| turbo                | `turbo run <task> --filter="...[${KLASP_BASE_REF}]"`       |
| nx                   | `nx affected -t <task> --base=${KLASP_BASE_REF}`           |
| Cargo workspace      | No native filter; `git diff` → `dirname` → `cargo -p <crate>` mapping (fragile — see notes below). |
| Go workspace         | `git diff --name-only ${KLASP_BASE_REF} -- '*.go' \| xargs -r -n1 dirname \| sort -u` and pass to `go test`. |

For tools without a native filter, the canonical extraction:

```toml
[[checks]]
name = "test-affected-fallback"
triggers = [{ on = ["commit"] }]
timeout_secs = 300
[checks.source]
type = "shell"
command = '''
  pkgs=$(git diff --name-only --diff-filter=ACM "$KLASP_BASE_REF" \
    | awk -F/ '/^(apps|packages|services)\//{print $1"/"$2}' \
    | sort -u)
  if [ -z "$pkgs" ]; then exit 0; fi
  for pkg in $pkgs; do
    (cd "$pkg" && pnpm run test) || exit 1
  done
'''
```

The `awk` filter pins the path shape — `apps/<name>/` or
`packages/<name>/` — so changes to repo-root files (configs, READMEs)
don't get classified as a package.

## Cache strategy at scale

Per-tool caching is the difference between a 5-minute commit and a
5-second commit at workspace scale. Hints by stack:

| Tool                  | Cache location                | Notes                                                         |
|-----------------------|-------------------------------|---------------------------------------------------------------|
| `tsc --incremental`   | `.tsbuildinfo` per package    | Gitignore. Per-checkout — fresh `git clone` runs are slow.   |
| ESLint `--cache`      | `.eslintcache` per package    | Required for any commit-tier eslint check on a workspace.    |
| Biome                 | None needed                    | Already millisecond-fast on full repos.                       |
| Vitest                | `node_modules/.vitest`        | `--changed ${KLASP_BASE_REF}` (Vitest 1.6+) for diff scoping. |
| Turbo                 | `node_modules/.cache/turbo`   | Remote cache via `turbo login` — sccache for TS.             |
| nx                    | `node_modules/.cache/nx`      | `nx affected` keys cache by content hash, not file mtime.    |
| Cargo                 | `target/` (workspace-shared)  | Don't `rm -rf target/` between agent iterations. `sccache` for cross-machine. |
| sccache               | `~/.cache/sccache` (local) / S3 / GCS | Set `RUSTC_WRAPPER=sccache`. Worth it on >10-crate workspaces. |
| `go build` / `go test`| `$GOCACHE` (default `~/.cache/go-build`) | Shared across the workspace. `go test -count=1` only on push. |
| ruff                  | `.ruff_cache/`                | Workspace-wide; gitignore.                                   |
| mypy                  | `.mypy_cache/` per package    | Per-package; preserved between iterations.                   |

Turbo and nx do package-graph caching natively — if you're already
using them, run klasp checks through `turbo run` / `nx affected` to
inherit the cache:

```toml
[[checks]]
name = "turbo-typecheck"
triggers = [{ on = ["commit"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "turbo run typecheck --filter=\"...[${KLASP_BASE_REF}]\""

[[checks]]
name = "turbo-test"
triggers = [{ on = ["commit"] }]
timeout_secs = 300
[checks.source]
type = "shell"
command = "turbo run test --filter=\"...[${KLASP_BASE_REF}]\""
```

Turbo's content-addressable cache means a re-run of an unchanged
package's typecheck is ~10ms (cache hit) instead of 25s (cold tsc).
That changes the math on what belongs on commit.

## Hard-block on changed packages, warn on the rest

A common monorepo pattern: strict gating on packages the agent
actually touched, informational warnings on the rest. Two ways to
express it.

**Approach A — Per-package leaf configs are the strict tier**: each
package's `klasp.toml` uses `policy = "any_fail"`. Root-level checks
that scan the whole workspace use `policy = "majority_fail"` so a
single noisy finding in an untouched package doesn't block.

This is the recommended default: discovery means the strict gate only
fires on touched packages, so leaf configs can stay strict without
blocking on inherited debt elsewhere.

**Approach B — Two checks per concern, one strict (affected) and one
warn-only (workspace)**:

```toml
# Strict on affected packages.
[[checks]]
name = "tsc-affected"
triggers = [{ on = ["commit"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "pnpm --filter \"...[${KLASP_BASE_REF}]\" run typecheck"

# Warn-only on the rest of the workspace.
[[checks]]
name = "tsc-workspace-warn"
triggers = [{ on = ["push"] }]
timeout_secs = 600
[checks.source]
type = "shell"
command = "pnpm -r run typecheck || true"
```

The trailing `|| true` forces exit 0, so klasp records `Pass` and the
agent never sees the finding. If you want the agent to *see* but not
be blocked, switch the gate's `policy` to `majority_fail` and let one
warn-only block sit alongside multiple strict ones — `majority_fail`
downgrades to `Warn` when at least one check passes. See
[`../recipes.md`](../recipes.md#verdict-policies) for the policy
matrix.

## Concrete example: realistic TS monorepo

A pnpm + turbo workspace with four packages:

```text
repo/
  pnpm-workspace.yaml
  package.json
  turbo.json
  klasp.toml                                # root: install, audit, lockfile-guard, knip
  apps/
    web/
      klasp.toml                            # biome, tsc, vitest (commit), playwright (push)
      package.json
    admin/
      klasp.toml                            # biome, tsc, vitest (commit), playwright (push)
      package.json
  services/
    api/
      klasp.toml                            # biome, tsc, vitest (commit), pact-tests (push)
      package.json
  packages/
    shared/
      klasp.toml                            # biome, tsc, vitest (commit), api-extractor (push)
      package.json
```

On a commit touching only `apps/web/src/page.tsx`:

- klasp's discovery from cwd resolves the root `klasp.toml` for the
  built-in commit/push trigger. The root config's `install`,
  `lockfile-guard`, and (on push) `knip-workspace` and `audit` fire.
- The web package's commit-tier gate runs (biome, tsc, vitest), gated
  by `apps/web/klasp.toml` — but only because `klasp gate` walks from
  the project root (the agent's cwd at commit time), so per-leaf
  configs are exercised via the wrappers in Pattern 1 / Pattern 2's
  shell commands, not by file-path discovery during the gate run
  itself.

In practice this means: root config's `pnpm --filter` blocks scope to
affected packages by their own logic; leaf `klasp.toml` files take
effect when an agent operates from inside the package directory or
when a per-package install hook routes there. For the cleanest
behaviour today, use Pattern 1 (root-only with affected-package
filtering) on Claude Code; Pattern 2's leaf configs become more useful
as agent-side cwd-routing matures.

| Trigger | Checks fired                                                              | Wall time (warm) |
|---------|---------------------------------------------------------------------------|------------------|
| commit  | `install`, `lockfile-guard`, `biome-affected`, `tsc-affected`, `vitest-affected` | ~6s              |
| push    | All commit checks + `knip-workspace`, `vitest-all`, `audit`               | ~90s             |

The `--filter "...[${KLASP_BASE_REF}]"` selector is what keeps commit
under 10s on a workspace that would otherwise run 4×25s tsc passes
sequentially.

## Cargo workspaces

Cargo's compilation graph is whole-crate by design, so per-package
`klasp.toml` files in a workspace are usually less useful than in TS —
the benefit only kicks in when a single member crate is dramatically
slower than the rest.

The dogfood [`/klasp.toml`](../../klasp.toml) at the klasp repo root
is the canonical Cargo-workspace example. Its strategy:

```toml
# At workspace root.
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

[[checks]]
name = "cargo-fmt"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "cargo fmt --all -- --check"

# Workspace-wide check; cargo's incremental cache makes this sub-second
# on warm target/.
[[checks]]
name = "cargo-check"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "cargo check --workspace --all-targets"

# clippy on commit; --workspace runs across all members.
[[checks]]
name = "cargo-clippy"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 180
[checks.source]
type = "cargo"
subcommand = "clippy"
extra_args = "--workspace --all-targets -- -D warnings"

# Tests on push only; workspace-wide.
[[checks]]
name = "cargo-test"
triggers = [{ on = ["push"] }]
timeout_secs = 600
[checks.source]
type = "cargo"
subcommand = "test"
extra_args = "--workspace"
```

For one member crate that dominates wall time (large codegen,
integration tests), pin it with `package = "<crate>"` in the typed
`type = "cargo"` recipe and put it on a separate trigger:

```toml
[[checks]]
name = "cargo-test-slow-crate"
triggers = [{ on = ["push"] }]
timeout_secs = 1200
[checks.source]
type = "cargo"
subcommand = "test"
package = "klasp-integration-tests"
```

Then exclude it from the workspace-wide block via `--workspace
--exclude klasp-integration-tests` if you want the rest faster on
commit. See [`./rust.md`](./rust.md#workspace-specific-notes) for
workspace-specific patterns.

## Go workspaces

`go.work` workspaces share the same module graph and the same `$GOCACHE`,
so a workspace-wide `go test ./...` is usually fast enough on push.
Per-module `klasp.toml` files become useful when individual modules
need different lint configurations:

```text
repo/
  go.work
  klasp.toml              # workspace-wide: gofmt, govulncheck, go-mod-tidy
  cmd/cli/
    go.mod
    klasp.toml            # cli-specific tests + golangci-lint with cli's .golangci.yml
  services/api/
    go.mod
    klasp.toml            # api integration tests, stricter staticcheck rules
  packages/shared/
    go.mod                # no klasp.toml; root applies
```

See [`./go.md`](./go.md#module-aware-notes) for the full pattern.
`govulncheck` always runs across the whole workspace — vulnerability
reachability is a whole-program property — so keep it in root.

## Cross-link to polyglot

If the monorepo is also polyglot (TS workspace + Python service + Go
workspace under one repo root), use polyglot's per-language strategy
([`./polyglot.md`](./polyglot.md#strategy-b--per-language-klasptoml-per-subdir))
at the top level, then this recipe within each language's subtree.
Discovery composes naturally — a staged TS file resolves the nearest
TS package's `klasp.toml`, a staged Python file resolves the Python
service's, etc.

## Trigger split

Same principle as polyglot, with workspace-affected-scoping layered
on:

| Trigger | What runs                                                                | Why                                                  |
|---------|--------------------------------------------------------------------------|------------------------------------------------------|
| commit  | Install integrity, lockfile-guard, lint (affected), typecheck (affected), test (affected) | Affected-only scoping keeps wall time under 10s on warm cache. |
| push    | Test (workspace), audit, dependency drift (knip / cargo-machete / depcheck), CVE scans | Workspace-wide; agent has already iterated past commit. |

## Custom triggers

Monorepo-aware build wrappers (turbo, nx, custom `./scripts/build`)
should each get a `[[trigger]]` so klasp fires on the agent's actual
command:

```toml
[[trigger]]
name = "turbo-build"
pattern = "^turbo\\s+run"

[[trigger]]
name = "nx-affected"
pattern = "^nx\\s+affected"
```

See [`../recipes.md`](../recipes.md#custom-trigger-blocks-v03) for the
full custom-trigger reference.

## See also

- [`./README.md`](./README.md) — audit recipe philosophy.
- [`./polyglot.md`](./polyglot.md) — multiple languages in one repo
  (read this if the monorepo is also polyglot).
- [`./typescript.md`](./typescript.md) — TS single-stack recipe.
- [`./rust.md`](./rust.md) — Rust single-stack recipe (workspaces).
- [`./go.md`](./go.md) — Go single-stack recipe (`go.work`).
- [`../recipes.md`](../recipes.md) — per-tool typed recipe reference,
  verdict-policy guidance.
- [`klasp-core/src/config.rs`](../../klasp-core/src/config.rs#L218) —
  `discover_config_for_path`, the load-bearing primitive.
- [`../../klasp.toml`](../../klasp.toml) — klasp's own dogfood
  Cargo-workspace config.
