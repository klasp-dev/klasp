# Rust audit recipe

A copy-pasteable klasp configuration for Rust projects. Three tiers — minimal,
serious, strict — each is a complete `klasp.toml` you can drop in at the
workspace root. Pick the tier that matches your team's tolerance for blocked
commits today; graduate up as you fix the debt the lower tier surfaces.

> klasp remains the agent gate and verdict normalizer; users bring the checks.
> This recipe wires `cargo fmt`, `cargo clippy`, `cargo test`, and the cargo
> audit ecosystem (`cargo-deny`, `cargo-machete`, `cargo-hack`, `cargo-msrv`,
> `cargo-audit`) into a klasp gate the agent hits on every `git commit` and
> `git push`. The reference dogfood config is [`/klasp.toml`](../../klasp.toml)
> at the klasp repo root — a real Rust workspace already wired through klasp.

## Target audience

Use this recipe if your repo is one of:

- **Single-crate library or binary** (`Cargo.toml` at the repo root, no
  `[workspace]`). Snippets work unchanged; drop `--workspace` if you find it
  noisy — cargo accepts it on a single crate but it's redundant.
- **Cargo workspace** with multiple member crates (the klasp repo itself,
  most production Rust services). Recipes assume this shape; single-crate is
  a strict subset.
- **Embedded / `no_std`** with a host-side test crate. Minimal and serious
  tiers work; strict needs MSRV pinning and a feature-matrix tuned to your
  target triple — see notes in the strict section.

If your repo uses `cargo-make`, `just`, `xtask`, or another orchestration
layer, point each `command` at your wrapper instead of cargo directly. The
recipe shape doesn't change.

## Required tools

Rustup ships `cargo`, `rustfmt`, and `clippy` — that covers the minimal and
serious tiers. The strict tier needs cargo plugins installed via `cargo install`:

```bash
# Minimal + serious tiers — comes with rustup
rustup component add rustfmt clippy

# Strict tier — supply-chain and dependency hygiene
cargo install cargo-deny       # license + advisory + duplicate dep checks
cargo install cargo-machete    # detect unused dependencies
cargo install cargo-hack       # feature-matrix testing
cargo install cargo-msrv       # verify minimum supported Rust version
cargo install cargo-audit      # rustsec advisory database scan
```

`cargo-deny` subsumes most of `cargo-audit` (it has its own `advisories`
section), so pick one. The strict tier uses `cargo-deny` as the primary
supply-chain gate; `cargo-audit` is shown as an optional addition.

`klasp doctor` flags missing tools with `WARN  path[name]: not found in
PATH`; the gate fails open on missing tools rather than blocking the agent.

## Tier 1 — Minimal

Fast feedback on every commit. Catches broken builds and trivial formatting
drift before the agent's commit message is even written.

```toml
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

[[checks]]
name = "cargo-check"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "cargo check --workspace --all-targets"
```

`cargo fmt --all -- --check` exits non-zero on any unformatted file without
rewriting the working tree. Don't use `cargo fmt` without `--check` in a
gate — the gate surfaces findings, it doesn't silently mutate the diff.

`cargo check --workspace --all-targets` is the cheapest sanity check:
type-checks the workspace including tests, examples, and benches without
codegen. Sub-second on a warm `target/`. `--all-targets` matters because
tests routinely introduce broken code that the default `cargo check` misses.

## Tier 2 — Serious

Tier 1 plus blocking lints and full test runs on push. This is the tier
most production teams should be on.

```toml
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

[[checks]]
name = "cargo-check"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "cargo check --workspace --all-targets"

[[checks]]
name = "cargo-clippy"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 180
[checks.source]
type = "cargo"
subcommand = "clippy"
extra_args = "--workspace --all-targets -- -D warnings"

[[checks]]
name = "cargo-test"
triggers = [{ on = ["push"] }]
timeout_secs = 300
[checks.source]
type = "cargo"
subcommand = "test"
extra_args = "--workspace"
```

Two important things at this tier.

First, `cargo clippy` runs with `-D warnings`, which promotes every warning
to an error. Without it, clippy exits zero on warnings and the gate sees a
pass while the diff has lint issues. The literal flag is `-- -D warnings` —
the bare `--` separates clippy's own args from the rustc args it forwards.

