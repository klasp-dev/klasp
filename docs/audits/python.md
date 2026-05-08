# Python audit recipe

A copy-pasteable starting point for gating an AI agent on a Python repo with
klasp. Tiered — pick the smallest one that covers your quality bar today,
graduate when the agent and the team stop hitting false positives.

> klasp is the agent gate and verdict normalizer. It does not replace ruff,
> pytest, mypy, pyright, deptry, vulture, bandit, pip-audit, or radon — it
> wraps them, gives the agent a structured "blocked, here's why" reply, and
> stops the agent from working around a red gate with `--no-verify`. The
> checks below are tools you bring; klasp is the glue that makes them
> agent-safe.

## Target repo shape

Assumes one of:

- A single-package Django, Flask, or FastAPI app, Poetry- or uv-managed,
  source under `src/<package>/` or a flat `<package>/` layout.
- A `src/`-layout app plus a small number of internal libraries under a
  single `pyproject.toml` (uv workspace, Poetry single-project, or hatch).
- A pure library with `pyproject.toml` + `tests/`.

Multi-package monorepos (apps + services + shared libs in one repo) are
covered by [`monorepo.md`](./monorepo.md). klasp walks up from the staged
file to find the nearest `klasp.toml`, so per-package configs compose cleanly.

## Required tools

Pick one package manager. The recipe is identical past install — checks call
the tool directly (not `poetry run` / `uv run`) because the agent's shell
already has the venv on `PATH` once `klasp install` has run.

```bash
# uv (recommended for new repos — fast, lockfile-first)
uv add --dev ruff pytest mypy deptry vulture bandit pip-audit radon

# Poetry
poetry add --group dev ruff pytest mypy deptry vulture bandit pip-audit radon

# pip / pip-tools
pip install ruff pytest mypy deptry vulture bandit pip-audit radon
```

Pyright is the alternative to mypy (`pip install pyright` or `npm i -g
pyright`). Pick one — running both double-bills the agent on the same error.

> Cache hints. `ruff` writes `.ruff_cache/`, `pytest` writes `.pytest_cache/`,
> `mypy` writes `.mypy_cache/` (materially faster on the second run). Add
> all three to `.gitignore` and leave them on disk between runs.

## Tier 1: Minimal

Smallest configuration that catches the bugs the agent is most likely to
introduce: bad formatting, undefined names, unused imports, broken tests.

```toml
# klasp.toml — minimal Python tier
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

# Format check on every commit. ruff format is whole-repo fast (<1s).
[[checks]]
name = "ruff-format"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "ruff format --check ."

# Lint on every commit. --no-fix keeps ruff from rewriting the working tree
# under the agent — the gate reports findings instead.
[[checks]]
name = "ruff-lint"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "ruff check --no-fix ."

# Tests on push. The pytest typed recipe parses pytest's exit code into a
# klasp verdict; with junit_xml = true it emits per-failure findings. Keep
# pytest off the commit trigger when the suite gets above ~5 seconds; the
# agent feels every second on retry loops.
[[checks]]
name = "pytest"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "pytest"
extra_args = "-q"
junit_xml = true
```

Two ruff checks instead of one keeps format-vs-lint failure surfaces distinct
in the agent's block message: a format failure is fixed by `ruff format .`,
a lint failure needs the agent to actually read the violation. Merging them
confuses both loops.

## Tier 2: Serious

Adds type-checking, dependency hygiene, and dead-code detection. Appropriate
once Tier 1 is green and you want to catch mistyped function signatures,
unused dependencies in `pyproject.toml`, and abandoned modules.

```toml
# klasp.toml — serious Python tier
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

# ── Tier 1 carries through unchanged ──────────────────────────────────────
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

# ── New in Tier 2 ─────────────────────────────────────────────────────────

# mypy on commit. --no-incremental is wrong for the dev loop — leave the
# .mypy_cache/ in place and mypy reuses it. Keep this on commit only when
# mypy completes in <5s on the warm cache; otherwise demote to push.
[[checks]]
name = "mypy"
triggers = [{ on = ["commit"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "mypy src/"

# Pyright alternative — pick one of mypy or pyright, not both.
# [[checks]]
# name = "pyright"
# triggers = [{ on = ["commit"] }]
# timeout_secs = 60
# [checks.source]
# type = "shell"
# command = "pyright --outputjson src/ | jq -e '.summary.errorCount == 0'"

# pytest on push, with JUnit XML so the agent sees per-failure findings
# (test name, file, line, assertion message) instead of "exit 1".
[[checks]]
name = "pytest"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "pytest"
extra_args = "-q"
junit_xml = true

# deptry on push: catches missing-from-pyproject and unused dependencies.
# Slow first run, fast on subsequent runs — leave on push to avoid the cost
# on every commit.
[[checks]]
name = "deptry"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "deptry ."

# vulture on push: dead-code detection. False-positive prone on Django /
# FastAPI / pytest fixtures — start at min-confidence 90 and tune downward
# only after the noise floor is calibrated.
[[checks]]
name = "vulture"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "vulture --min-confidence 90 src/"
```

