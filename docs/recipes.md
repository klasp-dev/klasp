# klasp recipes (v0.1, v0.2, v0.3)

Worked `klasp.toml` snippets for the most common check tools. Every snippet is
copy-pasteable into the `[[checks]]` section of your config; for the surrounding
shape, see [`design.md` §3.5](./design.md#35-configv1-versioned-config) or the
project's own dogfood config at [`/klasp.toml`](../klasp.toml).

> v0.1 shipped exactly one check source: `type = "shell"`. v0.2 W4 added the
> first typed recipe — `type = "pre_commit"` — alongside it; W5 adds
> `type = "fallow"`; W6 adds `type = "pytest"` and `type = "cargo"`,
> finishing the v0.2 named-recipe slate. The shell form continues to work
> unchanged for any tool a recipe doesn't cover yet — see
> [roadmap.md §v0.2](./roadmap.md#v02--codex--named-recipes-target-3-months-from-v01).

## Patterns

### Custom `[[trigger]]` blocks (v0.3+)

Built-in klasp triggers fire when the agent runs `git commit` or `git push`.
v0.3 adds user-configurable `[[trigger]]` blocks so you can extend this to
custom workflows the built-in regex doesn't catch:

```toml
# Fire on the exact command "gh pr create" for any agent.
[[trigger]]
name = "gh-pr"
commands = ["gh pr create"]

# Fire on any `make deploy` variant.
[[trigger]]
name = "make-deploy"
pattern = "^make\\s+deploy"
```

Rules:

- `pattern` — Rust regex tested against the full tool-input command string.
- `commands` — exact strings; matched in full (no substring).
- `agents` — restrict firing to listed agents; empty = all agents.
- At least one of `pattern` or `commands` is required per block.
- When both `pattern` and `commands` are set, a command fires if it matches
  *either* (the two are OR'd, not AND'd).
- User triggers **extend** the built-in commit/push triggers; they do not
  replace them. The built-in classifier wins for any command containing
  `git commit` or `git push` (including wrapped invocations like
  `jj git push`). User triggers therefore cannot use the `agents` filter
  to *restrict* built-in matches — only to *add* matches the built-in misses.
  If you need agent-specific commit/push gating, use `[gate].agents` at the
  config level instead.

### Commit vs push triggers

`triggers = [{ on = ["commit"] }]` runs the check when the agent attempts a
`git commit`. `triggers = [{ on = ["push"] }]` runs on `git push`. List both
to run on either.

A practical split:

| Trigger | Use for | Why |
|---|---|---|
| `commit` | Type/borrow checks, fast linters, formatting checks | The agent will retry the commit immediately on failure; fast feedback wins. |
| `push` | Full test suite, slow integration linters, coverage runs | The agent has already committed; blocking at push catches what fast checks missed without billing the wall time on every commit. |
| both | Linters whose violations should never reach `origin` | Belt-and-braces. Fine if the linter is fast enough that the doubled cost is acceptable. |

### `${KLASP_BASE_REF}`

Every shell check sees `KLASP_BASE_REF` in its environment, set by the gate
runtime to the merge-base of `HEAD` against the upstream tracking branch
(falling back to `HEAD~1` when no upstream is configured). Use it to scope
diff-aware tools to just the changed files — usually a 10x-100x wall-time
reduction on large repos:

```toml
command = "pre-commit run --hook-stage pre-commit --from-ref ${KLASP_BASE_REF} --to-ref HEAD"
command = "fallow audit --base ${KLASP_BASE_REF} --quiet --format json"
```

Tools that don't take a base ref (cargo, pytest, eslint with `--cache`) ignore
the variable; that's fine.

### Per-service checks in monorepos

v0.1 walks up from the cwd to the first `.git` directory and uses the
`klasp.toml` at that root. **Multi-config monorepos (different `klasp.toml`
per package, scoped to the staged-file subtree) are a known gap and ship in
v0.2.5** — see [design.md §14](./design.md#14-open-questions--known-gaps) and
[roadmap.md §v0.2.5](./roadmap.md#v025--parallel--monorepo--ci-output-target-5-months-from-v01).

Until then, v0.1 monorepo strategies in order of preference:

1. **Single root config, narrow shell commands** — point each check at its
   subdirectory (`command = "pytest packages/api"`). Fastest to adopt.
2. **Single root config, diff-aware commands** — let
   `${KLASP_BASE_REF}`-aware tools (`pre-commit`, `fallow`) decide what to run
   based on the diff. Cleanest for large repos already using those tools.
3. Wait for v0.2.5 if neither fits.

### Verdict policies

The `[gate].policy` field controls how individual check outcomes are folded
into the gate's final decision.

**`any_fail` (default)** — blocks if at least one check returned `Fail`. Use
this for standard quality gates where a single red check is reason enough to
stop the agent. This is the v0.1 behaviour and the right choice for most
repos.

**`all_fail`** — blocks only when every non-`Warn` check returned `Fail` and
no check returned `Pass`. If a subset of checks fails but at least one passes,
the gate downgrades the result to `Warn` (the agent is informed but not
blocked). Good for experimental "canary" check sets where you want coverage
without hard-blocking the agent until the checks are proven reliable.

**`majority_fail`** — blocks when strictly more than half the non-`Warn`
checks returned `Fail`. Ties (e.g. 2 pass + 2 fail) are not a majority and
downgrade to `Warn`. Useful for weighted-consensus setups where several
independent linters vote and you want partial disagreement surfaced as a
warning rather than a block.

`Warn` verdicts are never counted in the decisive majority or unanimity test
regardless of policy — they pass through as informational findings.

```toml
[gate]
agents = ["claude_code"]
policy = "majority_fail"   # "any_fail" | "all_fail" | "majority_fail"
```

Unknown policy values fail at config-load time with a parse error; there is no
silent fallback.

### Fail-open semantics

If a check tool isn't installed, `klasp doctor` warns (`WARN  path[name]: not
found in PATH`) and the gate runtime logs `klasp-gate: check 'name' runtime
error (...), skipping.` to stderr without blocking the agent. Same story for
schema mismatches between an upgraded `klasp` binary and an old hook script,
parse errors on stdin, and missing `klasp.toml`. The gate degrades to no-op
rather than wedging the agent. Re-running `klasp install` re-syncs everything
and `klasp doctor` shows the actual state.

---

## pre-commit

Runs the [pre-commit](https://pre-commit.com/) framework against the diff,
exactly as you'd run it locally. Use the same flags pre-commit uses internally
when invoked from its own `pre-commit` git hook so the agent hits identical
gates to a human typing `git commit`.

### Typed recipe form (v0.2 W4) — preferred

```toml
[[checks]]
name = "pre-commit"
triggers = [{ on = ["commit"] }]
timeout_secs = 120
[checks.source]
type = "pre_commit"
# Optional. Defaults shown.
# hook_stage = "pre-commit"
# config_path = ".pre-commit-config.yaml"
```

The typed recipe builds the equivalent `pre-commit run --hook-stage <stage>
--from-ref ${KLASP_BASE_REF} --to-ref HEAD [-c <config_path>]` invocation
internally, then parses pre-commit's per-hook stdout into structured findings
the agent can act on (`hook \`ruff\` failed`, `hook \`mypy\` failed`) instead
of a single opaque "exit 1" message. Pre-commit 3.x and 4.x are both
supported; outside that range the recipe surfaces a stderr warning but keeps
running on the bet that pre-commit's stable stdout format stays stable.

`hook_stage` accepts any of pre-commit's documented stages
(`pre-commit`, `pre-push`, `commit-msg`, `pre-merge-commit`, …). `config_path`
is forwarded as `-c <path>`; omit it to let pre-commit's own discovery find
`.pre-commit-config.yaml` at the repo root.

> v0.1 shipped a `verdict_path` field on `CheckConfig` that the design briefly
> implied; it was deferred to a future milestone (see
> [`design.md` §14](./design.md#14-open-questions--known-gaps) for the
> deferral note). The typed recipes don't need it — each recipe owns its
> tool's output format.

### v0.1 shell form (still supported)

```toml
[[checks]]
name = "pre-commit"
triggers = [{ on = ["commit"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = "pre-commit run --hook-stage pre-commit --from-ref ${KLASP_BASE_REF} --to-ref HEAD"
```

The `--hook-stage pre-commit` flag is what scopes the run to commit-stage hooks
(skipping `pre-push`, `commit-msg`, etc.). The `--from-ref / --to-ref` pair
restricts the run to files changed since the merge-base — without this, every
agent commit re-lints the whole repo.

## fallow

[fallow](https://github.com/fallow-dev/fallow) is the diff-aware audit tool
klasp's gate is modeled on. The recipe runs `fallow audit --format json`
against the diff and parses the structured verdict into per-finding rows
the agent can act on.

### Typed recipe form (v0.2 W5) — preferred

```toml
[[checks]]
name = "fallow"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "fallow"
# Optional. Defaults shown.
# base = "${KLASP_BASE_REF}"
# config_path = ".fallowrc.json"
```

The recipe builds the equivalent
`fallow audit --format json --quiet --base <ref> [-c <config_path>]`
invocation internally, then maps fallow's top-level `verdict` field to a
klasp verdict (`pass` / `warn` / `fail`). Per-finding rows from
`complexity.findings[]`, `dead_code.unused_*[]`, and
`duplication.clone_groups[]` carry through with file + line locations so
the agent can navigate to the offending site. fallow 2.x is supported;
outside that range the recipe surfaces a stderr warning but keeps
running on the bet that fallow's stable JSON schema stays stable.

`base` defaults to `${KLASP_BASE_REF}` (the gate-resolved merge-base),
which is what most users want — set it explicitly only when the audit
diff-base needs to diverge from the gate's resolved base ref (e.g. a
long-lived release branch auditing against a fixed mainline).
`config_path` is forwarded as `-c <path>`; omit it to let fallow's own
discovery find `.fallowrc.json`, `.fallowrc.jsonc`, or `fallow.toml` at
the repo root.

### v0.1 shell form (still supported)

```toml
[[checks]]
name = "fallow"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "fallow audit --base ${KLASP_BASE_REF} --quiet --format json"
```

The shell form falls back on fallow's non-zero exit code as the block
signal — no per-finding parsing — and is still the right choice if you
need to chain fallow with other commands in the same shell line.

## pytest

Fast feedback on commit, full coverage on push. The two-trigger pattern keeps
the agent's commit cycle snappy while still gating push on the slow run.

### Typed recipe form (v0.2 W6) — preferred

```toml
[[checks]]
name = "pytest"
triggers = [{ on = ["commit"] }]
timeout_secs = 120
[checks.source]
type = "pytest"
# Optional. Defaults shown.
# extra_args = "-x -q tests/"
# config_path = "pytest.ini"   # forwarded as `pytest -c <path>`
# junit_xml = true             # write JUnit XML and parse for findings
```

The typed recipe builds the equivalent
`pytest [-c <config>] [--junitxml=<path>] [<extra_args>]` invocation
internally. With `junit_xml = true`, klasp asks pytest to emit a
JUnit XML report under `.klasp-pytest-junit.xml` at the repo root and
parses it for per-failure findings (`test \`tests.test_math::test_add\`
failed: assert (1 + 1) == 3`) with file + line locations. Without
`junit_xml`, the recipe falls back to a generic count-based finding
based on pytest's exit code alone.

Pytest's documented exit codes ride through:

| Exit | Meaning | Verdict |
|---|---|---|
| 0 | All tests passed | `Pass` |
| 1 | One or more tests failed | `Fail` (per-failure findings via JUnit, else generic) |
| 2 | Test run interrupted (`KeyboardInterrupt`) | `Fail` with `interrupted` detail |
| 3 | Internal pytest error | `Fail` with `internal error` detail |
| 4 | pytest CLI usage error | `Fail` with `usage error` detail |
| 5 | No tests collected | `Fail` with `no tests` detail |

pytest 7.x and 8.x are both supported; outside that range the recipe
surfaces a stderr warning but keeps running on the bet that pytest's
stable JUnit format stays stable.

### v0.1 shell form (still supported)

```toml
[[checks]]
name = "pytest"
triggers = [{ on = ["commit"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = "pytest -q"

[[checks]]
name = "pytest-coverage"
triggers = [{ on = ["push"] }]
timeout_secs = 600
[checks.source]
type = "shell"
command = "pytest --cov --cov-fail-under=80"
```

The shell form falls back on pytest's non-zero exit code as the block
signal — no per-failure parsing — and is still the right choice if you
need to chain pytest with other commands in the same shell line. `-q`
keeps pytest's output compact so the agent's stderr buffer doesn't
overflow on large suites. If you use
[pytest-xdist](https://pytest-xdist.readthedocs.io), add `-n auto` to
either command.

## cargo

The setup the klasp repo dogfoods — see [`/klasp.toml`](../klasp.toml).
Three checks split across triggers by cost.

### Typed recipe form (v0.2 W6) — preferred

```toml
[[checks]]
name = "cargo-check"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "cargo"
subcommand = "check"
# Optional. Defaults shown.
# extra_args = "--all-features"
# package = "klasp-core"   # `-p <pkg>`; if None, runs `--workspace`

[[checks]]
name = "cargo-clippy"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 180
[checks.source]
type = "cargo"
subcommand = "clippy"
extra_args = "-- -D warnings"

[[checks]]
name = "cargo-test"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "cargo"
subcommand = "test"
```

The typed recipe dispatches one of `cargo check` / `cargo clippy` /
`cargo test` / `cargo build` based on the `subcommand` field (other
values are rejected at run time with a list of accepted ones). For
`check` / `clippy` / `build`, klasp asks cargo for
`--message-format=json` and walks the `compiler-message` stream to
extract per-diagnostic findings with file + line locations and the
rustc / clippy lint code. For `cargo test`, klasp parses the trailing
`test result: <status>. N passed; M failed; …` line for a count-based
summary — per-test-name parsing is deferred to v0.2.5 when cargo's
JSON test reporter stabilises out of nightly.

`cargo check` is the cheapest sanity check (compilation only, no
codegen); it catches most class-of-bugs the agent introduces before
clippy even runs. Use `extra_args = "-- -D warnings"` on clippy to
ensure warnings are blocking (clippy's default exit code is 0 for
warnings). `cargo test` is push-only because test wall time is a
per-commit cost the agent shouldn't pay on every iteration.

### v0.1 shell form (still supported)

```toml
[[checks]]
name = "cargo-check"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "cargo check --all-targets --workspace"

[[checks]]
name = "cargo-clippy"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "cargo clippy --all-targets --workspace -- -D warnings"

[[checks]]
name = "cargo-test"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "shell"
command = "cargo test --workspace"
```

The shell form falls back on cargo's non-zero exit code as the block
signal — no per-diagnostic parsing — and is still the right choice if
you need to chain cargo with other commands in the same shell line.

## ESLint / Biome

[ESLint](https://eslint.org/) and [Biome](https://biomejs.dev/) both have a
`--no-error-on-unmatched-pattern` story for diff-aware runs. The simplest
shape — let the tool's cache do the diff scoping:

```toml
# ESLint
[[checks]]
name = "eslint"
triggers = [{ on = ["commit"] }]
timeout_secs = 90
[checks.source]
type = "shell"
command = "eslint --max-warnings 0 --cache ."

# Biome
[[checks]]
name = "biome"
triggers = [{ on = ["commit"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "biome check ."
```

For diff-only runs, pipe `git diff --name-only` through `xargs`:

```toml
command = "git diff --name-only --diff-filter=ACM ${KLASP_BASE_REF} | xargs -r eslint --max-warnings 0"
```

Biome already operates in milliseconds on full repos, so the diff-only form
is rarely worth the complexity.

## ruff

[ruff](https://docs.astral.sh/ruff/) is fast enough that whole-repo runs are
fine on every commit. Use `--no-fix` so the gate reports findings instead of
silently rewriting the agent's working tree:

```toml
[[checks]]
name = "ruff-lint"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "ruff check --no-fix ."

[[checks]]
name = "ruff-format"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "ruff format --check ."
```

Two checks instead of one keeps the lint-vs-format failure surfaces distinct
in the agent's block message, which materially helps the agent self-correct
without retrying the wrong fix.

---

## What's next

v0.2 introduced named recipes — typed `CheckSource` impls that hide the
verbose `command = "..."` line behind a `type = "<recipe>"` shorthand. The
v0.2 slate is now complete: `type = "pre_commit"` (W4), `type = "fallow"`
(W5), `type = "pytest"` (W6), and `type = "cargo"` (W6). See the
sections above for worked examples.

Existing v0.1 `type = "shell"` configs continue working unchanged (no schema
bump). See
[roadmap.md §v0.2](./roadmap.md#v02--codex--named-recipes-target-3-months-from-v01)
for the full plan and
[roadmap.md §v0.2.5](./roadmap.md#v025--parallel--monorepo--ci-output-target-5-months-from-v01)
for what comes next (parallel execution, JUnit per-test-name parsing for
`cargo test`).
