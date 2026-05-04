# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once the schema commitment lands in v1.0 (see
[roadmap.md §v1.0](./docs/roadmap.md)). Until then, schema changes within v0.x
follow the migration notes attached to each minor release.

## [Unreleased]

Nothing pending.

## [0.1.0] — pending tag push

Implementation merged on `main` at
[`234908e`](https://github.com/klasp-dev/klasp/commit/234908e) on 2026-05-04
([PR #17](https://github.com/klasp-dev/klasp/pull/17), W6-7). The release
date below is filled in when the maintainer pushes the `v0.1.0` tag and the
`release.yml` workflow publishes to crates.io / npm / PyPI / GitHub Releases.

The MVP. Claude Code only. Shell-command checks. One-command install. See
[`docs/roadmap.md` §v0.1](./docs/roadmap.md) for the full milestone definition.

### Added

- **Three-crate workspace**: `klasp-core` (library — traits, types, gate
  protocol), `klasp-agents-claude` (Claude Code surface impl), `klasp`
  (binary). [`5740eb3`, W1]
- **`klasp-core` foundations**: `ConfigV1` (versioned `klasp.toml` parser),
  `Verdict` 3-tier enum (Pass / Warn / Fail) with `Finding` rendering,
  `GateProtocol` with `GATE_SCHEMA_VERSION = 1`, `AgentSurface` and
  `CheckSource` traits, `Trigger` regex (Rust port of fallow's POSIX ERE),
  typed `KlaspError` hierarchy. [`5740eb3`, W1]
- **`ClaudeCodeSurface`**: surgical `.claude/settings.json` JSON merge
  (preserves sibling hooks and key order), generated `klasp-gate.sh`
  bash shim, idempotent install/uninstall, Unix mode preservation,
  Windows-aware (no chmod on NTFS; bash.exe via Git for Windows). [#10, W2]
- **`klasp gate` runtime**: stdin parser, schema handshake, trigger
  classification, sequential check execution via the `Shell` `CheckSource`,
  fail-open on every degradation path (parse error, schema mismatch,
  missing config, source runtime error), structured block message on
  exit 2. [#11, W3]
- **`klasp doctor`**: four-stage diagnostic (config / hook script byte-equal
  re-render check / `.claude/settings.json` entry presence / `PATH`
  resolution for every shell command), FAIL/WARN/INFO output, exits 0 iff
  zero FAIL. [#13, W4]
- **`klasp init`**: scaffolds an example `klasp.toml` at the repo root with
  `--force` for overwrite. [#13, W4]
- **`klasp install` / `klasp uninstall`**: surface-discovery flow described
  in [design.md §5](./docs/design.md#5-install-flow); `--agent`, `--force`,
  `--dry-run`. Uninstall preserves sibling hooks. [#10, W2]
- **Four-platform release pipeline**: tag-triggered GitHub Actions workflow
  builds darwin-arm64, linux-x64-gnu, linux-arm64-gnu, win-x64 binaries,
  publishes to crates.io (`klasp`), npm (biome-style platform-shim,
  `@klasp-dev/klasp`), PyPI (maturin wheel, `klasp`), and GitHub Releases.
  `x86_64-apple-darwin` (macos-13) was dropped at v0.1.0 launch — see
  `docs/roadmap.md` "What v0.1 actually delivered". [#15, W5]
- **Documentation**: [`docs/design.md`](./docs/design.md) (architecture),
  [`docs/roadmap.md`](./docs/roadmap.md) (v0.1 → v1.0 milestones),
  [`docs/recipes.md`](./docs/recipes.md) (worked `klasp.toml` examples for
  pre-commit, fallow, pytest, cargo, ESLint/Biome, ruff), `README.md`
  quickstart. [#17, W6-7]
- **Dogfood**: klasp's own repo runs `cargo check` + `cargo clippy
  -D warnings` on every commit and `cargo test --workspace` on every push
  via this same gate; `.claude/settings.json` and
  `.claude/hooks/klasp-gate.sh` are tracked in git so worktrees inherit the
  install. [#17, W6-7]
- **`KLASP_BASE_REF` env var**: every shell check's child process sees
  `KLASP_BASE_REF` set to the merge-base of `HEAD` against the upstream
  tracking branch (falling back to `origin/main`, `origin/master`, then
  `HEAD~1`), so diff-aware tools (`pre-commit run --from-ref`,
  `fallow audit --base`) can scope themselves to the diff without an
  agent-side wrapper. Matches the contract documented in
  [design.md §3.5](./docs/design.md#35-configv1-versioned-config) and
  [recipes.md](./docs/recipes.md#klasp_base_ref). [#17, W6-7]

### Fixed (W3 follow-ups)

- Added a regression test (`source_runtime_error_fails_open`) confirming the
  fail-open path in `gate.rs::run` exits 0 with a stderr notice when a
  `CheckSource` raises a runtime error mid-gate. The behaviour was already
  correct in W3 (PR [#11](https://github.com/klasp-dev/klasp/pull/11)); this
  closes a test-coverage gap surfaced during W3 follow-up review. [#14]
- `Shell` `CheckSource` reaps its child process on timeout / interrupt
  rather than leaking it; killed-by-signal paths surface the signal in the
  finding. [#14]

### Schema

- `klasp.toml` declares `version = 1`. Forward-compat: parsing fails fast on
  unknown sections (`[plugin]`, etc.) with a clear "this config is for a
  newer klasp" message rather than silently ignoring them.
- `GATE_SCHEMA_VERSION = 1` is exported by `klasp-core` and embedded in the
  generated `klasp-gate.sh` as `KLASP_GATE_SCHEMA=1`. Mismatches between an
  upgraded binary and an old hook fail-open with a notice; `klasp doctor`
  surfaces the drift.

### Out of scope (planned for v0.2+)

- Codex / Cursor / Aider surfaces — v0.2 (Codex), v0.3 (Cursor + Aider)
- Named check recipes (`type = "pre_commit"`, `type = "fallow"`,
  `type = "pytest"`, `type = "cargo"`) — v0.2
- Parallel check execution, JUnit/SARIF output, monorepo config discovery,
  configurable verdict policies — v0.2.5
- Subprocess plugin model — v0.3
- Hosted runtime / team rollups — v1.0+

See [`docs/roadmap.md`](./docs/roadmap.md) for the full plan.

[Unreleased]: https://github.com/klasp-dev/klasp/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/klasp-dev/klasp/releases/tag/v0.1.0