Second, clippy and test use the typed `type = "cargo"` form. The typed
recipe parses cargo's `--message-format=json` stream into per-diagnostic
findings with file + line + lint-code — what the agent needs to navigate to
the offending site. The shell form falls back on cargo's exit code alone.
Both work; typed form has better agent UX. See
[`docs/recipes.md`](../recipes.md#cargo) for the per-tool comparison.

Tests run on push only. Test wall time is the biggest per-commit cost in a
Rust repo and the agent has already seen `cargo check` + clippy verdicts at
commit time — running the full suite on every commit attempt buys little.

## Tier 3 — Strict

Tier 2 plus supply-chain hygiene, unused-dep detection, feature-matrix
testing, and MSRV verification. This is what you want on a service with
real customers.

```toml
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

[[checks]]
name = "cargo-check"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "cargo check --workspace --all-targets"

[[checks]]
name = "cargo-clippy"
triggers = [{ on = ["commit", "push"] }]
timeout_secs = 180
[checks.source]
type = "cargo"
subcommand = "clippy"
extra_args = "--workspace --all-targets --all-features -- -D warnings"

[[checks]]
name = "cargo-test"
triggers = [{ on = ["push"] }]
timeout_secs = 600
[checks.source]
type = "cargo"
subcommand = "test"
extra_args = "--workspace --all-features"

[[checks]]
name = "cargo-machete"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "cargo machete"

[[checks]]
name = "cargo-deny"
triggers = [{ on = ["push"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = "cargo deny check"

[[checks]]
name = "cargo-hack-feature-matrix"
triggers = [{ on = ["push"] }]
timeout_secs = 900
[checks.source]
type = "shell"
command = "cargo hack check --workspace --feature-powerset --no-dev-deps"

[[checks]]
name = "cargo-msrv"
triggers = [{ on = ["push"] }]
timeout_secs = 600
[checks.source]
type = "shell"
command = "cargo msrv verify"
```

Notes:

- `cargo machete` finds deps declared in `Cargo.toml` but never imported.
  Fast (parses source, no compile) so it could go on commit, but unused-dep
  churn isn't urgent enough to slow the commit cycle.
- `cargo deny check` runs all four modes (`advisories`, `bans`, `licenses`,
  `sources`). Configure via `deny.toml`; run `cargo deny init` to scaffold
  a permissive starter, then tighten over time.
- `cargo hack check --feature-powerset --no-dev-deps` checks every feature
  combination compiles. Powerset is `2^N` builds — if your crate has >6
  features, switch to `--each-feature` (linear) or `--feature-powerset
  --depth 2` (pairs). `--no-dev-deps` skips dev-deps; `cargo test` covers
  those.
- `cargo msrv verify` reads `rust-version` from `Cargo.toml` (or
  `[workspace.package]`) and verifies the codebase still compiles. Requires
  the toolchain installed; `klasp doctor` warns if not.

If you'd rather use `cargo-audit` than `cargo-deny`'s advisory layer,
add a parallel check:

```toml
[[checks]]
name = "cargo-audit"
triggers = [{ on = ["push"] }]
timeout_secs = 60
[checks.source]
type = "shell"
command = "cargo audit --deny warnings"
```

## Commit vs push split

The same split the klasp dogfood config uses, applied here:

| Trigger | Checks | Why |
|---|---|---|
| `commit` | `fmt --check`, `cargo check`, `cargo clippy` | Sub-second to ~3min on warm cache; agent retries the commit immediately on fail. |
| `push` | `cargo test`, `cargo deny`, `cargo machete`, `cargo hack`, `cargo msrv` | Slower; agent has already iterated through commit-stage feedback. Block at push to catch regressions before they reach origin. |

`cargo clippy` straddles the line — slow enough on cold cache that commit
runs hurt, but it's the most useful single check and keeping it on commit
means the agent learns about lint violations while still holding the
context that produced them. Keep it on commit unless wall time is
intolerable.

## Hard-block vs warning

`-D warnings` is the right default for clippy — no warnings reach the
agent's commit message — but legacy debt makes it impractical day one.
Two graduation paths:

1. **Per-lint allow-list in `Cargo.toml`.** Add `[workspace.lints.clippy]`
   with the noisy lints set to `allow`. Promote to `warn` then `deny` as
   you fix debt.
2. **Drop `-D warnings` initially.** Run clippy without the deny flag
   first; promote once the baseline is clean.

For `cargo deny`, start with `cargo deny check advisories` (rustsec only),
expand to full `cargo deny check` once `deny.toml` is configured, and
promote known-OK hits to explicit `exceptions` in `deny.toml` rather than
ignoring deny output wholesale.

## `KLASP_BASE_REF` and diff scoping

cargo doesn't natively scope to changed files — the compilation graph
is whole-crate by design — but the gate runtime still sets
`KLASP_BASE_REF` in every shell check's environment, and you can use it
to drive crate-level scoping in monorepos:

```toml
[[checks]]
name = "cargo-test-changed"
triggers = [{ on = ["commit"] }]
timeout_secs = 300
[checks.source]
type = "shell"
command = "git diff --name-only ${KLASP_BASE_REF} HEAD | xargs -r dirname | sort -u | xargs -r -I{} cargo test -p $(basename {})"
```

This is fragile (the path-to-package mapping isn't always `basename`).
Most teams settle on:

1. Let `cargo test --workspace` run on push; rely on cargo's incremental
   cache for speed.
2. `--workspace --exclude <slow-crate>` drops chronically slow crates from
   the commit-stage check, with `cargo test -p <slow-crate>` on push.

## Cache hints

Cargo's incremental compilation does most of the work; a few additions help:

- **`target/` cache.** Don't `rm -rf target/` between agent iterations.
- **`sccache`** — set `RUSTC_WRAPPER=sccache`. Caches rustc invocations
  across crates and machines (S3/GCS backend). Worth it on workspaces
  with >10 member crates.
- **`CARGO_INCREMENTAL=1`** — default for debug builds. Some CI setups
  disable it; ensure local agent gates don't inherit `CARGO_INCREMENTAL=0`.
- **`cargo --offline`** — skip the dependency index fetch. Useful when
  agent network is restricted; harmless when the cache is warm.

## Expected findings

Sample agent-visible output for each tier's failure modes:

**clippy** (`-D warnings`):

```
error: unused variable: `x`
  --> src/lib.rs:42:9
   |
42 |     let x = compute();
   |         ^ help: if this is intentional, prefix it with an underscore: `_x`
   |
   = note: `-D unused-variables` implied by `-D warnings`
```

**cargo test** (typed recipe parses the trailing summary line):

```
test result: FAILED. 14 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
failures:
    crate::tests::auth::expired_token_rejected
```

**cargo deny advisory**:

```
error[A001]: Vulnerability detected
   ┌─ Cargo.lock:34:1
   │
34 │ time 0.1.45 registry+https://github.com/rust-lang/crates.io-index
   │ ----------- security vulnerability detected
   │
   = ID: RUSTSEC-2020-0071
   = Advisory: https://rustsec.org/advisories/RUSTSEC-2020-0071
```

The agent gets file, line, and lint/advisory code in each case —
enough to navigate and fix without re-running locally first.

## Common false positives and escape hatches

- **`#[allow(clippy::xxx)]`** on the offending item or module. Use when a
  lint is genuinely wrong for the local context — not when you don't feel
  like fixing it. A reviewer spots the difference.
- **`#[ignore]`** on a flaky test. `cargo test` skips ignored by default;
  `cargo test -- --ignored` runs only them. Keep this rare and time-boxed.
- **`#[cfg_attr(not(test), allow(unused))]`** when an item is only used
  in tests. Common with helper fixtures.
- **`unsafe_code = "forbid"` in `[lints.rust]`** — opt crates into
  unsafe-free at the manifest level rather than via clippy.
- **`exceptions` in `deny.toml`** for known-OK license or advisory hits.
  Each exception should carry a comment explaining why; without it the
  list rots into a silent allow-all.

## Workspace-specific notes

Every cargo command in this recipe takes `--workspace` already. A few
extras:

- **Per-member `[lints]` overrides.** Set workspace-wide lints in
  `[workspace.lints.clippy]`, then inherit per-member via
  `[lints] workspace = true`. Members override individual lints locally.
- **`--all-features` vs default.** `cargo check --workspace` uses each
  crate's defaults. If non-default features matter, add a parallel check
  with `--all-features`.
- **Per-crate `package = "<crate>"`** in the typed cargo recipe. Useful
  when one member is dramatically slower and you want it on a separate
  trigger. See [`docs/recipes.md`](../recipes.md#cargo).
- **`exclude = ["examples/*"]`** — workspace excludes aren't covered by
  `--workspace`. Add explicit checks for those subtrees if you want them
  gated (the klasp repo does this for `examples/`).

## Embedded / `no_std`

Minimal and serious tiers work as-is for `no_std` crates running host-side
tests. Strict needs adjustments:

- `cargo hack check --feature-powerset` should add `--no-dev-deps --target
  <triple>` to exercise the cross-compile path that matters for production.
- `cargo msrv verify` is doubly important — embedded toolchain support lags
  stable Rust. Pin `rust-version` in `Cargo.toml` to whatever your bootloader
  / RTOS supports.
- `cargo test` usually doesn't run on the target; gate on host-side tests
  (`#[cfg(test)]` modules in a `std`-using test crate).

## Graduation to plugin

If teams running the strict-tier recipe across many repos rewrite the same
shell glue in each `klasp.toml`, the recipe is a candidate for graduating
into `klasp-plugin-rust` — one config block dispatching to all of cargo
fmt / clippy / test / deny / machete / hack / msrv with structured
per-tool findings instead of relying on exit codes. The typed `type =
"cargo"` recipe already covers fmt / check / clippy / test; the plugin
graduation would fold in the audit ecosystem under one verdict.

File a feature request on the klasp repo to prioritise this.
