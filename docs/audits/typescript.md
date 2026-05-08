# TypeScript audit recipe

A copy-pasteable klasp audit posture for TypeScript repos. Three tiers
(minimal / serious / strict) plus a dedicated section on the klasp ↔ fallow
relationship.

> Positioning: klasp does not lint, type-check, test, or audit your code.
> It runs the tools you already use (`tsc`, `eslint`, `biome`, `vitest`,
> `fallow`, …) at the agent's `git commit` / `git push` surface and turns
> their exit codes and structured output into a verdict the agent can
> self-correct against. **You bring the checks; klasp gates them.**

> Positioning vs fallow: **fallow is the TS audit engine; klasp is the
> agent gate that can run fallow and normalize the verdict.** They
> compose; you can run either alone, but the strict tier here uses both.

For the surrounding config shape (`version = 1` header, `[gate]` block,
`policy` semantics) see [`docs/recipes.md`](../recipes.md). Every TOML
snippet below is a fragment of a complete `klasp.toml` — drop it in
alongside your `[gate]` block.

## Target audience and repo shape

This recipe assumes one of:

- **Next.js / Remix / React app** in a single package or pnpm workspace.
- **Vite / React / SolidJS** SPA, single package.
- **Node API** (Nest, Fastify, Hono) with `tsc --noEmit` as the build
  truth source.
- **TypeScript library** published to npm with `vitest` or `jest` as
  the test runner.

It assumes pnpm by default — swap in `npm`, `yarn`, or `bun` per the
table below. Checks scope to the repo root unless you narrow them with
`cd packages/<pkg> && ...`. For multi-package monorepos, see
[`docs/audits/monorepo.md`](./monorepo.md).

## Required tools and install

| Tool                 | What it does                              | Tier         |
| -------------------- | ----------------------------------------- | ------------ |
| Node 20+ or Bun 1.1+ | Runtime                                   | Always       |
| pnpm / npm / yarn    | Lockfile + install                        | Always       |
| TypeScript 5.x       | `tsc --noEmit` typecheck                  | Minimal      |
| Biome 1.9+           | Formatter + linter (single binary)        | Minimal      |
| ESLint 9 + Prettier  | Formatter + linter (legacy combo)         | Minimal alt. |
| Vitest 2.x / Jest 29 | Unit / integration tests                  | Serious      |
| `fallow` 2.x         | Diff-aware audit (complexity, dead code, duplication) | Strict       |
| `knip` 5.x           | Dead exports / files / deps               | Strict alt.  |

```bash
# pnpm (preferred)
pnpm install --frozen-lockfile
pnpm add -D typescript @biomejs/biome vitest fallow-cli knip

# npm
npm ci
npm i -D typescript @biomejs/biome vitest fallow-cli knip

# yarn (berry / classic)
yarn install --immutable          # berry; --frozen-lockfile on classic
yarn add -D typescript @biomejs/biome vitest fallow-cli knip

# bun
bun install --frozen-lockfile
bun add -d typescript @biomejs/biome vitest fallow-cli knip
```

## Tier 1 — Minimal

Three checks: lockfile integrity, format + lint, type-check. The "first
hour on the repo" tier — enough to catch obvious breakage without
demanding new tools.

```toml
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

# Lockfile drift / install integrity. `--frozen-lockfile` exits non-zero
# if package.json and the lockfile disagree. Cheaper than running other
# checks against a wrong dependency tree.
[[checks]]
name = "install"
triggers = [{ on = ["commit"] }]
timeout_secs = 90
[checks.source]
type = "shell"
command = "pnpm install --frozen-lockfile --prefer-offline"

# Format + lint via Biome. Single binary, runs in milliseconds on full
# repos; the diff-only form is rarely worth the complexity here.
[[checks]]
name = "biome"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "biome check ."

# Type-check. `--noEmit` skips codegen; we only want diagnostics.
# `--incremental` reuses .tsbuildinfo so warm runs land in 1-3s on
# repos with thousands of files.
[[checks]]
name = "tsc"
triggers = [{ on = ["commit"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = "tsc --noEmit --incremental"
```

