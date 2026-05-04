# klasp roadmap

> **Status (2026-05-04).** v0.1 shipped on `main` at [`234908e`](https://github.com/klasp-dev/klasp/commit/234908e) (PR [#17](https://github.com/klasp-dev/klasp/pull/17), W6-7). The `v0.1.0` tag push to publish to registries is the next step and the maintainer's responsibility. **v0.2 (Codex + named recipes) is the active milestone** — checklist below is unchanged from the original commitments and work has not started.

> **Reading this in 30 seconds:** v0.1 is a Claude Code-only gate (now shipped). v0.2 adds Codex three months later. v0.2.5 adds parallel execution and monorepo discovery. v0.3 adds Cursor + Aider plus an experimental plugin model. v1.0 commits to a stable schema 9-12 months from v0.1. Nothing here is contractual — dates slip when the design slips. The shape is what matters.

For the architecture this roadmap delivers, see [`design.md`](./design.md).

---

## Versioning policy

klasp follows **semver for the binary**, a **separate monotone integer for the gate protocol** (`KLASP_GATE_SCHEMA`), and a **third monotone integer for the config schema** (the `version = N` field in `klasp.toml`).

- The binary's `0.1.x` → `0.2.x` → `0.3.x` → `1.0.x` semver tracks user-visible feature additions.
- The gate protocol bumps independently, only when the wire format between the generated `klasp-gate.sh` script and the `klasp gate` runtime changes. Most binary releases will not bump the protocol.
- `klasp.toml`'s `version = 1` bumps **only when the config syntax breaks**. Adding new optional fields does not bump it. v0.5+ may introduce `version = 2`; old binaries reading new configs fail loudly with an upgrade message.

Pre-1.0 breaking changes are allowed at minor version boundaries with a clear migration note in the changelog. **Binary API and CLI flag breaks** are the lowest-friction kind (re-install fixes them). **`klasp.toml` syntax breaks** are higher-friction because the config is committed to the user's repo and shared with their team — these are taken seriously and avoided when possible. **`KLASP_GATE_SCHEMA` bumps** are silently handled (the schema mismatch path emits a notice, fails open, and instructs the user to re-run `klasp install`).

Post-1.0, breaking changes require a major version and a deprecation cycle.

---

## v0.1 — MVP (Shipped, target: 4-6 weeks; actual: 7 weeks W1-W7)

**Status:** Implementation merged on `main` at [`234908e`](https://github.com/klasp-dev/klasp/commit/234908e) on 2026-05-04 ([PR #17](https://github.com/klasp-dev/klasp/pull/17)). Awaiting `v0.1.0` tag push to publish to registries. The originally-committed 4-6 week window stretched to 7 weeks; the dogfood window (W6-7) ran 1.5 weeks as planned, and W1-W5 hit their original schedule.

**Headline:** Claude Code only. Shell-command checks. One-command install. Public launch.

### Deliverables

- [x] Three-crate workspace: `klasp-core`, `klasp-agents-claude`, `klasp` binary [W1, [#1](https://github.com/klasp-dev/klasp/issues/1)]
- [x] Five subcommands: `init`, `install`, `uninstall`, `gate`, `doctor` [W2-W4, [#2](https://github.com/klasp-dev/klasp/issues/2) [#3](https://github.com/klasp-dev/klasp/issues/3) [#4](https://github.com/klasp-dev/klasp/issues/4)]
- [x] `klasp.toml` config with `version = 1`, `[gate]`, and `[[checks]]` blocks [W1]
- [x] Shell `CheckSource` impl (the only check source in v0.1) [W3]
- [x] `ClaudeCodeSurface` impl: surgical `.claude/settings.json` merge, generated `klasp-gate.sh` with `KLASP_GATE_SCHEMA=1`, idempotent install/uninstall [W2]
- [x] 3-tier `Verdict` (Pass / Warn / Fail) with structured `Finding` rendering [W1]
- [x] Trigger pattern matching for `git commit` / `git push` [W3]
- [x] Five-platform binary release: darwin-arm64, darwin-x64, linux-x64-gnu, linux-arm64-gnu, win-x64 [W5]
- [x] Distribution: `cargo install klasp`, `npm i -g @klasp-dev/klasp` (biome-style shim), `pip install klasp` (maturin wheel) [W5, [#5](https://github.com/klasp-dev/klasp/issues/5)]
- [x] GitHub Actions release workflow on tag push [W5]
- [x] Test suite: trait-mocked unit tests, integration tests with real Claude tool-call fixtures, contract test for `GATE_SCHEMA_VERSION`, snapshot tests for the generated script (119 tests passing)
- [x] Documentation: `README.md` quickstart, `docs/design.md`, `docs/roadmap.md`, a single recipes doc with worked examples for pre-commit / fallow / pytest / cargo / ESLint/Biome / ruff [W6-7, [#6](https://github.com/klasp-dev/klasp/issues/6)]

### Success criteria

- [x] **The launch demo works**: validated against the dogfood — klasp gates its own `cargo check` / `cargo clippy -D warnings` / `cargo test --workspace` on every commit and push.
- [x] `klasp install` is idempotent (run twice = no diff).
- [x] `klasp uninstall` is idempotent and preserves sibling hooks in `.claude/settings.json`.
- [x] `klasp doctor` correctly diagnoses: missing config, missing hook, schema mismatch, unreachable check command.
- [x] Five-platform CI matrix is green (per-PR runs 4 platforms; darwin-x64 in release-only matrix — see [#9](https://github.com/klasp-dev/klasp/issues/9)).
- [x] No telemetry. No network calls outside `cargo install` / `npm install` / `pip install` themselves.

### Explicitly out of scope for v0.1

- Codex, Cursor, Aider — every other agent surface.
- Named check recipes (e.g., `type = "pre_commit"` shorthand). Users write `command = "pre-commit run --hook-stage pre-commit --from-ref ${KLASP_BASE_REF} --to-ref HEAD"` themselves; the README documents the pattern.
- Parallel check execution. Checks run sequentially in config order.
- A `klasp run` subcommand that bypasses agents.
- Custom verdict policies beyond `any-fail-blocks`.
- A hosted runtime, team rollups, or any cloud component.

### Risks

- **Settings.json merge correctness** — the highest-risk function in v0.1. Mitigated by a comprehensive fixture suite using real production `.claude/settings.json` files.
- **Schema versioning drift** — mitigated by the contract test that fails when `GATE_SCHEMA_VERSION` is bumped without updating fixtures.
- **Windows shim behaviour** — bash runs under Git for Windows; path handling needs an audit during week 3.
- **Anthropic changes Claude Code's hook stdin schema or `.claude/settings.json` format.** The biggest external dependency. Mitigated by fail-open on parse error in `GateProtocol::parse` (so klasp degrades to no-op rather than wedging the agent), and by `klasp doctor` reporting "hook installed but never observed receiving an event" so users notice the silent degradation.

### Timeline

| Week | Milestone |
|---|---|
| 1 | Workspace skeleton, `klasp-core` types and traits, config parsing, error hierarchy |
| 2 | `ClaudeCodeSurface` install/uninstall, `settings.json` merge with full unit test suite, hook script template |
| 3 | `klasp gate` runtime, trigger classification, Shell `CheckSource`, integration tests |
| 4 | `klasp doctor`, `--dry-run` polish, Windows path audit, cross-platform CI matrix green |
| 5 | Distribution wiring: npm shim, maturin PyPI wheel, GitHub Actions release pipeline, tag `v0.1.0-rc.1` |
| 6-7 | Dogfooding: install klasp on klasp's own repo with real checks, fix discovered edge cases, write recipes doc, tag `v0.1.0`, public launch |

The dogfooding window is **1.5 weeks**, not 1, because real-world install reliably surfaces 2-3 non-trivial edge cases and no buffer is the most common slip vector for solo-maintained OSS launches. The launch gate is "the demo passes clean and `klasp doctor` is green on both klasp's own repo and at least one external test repo" — not "zero known issues."

### What v0.1 actually delivered vs the original commitments

The honest delta between this section's checkboxes and what real-world implementation surfaced:

- **`KLASP_BASE_REF` was an accidental gap caught in W6-7.** The design ([`design.md` §3.5](./design.md#35-configv1-versioned-config)) committed to exporting `KLASP_BASE_REF` to every shell child, but the W3 `Shell` source originally hardcoded a placeholder. W6-7 wired the merge-base resolution through `RepoState::base_ref` and into `Shell::run_with_timeout`, with the documented fallback chain (upstream → `origin/main` → `origin/master` → `HEAD~1`). Caught in dogfood, fixed in [PR #17](https://github.com/klasp-dev/klasp/pull/17).
- **`verdict_path` was deferred.** The original design implied a `verdict_path` field on `CheckConfig` for parsing recipe-tool JSON. The shipped `CheckConfig` has 4 fields (`name`, `triggers`, `source`, `timeout_secs`) and v0.1's only source (`Shell`) maps exit code → verdict directly. Recipe-specific output parsing is the v0.2 named-recipe scope. Tracked as part of [W5 follow-ups #16](https://github.com/klasp-dev/klasp/issues/16).
- **`x86_64-apple-darwin` was dropped from per-PR CI.** The design committed to a five-platform matrix; the per-PR matrix runs four (the macOS-x64 GitHub-hosted runner is materially slower than the others and was removed during W3 to keep PR cycle times reasonable). The tag-triggered release workflow still builds darwin-x64. Reintroduction tracked in [#9](https://github.com/klasp-dev/klasp/issues/9).
- **Two follow-up issues filed during the milestone.** [#16](https://github.com/klasp-dev/klasp/issues/16) (W5 distribution polish, open) and [#12](https://github.com/klasp-dev/klasp/issues/12) (W3 gate-runtime correctness, closed by PR [#14](https://github.com/klasp-dev/klasp/pull/14)). Both surface drift from the design is `none` — these are correctness bugs and pipeline polish, not architectural changes.
- **The 4-6 week target slipped to 7 weeks.** W1-W5 hit their original schedule; W6-7 dogfood ran 1.5 weeks as planned. The extra week is in the W6-7 window where the recipes doc, the `KLASP_BASE_REF` wiring fix, and the `cargo publish`/`maturin build` shake-out for distribution all landed. The dogfood-buffer hypothesis (above) held: real installs reliably surface 2-3 non-trivial edge cases.

For implementation-level annotations on individual abstractions, see [`design.md` §17](./design.md#17-key-implementation-notes-w1-w7).

---

## v0.2 — Codex + named recipes (target: ~3 months from v0.1)

**Headline:** Two-agent coverage with named recipes for the most common check tools. Parallel execution and richer output formats deferred to v0.2.5.

### Deliverables

- [ ] **`CodexSurface`** in a new `klasp-agents-codex` crate. Manages a `<!-- klasp:managed -->` block in `AGENTS.md`, writes `.git/hooks/pre-commit` and `.git/hooks/pre-push` for actual enforcement (Codex has no programmatic gate equivalent — see [design.md §1](./design.md#1-problem)).
- [ ] **`klasp install --agent codex`** (and `--agent all` for both Claude + Codex). Discovery auto-detects `AGENTS.md` and the agents declared in `klasp.toml`'s `[gate].agents = [...]`.
- [ ] **Named check recipes** as new `CheckSource` impls:
  - `type = "pre_commit"` — knows pre-commit's stage flags and `--from-ref` semantics
  - `type = "fallow"` — knows fallow's audit JSON schema, no `verdict_path` needed
  - `type = "pytest"` — parses pytest exit codes and optional JUnit XML for findings
  - `type = "cargo"` — wraps `cargo check` / `cargo clippy` / `cargo test`

### Success criteria

- [ ] An agent on Codex doing `git commit` triggers the same verdict path as Claude Code on the same repo, via the git hook (verified against a captured Codex session that attempts a failing commit and observing klasp's block).
- [ ] All four named recipes (`pre_commit`, `fallow`, `pytest`, `cargo`) work without any `verdict_path` config in `klasp.toml`.
- [ ] No regressions on v0.1 success criteria.

### Migration from v0.1

- v0.1 `klasp.toml` with `version = 1` continues working unchanged. No schema bump.
- Users who ran `klasp install` for v0.1 do **not** need to re-run for v0.2 unless they want to enable Codex (`klasp install --agent codex`). Claude Code coverage continues uninterrupted.

### Out of scope for v0.2

- Parallel check execution → v0.2.5
- Monorepo config discovery → v0.2.5
- JUnit/SARIF output → v0.2.5
- Configurable verdict policies → v0.2.5
- Cursor, Aider, plugin model → v0.3
- Hosted runtime → v1.0+

### Risks

- **Codex's enforcement story relies on git hooks.** Users who have other git-hook tools installed (husky, lefthook, pre-commit framework's own hooks) need klasp to coexist cleanly. Mitigated by detection-and-merge logic in `CodexSurface::install`, with a fallback to "skip git hook write, log a notice" when conflict is detected.
- **Recipe schema drift.** Each named recipe parses a specific tool's output format (pytest's xdist, fallow's JSON, etc.). Tools change their output across versions. Mitigated by integration tests using captured real outputs from multiple recipe-tool versions, plus a recipe `min_version`/`max_version` declaration that fails the recipe's check loudly when run against an unsupported tool version.

---

## v0.2.5 — Parallel + monorepo + CI output (target: ~5 months from v0.1)

**Headline:** Performance and CI ergonomics. The pieces v0.2 deferred to keep its scope honest.

### Deliverables

- [ ] **Parallel check execution** with `[gate].parallel = true` (default `false` in v0.2.5 to avoid surprising existing users; default flips in v0.3 if no regressions reported). Uses `rayon`.
- [ ] **Verdict policies**: `any_fail`, `all_fail`, `majority_fail` configurable per `[gate]`.
- [ ] **JUnit XML and SARIF output** from `klasp gate --format junit|sarif` for CI consumers.
- [ ] **Monorepo config discovery**: walk up from staged files to find the nearest `klasp.toml`, run that config's checks scoped to that subtree.

### Success criteria

- [ ] Parallel mode reduces a 5-check, 5-second-each workload to ~5s total instead of 25s.
- [ ] Monorepo with three packages and three different `klasp.toml` files runs the right config per staged-file location.
- [ ] SARIF output validates against the SARIF 2.1 schema; JUnit output is consumable by GitHub Actions' test reporter and GitLab's JUnit integration.

### Migration from v0.2

- The `[gate].parallel` field is new but optional with `false` default. v0.2 configs continue working.
- Enabling `parallel = true` requires re-running `klasp install` once; the new generated script bumps `KLASP_GATE_SCHEMA=2` to signal the new env-var contract. Old shims with `KLASP_GATE_SCHEMA=1` see a one-line "schema mismatch, re-run klasp install" notice and fail open until updated. No silent breakage.

### Risks

- **Parallel-mode race conditions** between checks that touch shared state (the same temp file, the same git index, etc.). Mitigated by documenting that checks must be stateless and by integration tests that intentionally provoke common race patterns.

---

## v0.3 — Wider agent support + experimental plugin model (target: ~6 months from v0.1)

**Headline:** Cursor and Aider. Third parties can ship `klasp-plugin-*` binaries — protocol marked experimental until v1.0 promotes it.

### Deliverables

- [ ] **`AiderSurface`** in `klasp-agents-aider` crate. Edits `.aider.conf.yml` `commit-cmd-pre` field.
- [ ] **`CursorSurface`** in `klasp-agents-cursor` crate (conditional on Cursor's hook surface stabilising — see Risks).
- [ ] **Experimental plugin protocol** with `PLUGIN_PROTOCOL_VERSION = 0`. Subprocess-based, JSON over stdin/stdout. The `0` is intentional: the protocol is **not stable**, may break in any v0.3.x release, and graduates to `PLUGIN_PROTOCOL_VERSION = 1` only at v1.0 after real-world plugin authors have stress-tested it.
- [ ] **`klasp plugins`** subcommand: `list` (scan `$PATH` for `klasp-plugin-*` binaries), `info <name>` (run plugin with `--describe` to print its supported config types), `disable <name>`.
- [ ] **First reference plugin**: `klasp-plugin-pre-commit` ships as a separate crate, demonstrates the plugin model end-to-end. Documented as "pre-stable; expect breaking changes until v1.0." The built-in `pre_commit` recipe from v0.2 stays as the default; the plugin is the "you can do this in your own crate" reference.
- [ ] **Stronger trigger patterns**: configurable in `[[trigger]]` blocks beyond the built-in commit/push regex. Users can add `jj git push`, `gh pr create`, custom aliases.
- [ ] **`klasp gate --format json`** with a documented `KLASP_OUTPUT_SCHEMA = 1` for tooling consumers (note: the output schema *is* stable starting at v0.3, separate from the plugin protocol).

### Success criteria

- [ ] **Aider-only baseline:** Aider, Claude, and Codex all install via `klasp install --agent all` and gate correctly. (Cursor not required for v0.3 to ship — see Risks.)
- [ ] A third-party developer publishes a working `klasp-plugin-X` to crates.io / GitHub releases without depending on klasp's main crate or modifying klasp's source.
- [ ] No regressions on v0.2.5 success criteria.

### Risks

- **CursorSurface depends on Cursor having a documented, stable hook API by month 4 of v0.3 development.** Cursor's extension surface is moving; as of v0.1's authoring it does not have a `PreToolUse`-equivalent. **Go/no-go decision at month 4 of v0.3 work**: if Cursor's hook surface is not stable and documented, ship v0.3 with Aider + plugin model only; Cursor moves to v0.3.x or v1.0. The roadmap does not over-commit on something we don't control.
- **Plugin protocol mistakes are expensive to fix.** The reason `PLUGIN_PROTOCOL_VERSION = 0` is the explicit experimental tier rather than `= 1`. Plugin authors are warned in the docs that any v0.3.x release may break their plugin; only at v1.0 does the protocol commit to backward compat.
- **`AGENTS.md` managed-block markers must coexist with other tools using the same convention** (some teams already use `<!-- generated by X -->` blocks). Mitigated by namespacing klasp's markers (`<!-- klasp:managed:start -->`).

### Out of scope for v0.3

- Hosted runtime, team rollups → v1.0+
- Auto-fix mode → never; klasp gates, it doesn't write code

---

## v1.0 — Stable schema commitment (target: 9-12 months from v0.1)

**Headline:** klasp is officially stable. Schema versions, plugin protocol (now promoted to `PLUGIN_PROTOCOL_VERSION = 1`), and config format are all guaranteed for at least 12 months of backward compatibility.

### Deliverables

- [ ] **Schema commitment**: `GATE_SCHEMA_VERSION = N` is locked. New schema versions ship as additive only (new optional fields). Breaking changes wait for v2.0.
- [ ] **Plugin protocol commitment**: `PLUGIN_PROTOCOL_VERSION = 1` (promoted from `= 0` experimental at v0.3) is locked under the same rules. Plugin authors who tracked the experimental protocol get a clean migration path.
- [ ] **`klasp.toml` schema commitment**: `version = 1` configs continue working forever.
- [ ] **Documented migration path**: `klasp upgrade` checks the installed script's schema against the binary, runs `klasp install --force` if behind, no-ops if current.
- [ ] **Performance budget**: gate path completes in <50ms when no checks need to run (i.e., trigger doesn't match), <200ms cold including config parse. Measured by a benchmark suite that fails CI on regression.
- [ ] **Distribution**: cargo / npm / pypi / brew tap / curl installer (`curl klasp.dev/install.sh | sh`) all green.
- [ ] **Documentation site** at `klasp.dev`: architecture, recipes per language, all CLI references generated from clap, plugin authoring guide.
- [ ] **A representative real-world deployment** documented: a public GitHub repo using klasp on its own commits, ideally with two agents and three checks, as a living reference.

### Success criteria

- [ ] No breaking changes between any v1.x and v1.(x+1).
- [ ] At least one third-party plugin is in active use (i.e., klasp.dev links to it).
- [ ] **klasp is used in at least three distinct public GitHub repos not owned by the maintainer**, with real checks running on real agent commits. (This is verifiable from the repos' commit history; doesn't depend on landing a Fortune-500 logo.)

### Stretch

- [ ] Aspirational: at least one Fortune-500 or open-source-heavyweight-equivalent project uses klasp on `main`. Adoption signal — but not a release gate.
- [ ] **Hosted runtime (klasp-cloud, optional)**: an opt-in HTTP endpoint that records gate verdicts for team-level dashboards. Open-source server, paid hosting if anyone wants it. **Strictly opt-in via a `[runtime]` block in `klasp.toml`; no implicit network calls ever.**
- [ ] **Hooks beyond commit/push**: `pre-merge`, `pre-rebase`, `pre-tag`. Useful for CI integration scenarios.

---

## Out of scope (forever — these are deliberate non-goals)

- **Telemetry of any kind.** Dev tools that phone home get torched. klasp will never make a network call without an explicit user action that produces output (publishing to a registry, etc.).
- **Auto-fix capabilities.** klasp gates. Other tools fix. Conflating those produces a tool that's bad at both.
- **A klasp-managed config DSL beyond shell commands and named recipes.** No expression language, no inline scripting, no Lua/Starlark escape hatch. If you can't do it in a shell command or a recipe, write a plugin.
- **A TUI or interactive mode.** klasp is a gate, not an interactive interface. Output is structured for terminals (and `--format json` for tools), nothing else.
- **A `klasp run` standalone subcommand that runs checks bypassing agent surfaces.** Use `pre-commit run` or invoke the check tool directly. klasp's value is the agent integration; without that, it's a worse `pre-commit`.
- **Bundled checks.** klasp does not ship its own linter, dead-code detector, or test runner. The user brings the checks.
- **Anything that pretends to be a security boundary.** klasp's trigger pattern can be trivially bypassed by an adversarial agent (`bash -c "$(decode...)"` etc.). klasp helps honest agents who want to cooperate. Anyone treating it as security is misusing it; that's a known and documented limitation, not a bug to fix.

---

## How to influence the roadmap

- **Open a GitHub issue** at [github.com/klasp-dev/klasp/issues](https://github.com/klasp-dev/klasp/issues) labelled `roadmap-input`. Describe the use case, not just the feature.
- **Speculative or exploratory ideas** belong in [GitHub Discussions](https://github.com/klasp-dev/klasp/discussions) (created when the first issue arrives), not the roadmap. Streaming output, native Windows runner, GitLab/Bitbucket release pipelines, conversation memory, diff-only mode — all candidates for discussion threads where they can live without occupying milestone scope.
- **Vote with reactions** on existing roadmap-input issues. The maintainer (currently one person) reads them.
- **Major design changes** go through an `RFC-NNNN.md` PR in `docs/rfcs/`. The directory will be created the first time someone files one.
- **Plugin authors** can move faster than the core roadmap: ship `klasp-plugin-X` and link to it from your project; if it gets traction, it can graduate into a built-in recipe in a future minor release.

This roadmap is a snapshot of intent. It's revisited at each minor version boundary. The dates slip when reality demands; the shape is what matters.
