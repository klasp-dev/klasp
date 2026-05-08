# Polyglot audit recipe

A recipe for repos with **multiple languages in one tree** — Rust
backend + TypeScript frontend, Python service + Go CLI tools, the
classic three-stack monolith. The single-stack recipes
([`./python.md`](./python.md), [`./typescript.md`](./typescript.md),
[`./rust.md`](./rust.md), [`./go.md`](./go.md)) compose; this file
shows the two strategies for stitching them together.

> If the repo has many packages in one language (a pnpm workspace, a
> Cargo workspace), see [`./monorepo.md`](./monorepo.md) instead. If
> it's both polyglot and monorepo (TS workspace + Python service +
> Cargo workspace), read this file then jump to monorepo for
> per-package coordination.

## Audience

This recipe targets one of:

- **Two-stack app**: Rust API (`api/`) + TypeScript dashboard (`web/`).
- **Three-stack monolith**: TS frontend (`apps/web/`) + Python service
  (`services/api/`) + Go CLI tools (`tools/cli/`).
- **Backend split**: Python web app (`web/`) + Go background workers
  (`workers/`) sharing `proto/` definitions.
- **Library + binding**: Rust core (`core/`) with Python bindings
  (`bindings/python/`) and TypeScript types (`bindings/ts/`).

Single-stack repos stay on the language recipe. Many-package single-stack
repos use [`./monorepo.md`](./monorepo.md).

## klasp's nearest-`klasp.toml` discovery

This is the load-bearing primitive for polyglot composition.

