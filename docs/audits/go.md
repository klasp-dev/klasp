# Audit recipe: Go

A serious posture for a Go repo gated by klasp. klasp runs the toolchain
you already use (`go test`, `go vet`, `staticcheck`, `golangci-lint`,
`govulncheck`) and normalizes the verdict the agent sees; the checks stay
yours. Targets Go 1.23+ — generics everywhere, `go test -fuzz` stable,
`go.work` routine.

Cross-recipe shape: [`../recipes.md`](../recipes.md). Schema:
[`../design.md` §3.5](../design.md#35-configv1-versioned-config). Dogfood:
[`/klasp.toml`](../../klasp.toml).

## Repo shapes this recipe targets

| Shape | Layout | Notes |
|---|---|---|
| **Single-module CLI** | `main.go`, `cmd/`, `internal/`, single `go.mod` | Most common. The recipe applies as written. |
| **Service with internal/ layout** | `cmd/<svc>/main.go`, `internal/<pkg>/...`, `pkg/<pub>/...` | Same single `go.mod`. Triggers fire on either subtree. |
| **Multi-module monorepo** | `go.work` at root, several `go.mod` siblings | Use a root `klasp.toml` for cross-module checks (`govulncheck`) plus a per-module `klasp.toml` where the per-module test/lint scope matters. See "Module-aware notes" below. |

## Required tools

The toolchain ships `go vet`, `go test`, `gofmt`. Third-party analyzers
install via `go install`:

```sh
go version  # 1.23+

go install golang.org/x/vuln/cmd/govulncheck@latest
go install honnef.co/go/tools/cmd/staticcheck@latest
go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest
go install go.uber.org/nilaway/cmd/nilaway@latest  # strict tier only
```

`klasp doctor` warns `WARN path[name]: not found in PATH` for missing tools;
the gate skips the check rather than blocking. See
[fail-open semantics](../recipes.md#fail-open-semantics).

## Tier 1 — minimal

Every commit must build, every commit must format, every push must test. This
is the floor; below this the agent can land code that doesn't compile.

```toml
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"
parallel = true

# gofmt -l prints a list of misformatted files; non-empty stdout means
# formatting drift. Wrap with `[ -z "$(...)" ]` so a non-empty list exits
# non-zero and klasp blocks. `gofmt` is part of the toolchain — no install.
[[checks]]
name = "gofmt"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 30
[checks.source]
type = "shell"
command = "test -z \"$(gofmt -l .)\""

# Cheap build/typecheck — the Go equivalent of `cargo check`. Catches the
# class of bug an agent introduces fastest (broken imports, signature drift).
[[checks]]
name = "go-build"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "go build ./..."

# Full test suite on push. `-count=1` bypasses Go's test result cache —
# important on push because the agent has likely seen a cached PASS at
# commit time and we want fresh evidence before code reaches origin.
[[checks]]
name = "go-test"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "shell"
command = "go test -count=1 ./..."
```

`gofmt -l .` is intentionally hard-blocking on commit. Formatting drift in Go
is binary — there's no style debate to have, and the agent can fix it with a
single `gofmt -w` call.

## Tier 2 — serious

Adds static analysis. `go vet` is part of the toolchain and surfaces
correctness bugs the compiler doesn't catch (printf format mismatches,
unreachable code, struct-tag drift). `staticcheck` adds a deeper rule set
(`SA*` for correctness, `ST*` for style, `S*` for simplifications). Most
teams ship one or the other; this recipe shows both because they overlap
without subsuming each other.

```toml
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

# go vet on commit. Fast (sub-second on warm cache for most repos) and
# its findings are essentially never false positives — safe to hard-block.
[[checks]]
name = "go-vet"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "go vet ./..."

# staticcheck on push. Slower than vet (10-30s on a medium repo) and the
# `ST*` rules can be opinionated; allow-list to the correctness families on
# introduction, broaden once the team's comfortable. `-checks` accepts a
# comma-separated allow/deny list — see https://staticcheck.dev/docs/checks/
[[checks]]
name = "staticcheck"
triggers = [{ on = ["push"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "staticcheck -checks=SA*,S1*,ST1003,ST1005 ./..."

[[checks]]
name = "go-test"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "shell"
command = "go test -count=1 ./..."
```

### Alternative composition: `golangci-lint`

If the team already runs `golangci-lint`, replace `go-vet` and `staticcheck`
with a single check that runs the team's `.golangci.yml`. This is the most
common production setup in 2026 — `golangci-lint` bundles `govet`,
`staticcheck`, `errcheck`, `ineffassign`, `unused`, and ~45 others, with
shared caching across them. Pick one composition or the other; running
`golangci-lint` *and* `staticcheck` standalone double-charges for the same
analysis pass.

```toml
[[checks]]
name = "golangci-lint"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 240
[checks.source]
type = "shell"
# --new-from-rev scopes findings to commits since the merge-base, which
# is exactly what KLASP_BASE_REF is for. On push triggers, drop --new-from-rev
# to lint the full tree — split into two checks if you want both.
command = "golangci-lint run --new-from-rev=${KLASP_BASE_REF}"
```

## Tier 3 — strict

Append the following to the Tier 2 config. Each addition introduces wall
time or false-positive surface; promote to `any_fail` only after the team
trusts the output.

```toml
# Race detector on push only. Compiles a separate instrumented binary —
# 5-10x slower than a normal test run. Never put this on commit; the agent
# will time out and re-attempt until it gives up.
[[checks]]
name = "go-test-race"
triggers = [{ on = ["push"] }]
timeout_secs = 600
[checks.source]
type = "shell"
command = "go test -race -count=1 ./..."

# CVE scan against the Go vulnerability database. Push-only; the network
# fetch makes it inappropriate for commit. Reports only vulnerabilities
# reachable from your code (not just present in the dep graph).
[[checks]]
name = "govulncheck"
triggers = [{ on = ["push"] }]
timeout_secs = 180
[checks.source]
type = "shell"
command = "govulncheck ./..."

# Nil-safety. False-positive rate makes this warning-tier on introduction.
[[checks]]
name = "nilaway"
triggers = [{ on = ["push"] }]
timeout_secs = 240
[checks.source]
type = "shell"
command = "nilaway ./..."

# Module hygiene — fails if go.mod / go.sum drift from imports. Cheap.
[[checks]]
name = "go-mod-tidy"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "go mod tidy -diff"
```

For Tier 2's `staticcheck -checks=SA*,...` allow-list, drop the `-checks`
flag at this tier to enable the full default rule set.

## Commit vs push split

| Check | Commit | Push | Why |
|---|:---:|:---:|---|
| `gofmt -l` | yes | yes | Sub-second; format drift never lands. |
| `go build ./...` | yes | yes | Cheapest sanity check. |
| `go vet ./...` | yes | yes | Fast, near-zero false positives. |
| `go mod tidy -diff` | yes | yes | Sub-second; agents often forget to tidy. |
| `golangci-lint --new-from-rev` | yes | | Diff-scoped: fast on commit. |
| `staticcheck ./...` | | yes | 10-30s on medium repos. |
| `go test ./...` | | yes | Suite wall time; `-count=1` only on push. |
| `go test -race ./...` | | yes | 5-10x slower than plain test. |
| `govulncheck ./...` | | yes | Network fetch + database walk. |
| `nilaway ./...` | | yes | Slow; false positives common. |

Principle: commit triggers feed the agent feedback fast enough to keep the
next-iteration cycle sub-minute. Push triggers can take 5+ minutes because
the agent has already iterated past commit gates.

## Hard-block vs warning policy

- **`gofmt`, `go build`, `go vet`, `go mod tidy`** — hard-block from day
  one. Effectively zero false-positive rate; if they fire, the code is wrong.
- **`staticcheck` (full check set)** — block-tier for new code. For repos
  with existing debt, scope to correctness families (`-checks=SA*,S1*`) on
  introduction and broaden later. `ST*` and `U1*` are stylistic — promote
  gradually.
- **`go test -race`** — block-tier for new test code, but the race detector
  occasionally surfaces flakes that aren't true data races. Don't reach for
  `policy = "all_fail"` to mask flakes — it undermines the whole gate.
- **`govulncheck`** — block-tier from day one. Only reports vulnerabilities
  reachable from your code; noise floor is much lower than typical SCA
  tools. Remediation is usually `go get -u <module>`.
- **`nilaway`** — warning tier on introduction. Sound but conservative;
  expect 5-15% false positives on first run. Run non-blocking for one
  sprint, fix the genuine findings, then promote to `any_fail`.

## `KLASP_BASE_REF` patterns

Every shell check sees `KLASP_BASE_REF` set to the merge-base of `HEAD`
against the upstream tracking branch — a 10x-100x wall-time reduction on
large repos. Go-specific patterns:

```toml
# Test only the packages that contain a changed file. xargs trims to unique
# package dirs via dirname. The `${PKGS:-./...}` fallback handles the case
# where no Go files changed (docs-only commit) — `go test` on no packages
# exits 0, which is what we want.
[[checks]]
name = "go-test-changed"
triggers = [{ on = ["commit"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = "PKGS=$(git diff --name-only ${KLASP_BASE_REF} -- '*.go' | xargs -r -n1 dirname | sort -u | sed 's|^|./|' | tr '\\n' ' '); go test -count=1 ${PKGS:-./...}"

# golangci-lint has native diff-scoping — prefer this over hand-rolling.
[[checks]]
name = "golangci-lint-diff"
triggers = [{ on = ["commit"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = "golangci-lint run --new-from-rev=${KLASP_BASE_REF}"

# staticcheck has no native diff mode; pipe changed packages in.
[[checks]]
name = "staticcheck-diff"
triggers = [{ on = ["commit"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = "CHANGED=$(git diff --name-only ${KLASP_BASE_REF} -- '*.go' | xargs -r -n1 dirname | sort -u | sed 's|^|./|'); [ -z \"$CHANGED\" ] || staticcheck $CHANGED"
```

`govulncheck` has no diff-scoping — vulnerability reachability is a
whole-program property. Always run it across `./...`.

## Cache hints

- **`GOCACHE`** — honoured by default; don't disable. In CI, restore
  `$GOCACHE` (typically `~/.cache/go-build`) between runs.
- **`go test -count=1`** — bypasses the test result cache. Required on
  push for a fresh run; omit on commit so iterative re-runs are instant.
- **`GOFLAGS="-mod=readonly"`** — fail if `go.mod`/`go.sum` would change
  during the run. Recommended in any gate context.
- **`GOMAXPROCS`** — `go test -race` allocates per-CPU memory; on a
  large CI box a big suite can OOM. Pin to `GOMAXPROCS=4` per-check if so.

Pin env at the check level to keep the parent shell clean:

```toml
[[checks]]
name = "go-build"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "GOFLAGS=-mod=readonly go build ./..."
```

## Expected finding examples

Shell-form recipes surface tool stdout verbatim — the agent sees what a
human sees. Each format carries file path and line number, which is what
the agent needs to open the offending site.

```text
# go vet
./internal/store/cache.go:42:6: Printf format %s has arg n of wrong type int

# staticcheck
internal/api/handler.go:104:2: SA4006: this value of err is never used (staticcheck)
internal/api/handler.go:198:9: S1003: should use strings.Contains(s, "foo") instead of strings.Index(s, "foo") != -1 (staticcheck)

# govulncheck
Vulnerability #1: GO-2024-2611
    Excessive memory consumption when handling crafted HTTP/2 frames.
  Module: golang.org/x/net
    Found in: golang.org/x/net@v0.17.0
    Fixed in: golang.org/x/net@v0.23.0
    Example traces:
      #1: internal/proxy/server.go:88:23: proxy.Serve calls http2.Server.ServeConn

# nilaway
internal/store/lookup.go:55:9: error: Potential nil dereference in "Get":
  dereferenced return value of `cache.Lookup(key)` at line 56:9
```

If a tool's output is unhelpful (`exit status 1` and nothing else), the
agent can't act on it and will retry blindly. That's the bar a graduation
to `klasp-plugin-go` would clear.

## False positives and escape hatches

- **`//nolint:linter`** — golangci-lint. `//nolint:errcheck` for one,
  `//nolint:errcheck,gosec` for several. Pair with a justifying comment.
- **`//lint:ignore SA1019 reason here`** — staticcheck. Requires a
  non-empty justification.
- **Build tags** — `//go:build !race` skips a file under the race
  detector. Useful for tests that intentionally exercise the racing path.
- **Generated files** — staticcheck auto-skips `// Code generated ... DO
  NOT EDIT.`; golangci-lint has `issues.exclude-generated`.
- **`go vet` printf wrappers** — vet trusts known format functions only.
  Run `go vet -printfuncs=Logf,Debugf ./...` to register custom wrappers.
- **`govulncheck` unreachable CVEs** — reported as informational and
  exit 0. No escape hatch needed.

If the agent reaches for `//nolint` more than twice in a session, the right
move is usually to fix the underlying issue, not extend the ignore list.

## Module-aware notes

For a `go.work` workspace with several modules:

```text
repo/
  go.work
  klasp.toml              # root-scoped (govulncheck across workspace)
  cmd/cli/
    go.mod
    klasp.toml            # cli-only tests
  services/api/
    go.mod
    klasp.toml            # api-only tests, API integration
  packages/shared/
    go.mod                # no klasp.toml — root config applies
```

klasp walks up from each staged file to the nearest `klasp.toml`, so a
change to `services/api/handler.go` runs the api config; a change to
`packages/shared/util.go` falls through to root. Cross-group verdicts fold
under `any_fail` regardless of each group's own policy. See
[`../recipes.md` § per-service-checks](../recipes.md#per-service-checks-in-monorepos).

`replace` directives are honoured by all the tools in this recipe — no
klasp-side configuration needed. Gotcha: `go mod tidy -diff` fails if a
`replace` points outside the workspace; commit a `go.work` entry or remove
the dev-only replace before the gate runs.

## Graduation to plugin

Candidate: `klasp-plugin-go`. The plugin would normalize:

- `go vet -json` and `staticcheck -f json` per-finding rows with
  file/line/lint-code
- `go test -json` event stream parsed for per-test pass/fail/skip with
  elapsed time and stack frames
- `govulncheck -json` mapped to CVE-keyed findings
- `golangci-lint run --out-format json` per-linter per-finding rows

Until that plugin exists, the shell forms here fall back on each tool's
exit code — coarser than per-finding rows but always correct.
