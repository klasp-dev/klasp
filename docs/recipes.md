# klasp recipes (v0.1)

Worked `klasp.toml` snippets for the most common check tools. Every snippet is
copy-pasteable into the `[[checks]]` section of your config; for the surrounding
shape, see [`design.md` §3.5](./design.md#35-configv1-versioned-config) or the
project's own dogfood config at [`/klasp.toml`](../klasp.toml).

> v0.1 ships exactly one check source: `type = "shell"`. v0.2 adds named
> recipes (`type = "pre_commit"`, `type = "fallow"`, `type = "pytest"`,
> `type = "cargo"`) so users can drop the verbose `command = "..."` lines —
> see [roadmap.md §v0.2](./roadmap.md#v02--codex--named-recipes-target-3-months-from-v01).

## Patterns

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
klasp's gate is modeled on. Run its audit JSON output against the diff:

```toml
[[checks]]
name = "fallow"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "fallow audit --base ${KLASP_BASE_REF} --quiet --format json"
```

> v0.1 surfaces fallow's findings only via its non-zero exit code and
> redirected stdout/stderr — klasp does not parse fallow's JSON. The v0.2
> named recipes (`type = "fallow"`, `type = "pytest"`) will own JSON-output
> parsing for tools that emit structured verdicts and render typed findings
> into the gate's block message. Until then, fall back on the check tool's
> exit code (any non-zero blocks).

## pytest

Fast feedback on commit, full coverage on push. The two-trigger pattern keeps
the agent's commit cycle snappy while still gating push on the slow run.

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

`-q` keeps pytest's output compact so the agent's stderr buffer doesn't
overflow on large suites. If you use [pytest-xdist](https://pytest-xdist.readthedocs.io),
add `-n auto` to either command.

## cargo

The setup the klasp repo dogfoods — see [`/klasp.toml`](../klasp.toml). Three
checks split across triggers by cost:

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

`cargo check` is the cheapest sanity check (compilation only, no codegen); it
catches most class-of-bugs the agent introduces before clippy even runs. Use
`-- -D warnings` on clippy to ensure warnings are blocking (clippy's default
exit code is 0 for warnings). `cargo test` is push-only because test wall
time is a per-commit cost the agent shouldn't pay on every iteration.

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

v0.2 introduces named recipes — typed `CheckSource` impls that hide the
verbose `command = "..."` line behind a `type = "<recipe>"` shorthand:

```toml
[[checks]]
name = "lint"
triggers = [{ on = ["commit"] }]
[checks.source]
type = "pre_commit"   # knows --hook-stage / --from-ref semantics

[[checks]]
name = "audit"
triggers = [{ on = ["commit", "push"] }]
[checks.source]
type = "fallow"       # parses fallow's JSON, surfaces structured findings
```

Same for `pytest` and `cargo`. Existing v0.1 `type = "shell"` configs continue
working unchanged (no schema bump). See
[roadmap.md §v0.2](./roadmap.md#v02--codex--named-recipes-target-3-months-from-v01)
for the full plan.