When the gate runs, klasp walks up from each staged file's directory to
the first `klasp.toml` it finds, stopping at the repo root. (See
[`klasp-core/src/config.rs`](../../klasp-core/src/config.rs#L218) —
`discover_config_for_path`.) That means:

- A staged `services/api/handler.py` finds `services/api/klasp.toml`
  if one exists; otherwise falls through to the repo-root config.
- A staged `apps/web/src/page.tsx` finds `apps/web/klasp.toml` first,
  then root.
- A staged file outside the repo root returns `None` — gate noop.

This discovery is the same primitive the monorepo recipe uses; here we
exploit it to keep per-language tooling in per-language directories
without cross-stack contamination.

## Strategy A — Single root `klasp.toml`

Best fit when the repo is small (under ~20k lines), when ownership is
shared across all stacks, or when the team prefers one config to read.

The root config declares one `[[checks]]` block per stack, each scoped
to its subdir via `cd <dir> && ...`. klasp runs all blocks on every
trigger; the per-stack tools decide what to do based on the diff they
see.

```toml
# klasp.toml — root config for a polyglot repo
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

# ── TypeScript: apps/web/ ────────────────────────────────────────────────
[[checks]]
name = "web-install"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 90
[checks.source]
type = "shell"
command = "cd apps/web && pnpm install --frozen-lockfile --prefer-offline"

[[checks]]
name = "web-biome"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "cd apps/web && biome check ."

[[checks]]
name = "web-tsc"
triggers = [{ on = ["commit"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = "cd apps/web && tsc --noEmit --incremental"

[[checks]]
name = "web-vitest"
triggers = [{ on = ["push"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "cd apps/web && vitest run --reporter=verbose"

# ── Python: services/api/ ────────────────────────────────────────────────
[[checks]]
name = "api-ruff-format"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "cd services/api && ruff format --check ."

[[checks]]
name = "api-ruff-lint"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "cd services/api && ruff check --no-fix ."

[[checks]]
name = "api-mypy"
triggers = [{ on = ["commit"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "cd services/api && mypy src/"

[[checks]]
name = "api-pytest"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "shell"
command = "cd services/api && pytest -q"

# ── Go: tools/cli/ ───────────────────────────────────────────────────────
[[checks]]
name = "cli-gofmt"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "cd tools/cli && test -z \"$(gofmt -l .)\""

[[checks]]
name = "cli-go-build"
triggers = [{ on = ["commit"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "cd tools/cli && go build ./..."

[[checks]]
name = "cli-go-vet"
triggers = [{ on = ["commit"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "cd tools/cli && go vet ./..."

[[checks]]
name = "cli-go-test"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "shell"
command = "cd tools/cli && go test -count=1 ./..."
```

Pros: one file, easy to grep for the whole gate's behaviour, no
discovery surprises. Cons: every commit runs every stack's checks even
when only one was touched — fast tools (biome, ruff, gofmt) don't care,
but slow ones (`tsc`, `mypy`) bill the agent on unrelated commits.

The fix is staged-file scoping (see below) — but if you find yourself
adding `git diff` filtering to every block, switch to Strategy B.

## Strategy B — Per-language `klasp.toml` per subdir

Best fit when the repo is large, when stacks have separate owners, or
when one stack's checks shouldn't run on the other's commits.

Each language directory gets its own `klasp.toml`. klasp's discovery
walks from the staged file up — a staged Python file under
`services/api/` resolves `services/api/klasp.toml`; a staged Go file
under `tools/cli/` resolves `tools/cli/klasp.toml`; cross-cutting changes
(both stacks staged in one commit) trigger both gates.

Layout:

```text
repo/
  klasp.toml                     # root: cross-stack only
  apps/web/
    klasp.toml                   # TS stack, full single-stack recipe
    package.json
    tsconfig.json
    src/...
  services/api/
    klasp.toml                   # Python stack, full single-stack recipe
    pyproject.toml
    src/...
  tools/cli/
    klasp.toml                   # Go stack, full single-stack recipe
    go.mod
    cmd/...
```

Root `klasp.toml` (cross-cutting only — license headers, generated-file
guards, schema migrations the agent shouldn't touch):

```toml
# klasp.toml — root config for cross-cutting gates only
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

# Block edits to vendored proto definitions without an explicit marker file.
[[checks]]
name = "no-proto-edits-without-marker"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = '''
  if git diff --cached --name-only | grep -q "^proto/"; then
    test -f .agent-proto-allowed && exit 0
    echo "proto/ files staged. Drop a .agent-proto-allowed marker if intentional and re-stage."
    exit 1
  fi
'''
```

`apps/web/klasp.toml` (full TypeScript Tier 2 recipe — see
[`./typescript.md`](./typescript.md)):

```toml
# apps/web/klasp.toml
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

[[checks]]
name = "install"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 90
[checks.source]
type = "shell"
command = "pnpm install --frozen-lockfile --prefer-offline"

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
triggers = [{ on = ["push"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "vitest run --reporter=verbose"
```

`services/api/klasp.toml` (full Python Tier 2 — see
[`./python.md`](./python.md)):

```toml
# services/api/klasp.toml
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

[[checks]]
name = "ruff-format"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "ruff format --check ."

[[checks]]
name = "ruff-lint"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "ruff check --no-fix ."

[[checks]]
name = "mypy"
triggers = [{ on = ["commit"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "mypy src/"

[[checks]]
name = "pytest"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "pytest"
extra_args = "-q"
junit_xml = true
```

`tools/cli/klasp.toml` (full Go Tier 2 — see [`./go.md`](./go.md)):

```toml
# tools/cli/klasp.toml
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

[[checks]]
name = "gofmt"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "test -z \"$(gofmt -l .)\""

[[checks]]
name = "go-build"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "go build ./..."

[[checks]]
name = "go-vet"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "go vet ./..."

[[checks]]
name = "go-test"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "shell"
command = "go test -count=1 ./..."
```

Pros: per-stack ownership is explicit, each stack's check definitions
live next to its source, and a single-stack commit only runs that
stack's gate (klasp's discovery picks the leaf config). Cons: more
files to maintain, and a commit touching two stacks runs both gates
sequentially per discovery resolution — confirm `parallel = true` is
set in each.

## Trade-offs

| Concern                              | Strategy A (root) | Strategy B (per-dir) |
|--------------------------------------|-------------------|----------------------|
| File count                           | 1                 | 1 + N stacks         |
| Single-stack commit cost             | Runs all stacks (waste) | Runs only the leaf config's checks |
| Cross-stack commit cost              | Runs all stacks (correct) | Runs each affected leaf's checks (correct) |
| Clarity for a new contributor        | Read one file     | Read four files; navigation matches source layout |
| Per-stack ownership boundaries       | Implicit (block names) | Explicit (file location) |
| Schema migration: bumping `version`  | One file          | N+1 files            |
| `parallel = true` semantics          | One pool          | One pool per discovered config |
| Adding a new stack                   | Append blocks     | Add one new `klasp.toml` |
| Cross-cutting gates (proto guards)   | Inline as a block | Lives in root config |

Default recommendation: **Strategy A under ~20k lines or three stacks**;
**Strategy B above that**. Polyglot repos with three or more stacks
almost always benefit from B; two-stack repos are usually fine on A.

## Staged-file scoping with `KLASP_BASE_REF`

Both strategies benefit from diff-aware scoping when individual stacks
have slow tools. Every shell check sees `KLASP_BASE_REF` set to the
merge-base of `HEAD` against the upstream tracking branch.

**Strategy A** — wrap each stack's slow checks in a guard that exits 0
when no files of that stack changed:

```toml
[[checks]]
name = "web-tsc"
triggers = [{ on = ["commit"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = '''
  changed=$(git diff --name-only --diff-filter=ACM "$KLASP_BASE_REF" -- 'apps/web/**/*.ts' 'apps/web/**/*.tsx')
  if [ -z "$changed" ]; then exit 0; fi
  cd apps/web && tsc --noEmit --incremental
'''

[[checks]]
name = "api-mypy"
triggers = [{ on = ["commit"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = '''
  changed=$(git diff --name-only --diff-filter=ACM "$KLASP_BASE_REF" -- 'services/api/**/*.py')
  if [ -z "$changed" ]; then exit 0; fi
  cd services/api && mypy src/
'''

[[checks]]
name = "cli-go-test"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "shell"
command = '''
  changed=$(git diff --name-only --diff-filter=ACM "$KLASP_BASE_REF" -- 'tools/cli/**/*.go')
  if [ -z "$changed" ]; then exit 0; fi
  cd tools/cli && go test -count=1 ./...
'''
```

This is the pattern that, once repeated for every slow check, signals
you should switch to Strategy B — klasp's per-leaf discovery is a
cleaner version of the same idea.

**Strategy B** — discovery already does the scoping. Each leaf
`klasp.toml` only fires when files under its subtree are staged. No
shell guards needed.

## Concrete example: three-stack layout

A real `apps/web` (TS) + `services/api` (Python) + `tools/cli` (Go)
repo. Numbers below are typical, not normative.

| Stack          | LOC   | Slow check       | Cold-cache wall time |
|----------------|-------|------------------|----------------------|
| `apps/web`     | 35k   | `tsc --noEmit`   | ~25s                 |
| `services/api` | 18k   | `mypy src/`      | ~12s                 |
| `tools/cli`    | 6k    | `go test ./...`  | ~8s                  |

On a commit that touches only `apps/web/src/page.tsx`:

- **Strategy A** (no diff guards): runs web (25s) + api (12s) + cli
  fast checks (~3s) = **40s wall time**.
- **Strategy A** (with diff guards above): runs web (25s) + api (1s
  guard skip) + cli (1s guard skip) = **27s**.
- **Strategy B**: discovery picks `apps/web/klasp.toml`, runs web only
  = **25s**.

The wins from A → B grow with stack count. At three stacks B saves
~15s per cross-stack commit; at five stacks it's ~30s. The per-stack
recipe blocks are identical between strategies — only the location
changes.

## Trigger split across stacks

The commit/push split from each single-stack recipe carries through.
The cross-stack rule of thumb: if every stack's commit-tier checks
finish in <5s on warm cache, leave them on commit. If any one stack's
commit tier crosses 30s on cold cache, demote it to push and let the
agent iterate via the faster stacks first.

| Trigger | What runs | Why                                                  |
|---------|-----------|------------------------------------------------------|
| commit  | Format checks (gofmt, ruff format, biome), fast linters (ruff lint, biome, go vet), incremental typecheck (`tsc --incremental`, `mypy` warm cache) | Sub-30s on warm cache across all three stacks. |
| push    | Full test suites (vitest, pytest, go test), coverage thresholds, dead-code detection (knip, vulture, machete), CVE scans (pip-audit, govulncheck) | Slower; agent has already iterated past commit gates. |

## Custom triggers

If the team uses a deploy script that touches multiple stacks at once,
declare a `[[trigger]]` so klasp fires on it. Place the trigger in the
root config in Strategy B — discovery from a script invocation walks
from cwd, so a root-level trigger fires even when the script touches
files under a leaf:

```toml
# In root klasp.toml under Strategy B
[[trigger]]
name = "deploy-script"
commands = ["./scripts/deploy"]
```

See [`../recipes.md`](../recipes.md#custom-trigger-blocks-v03) for the
full custom-trigger reference.

## See also

- [`./README.md`](./README.md) — audit recipe philosophy.
- [`./monorepo.md`](./monorepo.md) — many packages in one stack
  (read this if your repo is also a workspace).
- [`./python.md`](./python.md), [`./typescript.md`](./typescript.md),
  [`./rust.md`](./rust.md), [`./go.md`](./go.md) — single-stack
  recipes that compose into the snippets above.
- [`../recipes.md`](../recipes.md) — per-tool typed recipe reference,
  verdict-policy guidance.
- [`klasp-core/src/config.rs`](../../klasp-core/src/config.rs#L218) —
  `discover_config_for_path`, the load-bearing primitive.