### Pre-commit alternative

If the repo already uses the pre-commit framework for human commits, route
klasp through `type = "pre_commit"` instead. The typed recipe parses
pre-commit's per-hook stdout into structured findings (`hook 'ruff' failed`,
`hook 'mypy' failed`) instead of a single opaque exit code:

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

A representative `.pre-commit-config.yaml` for this tier:

```yaml
repos:
  - repo: https://github.com/astral-sh/ruff-pre-commit
    rev: v0.7.0
    hooks:
      - id: ruff-format
      - id: ruff
        args: [--no-fix]
  - repo: https://github.com/pre-commit/mirrors-mypy
    rev: v1.13.0
    hooks:
      - id: mypy
        files: ^src/
```

Use either the typed `pre_commit` form or the per-tool shell form, not
both — running ruff via pre-commit and again via a `ruff-lint` shell check
double-bills the same findings.

## Tier 3: Strict

Adds security, dependency CVEs, coverage thresholds, and complexity ceilings.
Appropriate for production-ship repos, compliance-bound codebases, or any
project where a regressed bandit hit is a release-blocker.

```toml
# klasp.toml — strict Python tier
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

# ── Tiers 1 + 2 carry through unchanged ───────────────────────────────────
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
name = "deptry"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "deptry ."

[[checks]]
name = "vulture"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "vulture --min-confidence 90 src/"

# ── New in Tier 3 ─────────────────────────────────────────────────────────

# pytest with coverage threshold on push. --cov-fail-under makes pytest exit
# non-zero (and klasp Fail) if coverage drops below the floor. Keep junit_xml
# for per-failure findings.
[[checks]]
name = "pytest-coverage"
triggers = [{ on = ["push"] }]
timeout_secs = 600
[checks.source]
type = "pytest"
extra_args = "-q --cov=src --cov-fail-under=80"
junit_xml = true

# bandit on commit: SAST for common Python security antipatterns
# (hardcoded passwords, shell=True, weak crypto). -ll skips low-severity.
# Configure exclusions via .bandit / pyproject.toml [tool.bandit] rather
# than per-line `# nosec` so the policy lives with the code, not in
# scattered comments.
[[checks]]
name = "bandit"
triggers = [{ on = ["commit"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "bandit -r src/ -ll -q"

# pip-audit on push: scans the resolved dependency tree against the PyPI
# advisory database. Slow on cold cache (~30s); fine on push, painful on
# commit. --strict makes warnings blocking.
[[checks]]
name = "pip-audit"
triggers = [{ on = ["push"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = "pip-audit --strict"

# radon cyclomatic-complexity ceiling on commit. -n B fails on grade B or
# worse (CC 11+). Tune the grade letter to match the repo's existing
# debt baseline — see the baselining section below.
[[checks]]
name = "radon-cc"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "radon cc src/ -n B -s --total-average"
```

## Commit vs push split

The split below is what each tier above ships. Principle: commit checks must
fit inside the agent's retry budget (a few seconds); push checks can take
longer because the agent has already committed.

| Check         | Tier   | Commit | Push | Why |
|---------------|--------|--------|------|-----|
| ruff format   | 1+2+3  | yes    |      | <1s, fixes are mechanical, agent retries are cheap. |
| ruff check    | 1+2+3  | yes    |      | <1s. Different findings from format — separate check. |
| mypy          | 2+3    | yes    |      | Cached run is fast. Demote to push if cold-cache exceeds budget. |
| pytest        | 1+2    |        | yes  | Even a fast suite is too slow for the commit retry loop. |
| pytest+cov    | 3      |        | yes  | Coverage threshold reads stale until tests actually run. |
| deptry        | 2+3    |        | yes  | First run is slow; doesn't change per-commit. |
| vulture       | 2+3    |        | yes  | Same as deptry — repo-shape, not per-edit. |
| bandit        | 3      | yes    |      | <2s on most repos, security regressions should block early. |
| pip-audit     | 3      |        | yes  | Network call, ~30s. Push is the right gate. |
| radon         | 3      | yes    |      | Per-function metric, fast, matches the agent's iteration loop. |

If a check's commit-time cost exceeds the agent's patience (~5 seconds of no
feedback before re-evaluation), demote it to push.

## Hard-block vs warn-only

Verdict tier is decided by exit code: non-zero from a shell check produces a
`Fail`, which under the default `policy = "any_fail"` blocks the agent.
There is no per-check `fail_on` field — policy is gate-wide. Three real
patterns for "informational, not blocking":

### 1. Wrap the command in `|| true`

```toml
[[checks]]
name = "vulture-warn-only"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "vulture --min-confidence 90 src/ || true"
```

The check always exits 0, so klasp records `Pass` and the agent never sees
the finding. Useful for one-shot audits you read in CI logs; for agent-facing
warnings, use option 2.

### 2. Switch the gate policy

```toml
[gate]
agents = ["claude_code"]
policy = "majority_fail"   # or "all_fail"
```

Under `majority_fail`, the gate only blocks when more than half the checks
return `Fail`. A single experimental check failing alongside three healthy
ones downgrades to `Warn` — the agent sees the finding but isn't blocked.
Cleanest "canary" mode for a check you don't yet trust. `all_fail` is
stricter: blocks only when every check failed, useful when independent
linters should outvote single-tool false positives.

### 3. Move the check to a less-aggressive trigger

A push-only check still blocks the push, but the agent has already committed
and can iterate on the warning before re-pushing. For agent-safe rollout:

1. Land on `push` with `policy = "majority_fail"`. Watch for a week.
2. Promote to `any_fail` once the noise floor is calibrated.
3. Optionally promote to `commit` for fast feedback.

### Baselining a legacy repo

A repo with 200 existing ruff violations cannot turn on `ruff check` without
blocking every commit. The pragmatic path:

- ruff: `ruff check --add-noqa .`, commit the `# noqa` comments, then add
  the check. New violations blocked; old ones grandfathered.
- mypy: `[mypy] strict = false`, then ratchet per-module
  `[mypy-yourpkg.module] strict = true` as each module is cleaned.
- pytest coverage: pin `--cov-fail-under` to the *current* coverage rounded
  down. Ratchet upward as tests land. Never set a floor the repo doesn't meet.
- bandit: `[tool.bandit] skips = [...]` for existing exception classes,
  tighten one rule at a time.

A green gate with pinned noise is better than a red gate the team learns to
ignore.

## KLASP_BASE_REF and runtime budget

Every shell check sees `KLASP_BASE_REF`, set by klasp to the merge-base of
`HEAD` against the upstream tracking branch (falling back to `HEAD~1` with no
upstream). Use it to scope diff-aware tools to changed files:

```toml
# Run ruff against changed Python files only.
[[checks]]
name = "ruff-lint-diff"
triggers = [{ on = ["commit"] }]
timeout_secs = 15
[checks.source]
type = "shell"
command = '''
  files=$(git diff --name-only --diff-filter=ACM "$KLASP_BASE_REF" -- '*.py')
  if [ -z "$files" ]; then exit 0; fi
  echo "$files" | xargs ruff check --no-fix
'''

# mypy against changed packages only.
[[checks]]
name = "mypy-diff"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = '''
  pkgs=$(git diff --name-only --diff-filter=ACM "$KLASP_BASE_REF" -- 'src/**/*.py' \
    | xargs -n1 dirname | sort -u)
  if [ -z "$pkgs" ]; then exit 0; fi
  mypy $pkgs
'''
```

Tools that don't take a base ref (pytest, pip-audit, deptry, bandit) ignore
the variable. ruff is fast enough that diff-scoping rarely earns its
complexity — start with whole-repo `ruff check .` and only switch if wall
time becomes a problem.

> Caches matter more than diff-scoping at this scale. `.mypy_cache/` and
> `.pytest_cache/` give you most of the speedup with none of the shell
> plumbing. Add them to `.gitignore`, leave them on disk, let the tool
> decide what to re-run.

## Expected agent-visible output

When ruff, pytest, or mypy report errors under klasp, the agent receives a
structured block message rather than raw stderr.

### ruff failure

```
gate: any_fail blocked the agent.
check 'ruff-lint' failed:
  src/handlers/auth.py:42:5  F401  'os' imported but unused
  src/handlers/auth.py:118:9 E501  line too long (102 > 88 characters)
  src/services/billing.py:7:1  F811  redefinition of unused 'logger'

3 findings. Run `ruff check --fix .` to auto-fix F401 and F811, then re-stage.
```

The agent sees rule codes, file:line locations, and a fix path. The next
tool call is usually `ruff check --fix .` followed by a re-attempted commit.

### pytest failure (with `junit_xml = true`)

```
gate: any_fail blocked the agent.
check 'pytest' failed:
  test 'tests.test_billing::test_refund_partial' failed:
    tests/test_billing.py:56  AssertionError: assert refund.amount == 25.0
  test 'tests.test_billing::test_refund_full' failed:
    tests/test_billing.py:71  AssertionError: assert refund.status == 'complete'

2 of 47 tests failed.
```

Without `junit_xml = true` the recipe falls back to a generic count-based
finding (`pytest exit 1: 2 failed, 45 passed`) and the agent has to re-run
pytest itself to see which tests are red. Always set `junit_xml = true` for
agent-facing gates.

### mypy failure

```
gate: any_fail blocked the agent.
check 'mypy' failed:
  src/services/billing.py:88: error: Incompatible return value type (got "Decimal", expected "int")  [return-value]
  src/handlers/auth.py:142: error: Argument 1 to "verify_token" has incompatible type "str | None"; expected "str"  [arg-type]

2 findings.
```

Mypy's error codes (`return-value`, `arg-type`) are the agent's hook for
`# type: ignore[code]` on genuine false positives — see below.

## False positives and escape hatches

| Tool   | Comment escape          | Config-file escape                                        |
|--------|--------------------------|-----------------------------------------------------------|
| ruff   | `# noqa: E501`           | `[tool.ruff.lint] ignore = ["E501"]` in `pyproject.toml`  |
| mypy   | `# type: ignore[arg-type]` | `[mypy-yourpkg.legacy] ignore_errors = true`              |
| pyright| `# pyright: ignore[reportGeneralTypeIssues]` | `pyrightconfig.json` `"ignore": [...]`  |
| pytest | `@pytest.mark.xfail(reason=...)` | `[tool.pytest.ini_options] xfail_strict = true`     |
| bandit | `# nosec B101`           | `[tool.bandit] skips = ["B101"]` in `pyproject.toml`      |
| vulture| `# noqa` is *not* honoured | `whitelist.py` allow-list, or `--ignore-names`           |
| deptry | n/a                      | `[tool.deptry.per_rule_ignores]` in `pyproject.toml`      |
| radon  | n/a                      | `radon cc -e 'src/legacy/*.py'` to exclude paths          |

Rules of thumb:

- Comment-level escapes are local and reviewable in the diff. Prefer them
  for one-off cases.
- Config-file escapes are global and easy to drift past review. Use them for
  whole-module legacy carve-outs and policy decisions; never let the agent
  add a project-wide `ignore = [...]` for a single hit.

If false-positive rate is genuinely too high, demote the check from `commit`
to `push` or move the gate from `any_fail` to `majority_fail` — don't
silence the rule across the codebase.

## Custom triggers

If your commit/push flow uses something other than `git commit` / `git push`
(jj, sapling, a wrapper script), declare a `[[trigger]]` so klasp fires on
your actual command:

```toml
# Fire on `jj git push` for any agent.
[[trigger]]
name = "jj-push"
pattern = "^jj\\s+git\\s+push"

# Fire on the team's wrapper.
[[trigger]]
name = "ship-script"
commands = ["./scripts/ship", "./scripts/ship --skip-ci"]
```

Built-in commit/push detection still fires for raw `git commit` / `git push`
— user triggers extend the regex, they don't replace it.

## Graduation to a plugin

This recipe is shell glue. Signals it should graduate into a first-party
`klasp-plugin-python`:

- Three or more repos in the org running the same Tier 2 or Tier 3 config
  with the same shell wrappers.
- `|| true` and `xargs` plumbing for diff-scoping copy-pasted unchanged.
- pytest JUnit XML is the only structured output the agent gets; ruff, mypy,
  and bandit still surface as single-line "exit 1" verdicts.

The plugin would wrap all eight tools behind one `klasp-plugin-python` binary
on `$PATH`, invoked as `[checks.source] type = "plugin", name = "python"`,
and parse each tool's structured output (ruff JSON, mypy
`--show-error-codes`, bandit JSON, pytest JUnit) into klasp `Finding` rows.
Per-tool `[[checks]]` blocks collapse into one tier:
`settings = { tier = "serious", coverage_min = 80, bandit_severity = "low" }`.

Tracked in [`klasp-dev/klasp#96`](https://github.com/klasp-dev/klasp/issues/96)
follow-ups. Until that lands, this recipe is the canonical Python setup —
copy the tier you need, tune the trigger split, ship the gate.