### ESLint + Prettier instead of Biome

```toml
[[checks]]
name = "prettier"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "prettier --check ."

[[checks]]
name = "eslint"
triggers = [{ on = ["commit"] }]
timeout_secs = 90
[checks.source]
type = "shell"
command = "eslint --max-warnings 0 --cache --cache-location .eslintcache ."
```

`--cache` is load-bearing: ESLint without it re-parses every file on
every commit and adds 5-30s on a mid-sized repo. Cache lives at
`.eslintcache` — gitignore it.

## Tier 2 — Serious

Adds tests, dependency audit, stricter lint configuration. Push-only
checks live here — commit cycle stays fast, push catches what fast
checks missed.

```toml
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

# --- commit (fast feedback) ---------------------------------------------

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

# Vitest in run mode. `--reporter=verbose` writes one line per test so
# the agent can identify the failing test name. `--passWithNoTests` is
# intentionally omitted — empty runs on a serious-tier repo are usually
# a config bug worth surfacing.
[[checks]]
name = "vitest"
triggers = [{ on = ["commit"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "vitest run --reporter=verbose"

# --- push (slower, more thorough) ---------------------------------------

# `--audit-level=high` blocks on high+ severity only. Tighten to
# `moderate` if your team has the appetite.
[[checks]]
name = "audit"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "pnpm audit --audit-level=high --prod"

# Coverage on push only. Tune the threshold to your team's bar; a low
# floor is better than no floor.
[[checks]]
name = "vitest-coverage"
triggers = [{ on = ["push"] }]
timeout_secs = 600
[checks.source]
type = "shell"
command = "vitest run --coverage --coverage.thresholds.lines=80"
```

### Jest variant

```toml
[[checks]]
name = "jest"
triggers = [{ on = ["commit"] }]
timeout_secs = 240
[checks.source]
type = "shell"
command = "jest --ci --reporters=default --reporters=jest-junit"
```

`jest-junit` writes `junit.xml` to the repo root (gitignore it).
klasp's pytest typed recipe parses JUnit; the TS path uses Jest's
exit code only today.

### Stricter lint rules

For a serious tier, enable the full Biome `style` and `nursery` groups
in `biome.json`:

```jsonc
{
  "linter": {
    "rules": {
      "recommended": true,
      "style": { "noNonNullAssertion": "error", "useImportType": "error" },
      "suspicious": { "noExplicitAny": "error" }
    }
  }
}
```

For ESLint, add `@typescript-eslint/recommended-type-checked` and
`eslint-plugin-import` with `import/no-cycle` enabled. These are slow
enough that they belong on push if `--cache` doesn't keep the commit
run under ~5s.

## Tier 3 — Strict

