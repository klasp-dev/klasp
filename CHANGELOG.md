# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once the schema commitment lands in v1.0 (see
[roadmap.md §v1.0](./docs/roadmap.md)). Until then, schema changes within v0.x
follow the migration notes attached to each minor release.

## [Unreleased]

Nothing pending.

## [0.3.0]

One `klasp.toml`, three surfaces, identical gate contract. Aider joins Claude Code
and Codex as a fully-supported agent surface. Plugin protocol v0 ships for
third-party extensibility. Cursor is documented as not supported in v0.3 (NO-GO
assessment in W5/#44).

### Added

- **Aider agent surface ([#40], [#46])** — `klasp install --agent aider` edits
  `.aider.conf.yml`'s `commit-cmd-pre` key to invoke the klasp gate before every
  Aider commit. Uninstall removes only the klasp entry; existing commands are
  chained (klasp first, user value second). The `aider` surface is now registered
  in the CLI surface registry alongside `claude_code` and `codex`.
  `klasp install --agent all` installs all three in one step.
- **Aider captured-session integration test ([#46])** — `klasp/tests/aider_captured_session.rs`
  verifies that a failing commit via Aider is blocked by klasp with a structured
  verdict identical in shape to Claude Code and Codex (same `klasp-gate: blocked`
  prefix, same `errors` count, same `policy=` tag, same findings array).
  Covers: failing commit blocked, passing commit allowed, verdict shape parity,
  install/uninstall round-trip, `doctor` reports surface as installed.
- **Plugin protocol v0 smoke tests ([#46])** — `klasp/tests/plugin_smoke.rs`
  exercises the full end-to-end path: plugin binary on PATH, `type = "plugin"`
  check in `klasp.toml`, `klasp gate` dispatches correctly, pass/fail routing
  works. Validates the `klasp-plugin-pre-commit` reference plugin's `--describe`
  output reports `protocol_version = 0`.
- **`docs/conformance-matrix.md` ([#46], [#68])** — public contract table listing
  per-surface support across install / uninstall / doctor / commit-gate /
  push-gate / structured-verdict / conflict / captured-session. Claude Code,
  Codex, and Aider are all-green `✓`. Cursor documents the NO-GO verdict with a
  pointer to `docs/cursor-assessment.md`.
- **`docs/plugins.md` ([#46])** — plugin authoring guide stub. Covers:
  `PLUGIN_PROTOCOL_VERSION = 0`, `--describe` / `--gate` wire format, naming
  convention (`klasp-plugin-<name>`), `klasp.toml` configuration, and the
  "fork this directory" mantra. Links to `docs/plugin-protocol.md` for the
  full spec and `examples/klasp-plugin-pre-commit/` for the canonical example.

### Changed

- `SurfaceRegistry` now includes `AiderSurface` alongside `ClaudeCodeSurface`
  and `CodexSurface`. `klasp install --agent aider`, `klasp uninstall --agent aider`,
  and `klasp doctor` all dispatch to the aider surface correctly.
- README "What works today" table updated: Aider row added with v0.3 status.
  Install section documents `klasp install --agent aider`. Conformance matrix
  linked from the README.
- Version bumped: workspace `Cargo.toml` → `0.3.0`, `pypi/pyproject.toml` →
  `0.3.0`. All path-dependency version specifiers updated across workspace members.

### Deferred

- Cursor surface — NO-GO. See `docs/cursor-assessment.md`.
- Killer demo third agent recording (Aider) — video/recording task; tracked as
  a follow-up to [#69].

## [0.2.5]

The performance + CI-output release. Schema bumped to 2 — re-run `klasp install` after upgrade.

### Added

- **`[gate].parallel = true` ([#34])** — opt-in rayon work-stealing pool over the per-config check loop. Default `false` (back-compat). A 5-check workload that takes ~25 seconds sequentially completes in ~5 seconds in parallel mode. Checks must be stateless; see `docs/design.md §6.1`.
- **Verdict policies `all_fail` / `majority_fail` ([#35])** — `[gate].policy = "all_fail"` requires unanimous failure to block; `"majority_fail"` requires >50% of decisive (non-Warn) verdicts to fail before blocking. `any_fail` remains the default.
- **`klasp gate --format junit|sarif` ([#36], [#37])** — Surefire 3.0 JUnit XML and SARIF 2.1.0 JSON output. `--output <PATH>` writes to disk; default stdout for machine formats, stderr for terminal. Validated against schemas.
- **Monorepo config discovery ([#38])** — `klasp gate` walks up from each staged file to find the nearest enclosing `klasp.toml` and runs that config's checks scoped to that group's files. Files outside any config emit a notice and are skipped, not erroring. Cross-group verdict aggregation under `AnyFail`.
- **`KLASP_GATE_SCHEMA = 2`** — bumped to signal the new env-var contract. Old shims with `KLASP_GATE_SCHEMA=1` see "klasp-gate: schema mismatch (...), skipping. Re-run `klasp install` to update the hook." and fail open at runtime — no silent breakage.

### Migration from v0.2

After `cargo install klasp` (or your package manager equivalent) upgrades the binary, **re-run `klasp install`** in each enrolled repo. Old shims fail open with the schema-mismatch notice until the hook is regenerated. v0.2 configs without `parallel`/`policy` continue working unchanged.

### Internal

- `RepoState.staged_files: Vec<PathBuf>` — new field exposing the per-group file scope to `CheckSource` impls. Empty Vec means whole-repo behaviour for callers that don't dispatch per-group.
- `klasp-core::GATE_SCHEMA_VERSION` bumped to `2` — single source of truth for the schema version across crates.

## [0.2.3]

Two critical bugfixes surfaced during the smart-dispatch dogfood test of v0.2.2.

### Fixed

- **`pre_commit` and `fallow` recipes now scope to the staged index on `commit` trigger.** Both recipes previously emitted ref-range argv (`--from-ref/--to-ref` for pre-commit, `--base` for fallow) regardless of trigger. At PreToolUse for `git commit`, `HEAD` is the parent — staged changes are invisible to ref-range scoping. Result: violations in the staged index passed klasp's gate; pre-commit's own `.git/hooks/pre-commit` framework caught them instead, so klasp added zero signal over a vanilla pre-commit setup. Now: `commit` trigger → no ref args (pre-commit defaults to staged files; fallow audits working tree). `push` trigger → existing ref-range form preserved. [#72]

- **`klasp doctor` now respects `[gate].agents` instead of auto-detecting Codex from `AGENTS.md` presence.** Previously, doctor called `surface.detect()` to decide which surfaces to check; `CodexSurface.detect()` returns true whenever `AGENTS.md` exists. False-positive `FAIL hook[codex]:` on every project that uses `AGENTS.md` as a docs file unrelated to Codex. Now: doctor iterates `config.gate.agents` as authoritative; `AGENTS.md` filesystem signal is preserved as a non-fatal `INFO` advisory ("AGENTS.md present but codex not in [gate].agents — `klasp install --agent codex` to enable"). Falls back to `detect()` only when config fails to load. [#73]

## [0.2.2]

First actually-published release of the v0.2 line. Functionally identical to
the intended v0.2.0 / v0.2.1 work. Both earlier tags shipped but never
published — `v0.2.0` was blocked by an unbumped `klasp-agents-codex` path-dep
specifier (`scripts/bump-source-versions.mjs` hardcoded list missed the new
W2 crate; fixed in #66), and `v0.2.1` was blocked by an invalid `secrets`
context in step-level `if:` expressions in `release.yml` (W7 OIDC split;
fixed in #67). v0.2.2 is the first tag pushed against a release.yml that
GitHub's workflow validator accepts.



### Added

- **Codex agent support** — `klasp install --agent codex` writes
  `.codex/git-hooks/<gate>.sh`, wiring the same gate protocol into Codex's
  git-hook surface. [#52, W2] `--agent all` installs both Claude Code and
  Codex in one command. [#54, W3] Conflict detection warns (and with
  `--force` overwrites) when an existing hook is present at the target path.
  [#52-#54, W2-W3]
- **Named recipes** — `[checks.source]` now accepts a typed `type` field
  beyond the existing `type = "shell"` form. Four typed recipes shipped
  across W4–W6:
  - `type = "pre_commit"` — invokes `pre-commit run` with the correct
    `--hook-stage`, `--from-ref`, and `--to-ref` flags; parses per-hook
    output into structured findings. [#56, W4]
  - `type = "fallow"` — runs `fallow audit` scoped to `KLASP_BASE_REF`;
    parses fallow's JSON output into the `Verdict` / `Finding` model.
    [#57, W5]
  - `type = "pytest"` — runs pytest, parses JUnit-XML output, maps
    failures to structured findings. [#58, W6]
  - `type = "cargo"` — runs `cargo test` (or a configurable subcommand),
    parses stderr diagnostics into findings. [#58, W6]

  Each recipe emits a `verdict.json` file (path configurable via
  `verdict_path` in `klasp.toml`; auto-placed in dogfood mode) consumed by
  the gate before the agent sees a result.
- **`klasp_core::recipes` trait** — shared abstraction used by all four
  named recipes, providing uniform `run() -> Verdict` and `parse_output()`
  contracts so future recipes can be added without touching gate
  orchestration code. [#56, W4]

### Out of scope (planned for v0.2.5+)

- Parallel check execution, JUnit/SARIF output, monorepo config discovery,
  configurable verdict policies — v0.2.5
- Cursor / Aider surfaces — v0.3

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

[Unreleased]: https://github.com/klasp-dev/klasp/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/klasp-dev/klasp/compare/v0.2.5...v0.3.0
[0.2.5]: https://github.com/klasp-dev/klasp/compare/v0.2.3...v0.2.5
[0.2.3]: https://github.com/klasp-dev/klasp/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/klasp-dev/klasp/compare/v0.1.0...v0.2.2
[0.1.0]: https://github.com/klasp-dev/klasp/releases/tag/v0.1.0
[#34]: https://github.com/klasp-dev/klasp/pull/34
[#35]: https://github.com/klasp-dev/klasp/pull/35
[#36]: https://github.com/klasp-dev/klasp/pull/36
[#37]: https://github.com/klasp-dev/klasp/pull/37
[#38]: https://github.com/klasp-dev/klasp/pull/38
[#40]: https://github.com/klasp-dev/klasp/pull/40
[#46]: https://github.com/klasp-dev/klasp/issues/46
[#68]: https://github.com/klasp-dev/klasp/issues/68
[#69]: https://github.com/klasp-dev/klasp/issues/69