Adds dead-code / dependency drift detection and the fallow audit. This
tier assumes the team has the appetite to fix what strict tools surface
— see [escape hatches](#common-false-positives-and-escape-hatches) for
the inevitable suppression workflow.

```toml
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

# --- commit -------------------------------------------------------------

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
triggers = [{ on = ["commit"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "vitest run --reporter=verbose"

# fallow as a typed recipe. `base` defaults to ${KLASP_BASE_REF}, which
# is what most repos want. See the fallow integration section below.
[[checks]]
name = "fallow"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 90
[checks.source]
type = "fallow"

# --- push ---------------------------------------------------------------

[[checks]]
name = "audit"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "pnpm audit --audit-level=high --prod"

[[checks]]
name = "vitest-coverage"
triggers = [{ on = ["push"] }]
timeout_secs = 600
[checks.source]
type = "shell"
command = "vitest run --coverage --coverage.thresholds.lines=80"

# Dead exports, unused files, unused deps, undeclared deps. Most
# comprehensive of (knip / ts-prune / depcheck) but also noisiest on
# first adoption — start with `policy = "all_fail"` until baseline is
# clean if necessary.
[[checks]]
name = "knip"
triggers = [{ on = ["push"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = "knip --no-progress --reporter compact"
```

### Lighter alternatives to knip

```toml
# Dead exports only — fastest, narrowest.
[[checks]]
name = "ts-prune"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "ts-prune --error"

# Unused / undeclared deps only — orthogonal to dead exports.
[[checks]]
name = "depcheck"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "depcheck --skip-missing=false"
```

## fallow integration

[fallow](https://github.com/fallow-dev/fallow) is the TS audit engine
this recipe is built around at the strict tier:

- **fallow** runs a diff-aware audit of TS / JS code: cyclomatic
  complexity, dead code, duplication, and `any`-type drift, scoped to
  changed files. Returns a structured JSON verdict.
- **klasp** is the agent gate. It invokes fallow at the right trigger
  (`commit` for incremental audits, `push` for full passes), normalises
  fallow's verdict to klasp's `pass` / `warn` / `fail` shape, and
  surfaces per-finding rows with file + line locations the agent can
  navigate to.

You can run fallow standalone (it's a fine CLI). You can run klasp
without fallow (the minimal and serious tiers don't use it). The
interesting combination is klasp invoking fallow: fallow's audit
quality plus klasp's `PreToolUse` blocking before the agent's commit
ever lands.

### When to use fallow vs alternatives

| Need                                | Tool                          |
| ----------------------------------- | ----------------------------- |
| Cyclomatic complexity, file churn   | fallow                        |
| Dead exports                        | fallow / ts-prune / knip      |
| Unused files / deps                 | fallow / knip                 |
| Code duplication                    | fallow                        |
| `any`-type drift over time          | fallow (best fit)             |
| Single tool, broadest coverage      | knip                          |
| Fastest commit-time signal          | ts-prune (dead exports only)  |

fallow is the broadest single tool and the only one with first-class
diff-awareness via `--base`. If you've already standardised on knip,
keep knip — don't add fallow just to have both. They overlap on
dead-code detection.

### Recommended invocation (typed recipe)

```toml
[[checks]]
name = "fallow"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 90
[checks.source]
type = "fallow"
# Optional. Defaults shown.
# base = "${KLASP_BASE_REF}"
# config_path = ".fallowrc.json"
```

The typed recipe builds the equivalent
`fallow audit --format json --quiet --base <ref> [-c <config>]`
internally and parses fallow's `verdict`, `complexity.findings`,
`dead_code.unused_*`, and `duplication.clone_groups` sections into
per-finding rows. fallow 2.x is supported.

`base` defaults to `${KLASP_BASE_REF}` (the gate-resolved merge-base
of `HEAD` against the upstream tracking branch). Override only when
the audit base needs to diverge from the gate's base — e.g. a
long-lived release branch auditing against a fixed mainline.

### Shell form (still supported)

```toml
[[checks]]
name = "fallow"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 90
[checks.source]
type = "shell"
command = "fallow audit --base ${KLASP_BASE_REF} --quiet --format json"
```

The shell form falls back on fallow's exit code as the block signal —
no per-finding parsing — and is right when you need to chain fallow
with other commands on the same shell line.

### How klasp normalises the verdict

| fallow         | klasp   | Agent sees                                |
| -------------- | ------- | ----------------------------------------- |
| `pass`         | `Pass`  | Nothing; check passes silently             |
| `warn`         | `Warn`  | Findings surfaced, gate not blocked        |
| `fail`         | `Fail`  | Findings surfaced, gate blocks the commit  |

Per-finding rows come through with `file`, `line`, and `detail`
populated, which is what makes the agent's self-correction loop
tractable: "complexity finding in `src/foo.ts:42`" is actionable in
a way "fallow exited 1" is not.

## Commit vs push split

The default split for TS repos:

| Trigger | Checks                                                                       |
| ------- | ---------------------------------------------------------------------------- |
| commit  | install integrity, biome / eslint, `tsc --noEmit`, vitest (no coverage), fallow (diff) |
| push    | dependency audit, vitest with coverage, knip / ts-prune / depcheck           |

Principle: **anything the agent can fix in under 30 seconds belongs on
commit.** Type errors, lint violations, fallow findings on the diff —
all fast enough on a warm cache that the agent's retry loop converges.
Slow stuff (full-repo coverage, dead-code sweeps, audit) goes on push
because the agent has already iterated to a plausibly-correct state by
then; push-time blocks catch the regressions fast checks missed.

## Hard-block vs warning policy

`[gate].policy = "any_fail"` (the default) blocks on any single failed
check. For greenfield repos that's the right starting point.

For legacy repos with thousands of `any`-types, dead exports the team
hasn't triaged, or a pre-fallow codebase, hard-blocking on day one
strands the agent. Three patterns to soften the landing:

### Pattern 1 — Warn-only via `policy = "all_fail"`

```toml
[gate]
agents = ["claude_code"]
policy = "all_fail"
```

`all_fail` only blocks when every check fails. With `tsc` passing,
fallow can warn-without-blocking until the baseline is clean. Switch
back to `any_fail` once noisy checks land green for a week.

### Pattern 2 — Per-check baselining

Most TS tools support a baseline:

```bash
biome check . --write-suppressions   # generate biome.json suppressions
eslint . --fix-type suggestion --fix # auto-fix what's mechanical
ts-prune --ignore "src/legacy/**"    # exclude a path
```

For `tsc` baselining on a repo with too-many `any`-types, the canonical
move is `tsconfig.strict.json` — strict config used for new code only,
applied via path-restricted `include` arrays. Main `tsconfig.json` keeps
`strict: false` until migration is complete.

### Pattern 3 — Fail-only-on-net-new

`fallow audit --base ${KLASP_BASE_REF}` is already diff-scoped: existing
findings outside the diff are not surfaced. The right behaviour for a
legacy repo — the agent is only blocked on regressions it introduced,
not on debt it inherited.

For other tools, wrap with `git diff` filtering:

```toml
[[checks]]
name = "eslint-diff-only"
triggers = [{ on = ["commit"] }]
timeout_secs = 90
[checks.source]
type = "shell"
command = "git diff --name-only --diff-filter=ACM ${KLASP_BASE_REF} | grep -E '\\.(ts|tsx)$' | xargs -r eslint --max-warnings 0"
```

## `KLASP_BASE_REF` and incremental performance

Every shell check sees `KLASP_BASE_REF` in its environment. Use it for
any tool with a diff base:

| Tool      | Diff-aware flag                                        |
| --------- | ------------------------------------------------------ |
| fallow    | `--base ${KLASP_BASE_REF}` (typed recipe handles this)  |
| eslint    | Pipe `git diff --name-only ${KLASP_BASE_REF}` to xargs  |
| biome     | `biome check --staged` (uses git index)                 |
| tsc       | No diff base; rely on `--incremental` and `.tsbuildinfo` |
| vitest    | `vitest run --changed ${KLASP_BASE_REF}` (Vitest 1.6+)  |
| knip      | No diff base; full-repo only — keep on push              |

`tsc --incremental` writes `.tsbuildinfo` to the project root (or
`outDir` if set). Gitignore it. Warm cache: 1-3s on repos with
thousands of files. Cold cache: full time. Cache is per-checkout —
fresh `git clone` runs are slow.

`eslint --cache --cache-location .eslintcache` is similar.

### Forward-looking: tsgo

Microsoft's Go-based `tsc` rewrite (`tsgo`) is in 2026 preview with
~10x cold-build speedups on large monorepos. Not yet production-stable
— feature parity gaps exist around `--build` mode, project references,
and edge cases in module resolution. When tsgo stabilises:

```toml
command = "tsgo --noEmit --incremental"
```

Drop-in replacement, same diagnostics format, same exit codes.

## Expected findings

What the agent sees when each check fails. Real output, lightly edited.

### `tsc --noEmit`

```
src/api/user.ts:42:7 - error TS2322: Type 'string | undefined' is not assignable to type 'string'.

42       const id: string = req.params.id;
         ~~

Found 1 error in src/api/user.ts:42
```

File, line, column, error code (`TS2322`), narrative. Cleanest auto-
correction surface in the TS toolchain — agents land `tsc` fixes
faster than any other class of finding.

### Biome

```
src/utils/format.ts:18:3 lint/suspicious/noExplicitAny  ✖
  × Unexpected any. Specify a different type.
  > 18 │     const obj = value as any;
       │                          ^^^
```

### ESLint

```
/repo/src/components/Card.tsx
  17:6  error  React Hook "useEffect" has a missing dependency: 'props.id'  react-hooks/exhaustive-deps
✖ 1 problem (1 error, 0 warnings)
```

### Vitest

```
 FAIL  src/api/user.test.ts > getUser > returns a user
AssertionError: expected null to deeply equal { id: 'u1', name: 'Alice' }
 ❯ src/api/user.test.ts:14:5
```

### fallow

The typed recipe parses fallow's JSON verdict:

```
[fallow] complexity:src/api/handler.ts:142  cyclomatic complexity 18 exceeds threshold 12
[fallow] dead-code:src/utils/legacy.ts:7    unused export `formatLegacy`
[fallow] duplication:src/api/foo.ts:30..58  clone group with src/api/bar.ts:22..50 (95% similarity)
```

## Common false positives and escape hatches

Every strict tool generates noise. The TS escape-hatch ladder, in order
of reach:

### `tsc`

```ts
// @ts-expect-error — Foo<T> narrowing intentional, fix in #1234
const x: Foo<string> = legacy as unknown as Foo<string>;
```

`@ts-expect-error` is preferred over `@ts-ignore` because it errors if
the suppression becomes unnecessary — the team's notified when the bug
is actually fixed upstream.

### ESLint

```ts
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const value: any = libraryReturnsUntyped();
```

Always prefer `disable-next-line` over `disable-line` and over
file-level `eslint-disable`. Always include the rule name — bare
`disable` masks every rule and creates a long-tail review burden.

### Biome

```ts
// biome-ignore lint/suspicious/noExplicitAny: external API returns unknown shape
const value: any = legacyApi.fetch();
```

Biome's ignore directives require the rule path
(`lint/<group>/<rule>`) and a justification after the colon. The
justification requirement is deliberate — Biome refuses ignores
without one.

### fallow

fallow respects `// fallow-ignore-next-line: <reason>` for complexity
and dead-code findings, plus `clone` directives in `.fallowrc.json` for
duplication exclusions.

### `--no-verify` is closed

The whole point of klasp: `git commit --no-verify` doesn't bypass the
`PreToolUse` hook. If your team needs an actual emergency override,
gate it on a marker file the agent can't create on its own — see the
protected-path guard pattern in
[`README.md`](../../README.md#4-protected-path-guards).

## Graduation to plugin

This recipe is a candidate for `klasp-plugin-typescript`. The graduation
trigger is when enough of the shell glue above repeats across enough
real klasp users that wrapping it in a single typed recipe becomes
clearly worth the maintenance cost.

A first-party `klasp-plugin-typescript` would:

- Detect package manager from the lockfile (`pnpm-lock.yaml`,
  `package-lock.json`, `yarn.lock`, `bun.lockb`).
- Discover formatter / linter (Biome vs ESLint+Prettier) from config.
- Run `tsc --noEmit` with `.tsbuildinfo` cache management.
- Optionally chain into the `fallow` typed recipe.
- Parse `tsc`, ESLint, Biome, and Vitest output into klasp findings
  with file + line locations.
- Emit a single structured verdict the agent acts against.

Until that plugin lands, this recipe is the canonical reference. If
you adopt it on a real repo, file the rough edges as issues against
[klasp-dev/klasp](https://github.com/klasp-dev/klasp/issues) — that's
the demand signal the plugin's prioritisation runs on.

## See also

- [`docs/audits/README.md`](./README.md) — audit recipe philosophy.
- [`docs/recipes.md`](../recipes.md) — full per-tool recipe reference.
- [`klasp.toml`](../../klasp.toml) — klasp's own dogfood config.
- [fallow](https://github.com/fallow-dev/fallow) — the TS audit engine
  this recipe wraps.
