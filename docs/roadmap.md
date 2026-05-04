# klasp roadmap

> **Reading this in 30 seconds:** v0.1 is a Claude Code-only gate that ships in 4-6 weeks. v0.2 adds Codex three months later. v0.3 adds Cursor + Aider plus a plugin model. v1.0 commits to a stable schema 9-12 months from v0.1. Nothing here is contractual — dates slip when the design slips. The shape is what matters.

For the architecture this roadmap delivers, see [`design.md`](./design.md).

---

## Versioning policy

klasp follows **semver for the binary** and a **separate monotone integer for the gate protocol** (`KLASP_GATE_SCHEMA`).

- The binary's `0.1.x` → `0.2.x` → `0.3.x` → `1.0.x` semver tracks user-visible feature additions.
- The gate protocol bumps independently, and only when the wire format between the generated `klasp-gate.sh` script and the `klasp gate` runtime changes. Most binary releases will not bump the protocol.
- `klasp.toml` declares its own integer `version = 1`. v0.5+ may introduce `version = 2` configs; old binaries reading new configs fail loudly with an upgrade message.

Pre-1.0, **breaking changes are allowed** at minor version boundaries with a clear migration note in the changelog. Post-1.0, breaking changes require a major version and a deprecation cycle.

---

## v0.1 — MVP (target: 4-6 weeks from project start)

**Headline:** Claude Code only. Shell-command checks. One-command install. Public launch.

### Deliverables

- [ ] Three-crate workspace: `klasp-core`, `klasp-agents-claude`, `klasp` binary
- [ ] Five subcommands: `init`, `install`, `uninstall`, `gate`, `doctor`
- [ ] `klasp.toml` config with `version = 1`, `[gate]`, and `[[checks]]` blocks
- [ ] Shell `CheckSource` impl (the only check source in v0.1)
- [ ] `ClaudeCodeSurface` impl: surgical `.claude/settings.json` merge, generated `klasp-gate.sh` with `KLASP_GATE_SCHEMA=1`, idempotent install/uninstall
- [ ] 3-tier `Verdict` (Pass / Warn / Fail) with structured `Finding` rendering
- [ ] Trigger pattern matching for `git commit` / `git push`
- [ ] Five-platform binary release: darwin-arm64, darwin-x64, linux-x64-gnu, linux-arm64-gnu, win-x64
- [ ] Distribution: `cargo install klasp`, `npm i -g @klasp-dev/klasp` (biome-style shim), `pip install klasp` (maturin wheel)
- [ ] GitHub Actions release workflow on tag push
- [ ] Test suite: trait-mocked unit tests, integration tests with real Claude tool-call fixtures, contract test for `GATE_SCHEMA_VERSION`, snapshot tests for the generated script
- [ ] Documentation: `README.md` quickstart, `docs/design.md`, `docs/roadmap.md`, a single recipes doc with worked examples for pre-commit / fallow / pytest / cargo

### Success criteria

- [ ] **The launch demo works**: install klasp on a Python project that uses pre-commit + ruff + pytest; have Claude Code attempt a commit that fails ruff; klasp blocks with structured findings; Claude self-corrects and re-attempts.
- [ ] `klasp install` is idempotent (run twice = no diff).
- [ ] `klasp uninstall` is idempotent and preserves sibling hooks in `.claude/settings.json`.
- [ ] `klasp doctor` correctly diagnoses: missing config, missing hook, schema mismatch, unreachable check command.
- [ ] Five-platform CI matrix is green.
- [ ] No telemetry. No network calls outside `cargo install` / `npm install` / `pip install` themselves.

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

### Timeline

| Week | Milestone |
|---|---|
| 1 | Workspace skeleton, `klasp-core` types and traits, config parsing, error hierarchy |
| 2 | `ClaudeCodeSurface` install/uninstall, `settings.json` merge with full unit test suite, hook script template |
| 3 | `klasp gate` runtime, trigger classification, Shell `CheckSource`, integration tests |
| 4 | `klasp doctor`, `--dry-run` polish, Windows path audit, cross-platform CI matrix green |
| 5 | Distribution wiring: npm shim, maturin PyPI wheel, GitHub Actions release pipeline, tag `v0.1.0-rc.1` |
| 6 | Dogfooding: install klasp on klasp's own repo with real checks, fix discovered edge cases, write recipes doc, tag `v0.1.0`, public launch |

---

## v0.2 — Codex + recipes + parallel execution (target: ~3 months from v0.1)

**Headline:** Two-agent coverage. Named recipes for the most common check tools. Parallel check execution.

### Deliverables

- [ ] **`CodexSurface`** in a new `klasp-agents-codex` crate. Manages a `<!-- klasp:managed -->` block in `AGENTS.md`, writes a parallel `git pre-commit` / `pre-push` hook for actual enforcement (Codex has no programmatic gate equivalent — see design doc §1).
- [ ] **`klasp install --agent codex`** (and `--agent all` for both). Discovery auto-detects `AGENTS.md` and the agents declared in `klasp.toml`'s `[gate].agents = [...]`.
- [ ] **Named check recipes** as new `CheckSource` impls:
  - `type = "pre_commit"` — knows pre-commit's stage flags and `--from-ref` semantics
  - `type = "fallow"` — knows fallow's audit JSON schema, no `verdict_path` needed
  - `type = "pytest"` — parses pytest exit codes and optional JUnit XML for findings
  - `type = "cargo"` — wraps `cargo check` / `cargo clippy` / `cargo test`
- [ ] **Parallel check execution** with `[gate].parallel = true` (default `false` in v0.2 to avoid surprising v0.1 users). Uses `rayon` rather than `tokio` to keep the gate synchronous.
- [ ] **Verdict policies**: `any_fail`, `all_fail`, `majority_fail` configurable per `[gate]`.
- [ ] **JUnit XML and SARIF output** from `klasp gate --format junit|sarif` for CI consumers.
- [ ] **Monorepo config discovery**: walk up from staged files to find the nearest `klasp.toml`, run that config's checks scoped to that subtree.

### Success criteria

- [ ] An agent on Codex doing `git commit` triggers the same verdict path as Claude Code on the same repo, via the git hook.
- [ ] `pre-commit` recipe runs without any `verdict_path` config.
- [ ] Parallel mode reduces a 5-check, 5-second-each workload to ~5s total instead of 25s.
- [ ] Monorepo with three packages and three different `klasp.toml` files runs the right config per staged-file location.

### Migration from v0.1

- `klasp.toml` v1 is fully forward-compatible with v0.2. No user action required.
- `klasp install` re-run upgrades the script template if the binary is newer (script schema may bump from 1 to 2 if `[gate].parallel` introduces a new env var; users see "Re-run klasp install" notice meanwhile).

### Out of scope for v0.2

- Cursor, Aider — v0.3.
- Plugin model — v0.3.
- Hosted runtime — v1.0+.

---

## v0.3 — Wider agent support + plugin model (target: ~6 months from v0.1)

**Headline:** Cursor and Aider land. Third parties can ship `klasp-plugin-*` binaries.

### Deliverables

- [ ] **`CursorSurface`** in `klasp-agents-cursor` crate. Writes to `.cursor/rules/klasp.mdc` (or whatever Cursor's hook surface stabilises to by then).
- [ ] **`AiderSurface`** in `klasp-agents-aider` crate. Edits `.aider.conf.yml` `commit-cmd-pre` field.
- [ ] **Plugin protocol v1**: subprocess-based, JSON over stdin/stdout, declared `PLUGIN_PROTOCOL_VERSION`. See design.md §8 for the sketch.
- [ ] **`klasp plugins`** subcommand: `list` (scan `$PATH` for `klasp-plugin-*` binaries), `info <name>` (run plugin with `--describe` to print its supported config types), `disable <name>`.
- [ ] **First reference plugin**: `klasp-plugin-pre-commit` ships as a separate crate, demonstrates the plugin model end-to-end. (The built-in `pre_commit` recipe from v0.2 stays as the default; the plugin is the "you can do this in your own crate" reference.)
- [ ] **Stronger trigger patterns**: configurable in `[[trigger]]` blocks beyond the built-in commit/push regex. Users can add `jj git push`, `gh pr create`, custom aliases.
- [ ] **`klasp gate --format json`** with a documented `KLASP_OUTPUT_SCHEMA = 1` for tooling consumers.

### Success criteria

- [ ] A third-party developer publishes a working `klasp-plugin-X` to crates.io / GitHub releases without depending on klasp's main crate or modifying klasp's source.
- [ ] Three agents (Claude / Codex / Cursor) and Aider all install via a single `klasp install --agent all`.
- [ ] No regressions on v0.2 success criteria.

### Out of scope for v0.3

- Hosted runtime, team rollups — v1.0+.
- Auto-fix mode (klasp generates patches for findings) — never; klasp gates, it doesn't write code.

---

## v1.0 — Stable schema commitment (target: 9-12 months from v0.1)

**Headline:** klasp is officially stable. Schema versions, plugin protocol, config format are all guaranteed for at least 12 months of backward compatibility.

### Deliverables

- [ ] **Schema commitment**: `GATE_SCHEMA_VERSION = N` is locked. New schema versions ship as additive only (new optional fields). Breaking changes wait for v2.0.
- [ ] **Plugin protocol commitment**: `PLUGIN_PROTOCOL_VERSION = N` is locked under the same rules.
- [ ] **`klasp.toml` schema commitment**: `version = 1` configs continue working forever.
- [ ] **Documented migration path**: `klasp upgrade` checks the installed script's schema against the binary, runs `klasp install --force` if behind, no-ops if current.
- [ ] **Performance budget**: gate path completes in <50ms when no checks need to run (i.e., trigger doesn't match), <200ms cold including config parse. Measured by a benchmark suite that fails CI on regression.
- [ ] **Distribution**: cargo / npm / pypi / brew tap / curl installer (`curl klasp.dev/install.sh | sh`) all green.
- [ ] **Documentation site** at `klasp.dev`: architecture, recipes per language, all CLI references generated from clap, plugin authoring guide.
- [ ] **A representative real-world deployment** documented: a public GitHub repo using klasp on its own commits, ideally with two agents and three checks, as a living reference.

### Success criteria

- [ ] No breaking changes between any v1.x and v1.(x+1).
- [ ] At least one third-party plugin is in active use (i.e., klasp.dev links to it).
- [ ] At least one Fortune-500 or open-source-heavyweight-equivalent project uses klasp on `main`. (Wishful, but stated as a target — adoption signal matters more than feature count.)

### Stretch

- [ ] **Hosted runtime (klasp-cloud, optional)**: an opt-in HTTP endpoint that records gate verdicts for team-level dashboards. Open-source server, paid hosting if anyone wants it. **Strictly opt-in via a `[runtime]` block in `klasp.toml`; no implicit network calls ever.**
- [ ] **Hooks beyond commit/push**: `pre-merge`, `pre-rebase`, `pre-tag`. Useful for CI integration scenarios.

---

## Beyond v1.0 (exploratory, no commitments)

- **Streaming output to the agent during slow checks.** v0.1-v1.0 buffer check output; for very slow checks, streaming would let the agent see "ruff: 47 files lint clean..." mid-run.
- **GitLab and Bitbucket equivalents** of the GitHub-only release pipeline.
- **Native Windows runner.** v0.1 ships Windows binaries via the bash shim under Git for Windows. A native PowerShell or cmd shim would remove the bash dependency.
- **Cross-tool conversation memory.** Each agent surface today is independent. A v2.0 conversation surface where klasp tracks "this fix attempt failed three times, escalate to human" would change the UX meaningfully — but it requires storage and is therefore not in scope until the hosted runtime is real.
- **`klasp gate --diff-only` mode** that runs only against changed files, leveraging trigger context to know what changed. Recipes that already support this (ruff, eslint) would benefit; others would be unaffected.

---

## Out of scope (forever — these are deliberate non-goals)

- **Telemetry of any kind.** Dev tools that phone home get torched. klasp will never make a network call without an explicit user action that produces output (publishing to a registry, etc.).
- **Auto-fix capabilities.** klasp gates. Other tools fix. Conflating those produces a tool that's bad at both.
- **A klasp-managed config DSL beyond shell commands and named recipes.** No expression language, no inline scripting. If you can't do it in a shell command or a recipe, write a plugin.
- **Bundled checks.** klasp does not ship its own linter, dead-code detector, or test runner. The user brings the checks.
- **Anything that pretends to be a security boundary.** klasp's trigger pattern can be trivially bypassed by an adversarial agent (`bash -c "$(decode...)"` etc.). klasp helps honest agents who want to cooperate. Anyone treating it as security is misusing it; that's a known and documented limitation, not a bug to fix.

---

## How to influence the roadmap

- **Open a GitHub issue** at [github.com/klasp-dev/klasp/issues](https://github.com/klasp-dev/klasp/issues) labelled `roadmap-input`. Describe the use case, not just the feature.
- **Vote with reactions** on existing roadmap-input issues. The maintainer (currently one person) reads them.
- **Major design changes** go through an `RFC-NNNN.md` PR in `docs/rfcs/`. The directory will be created the first time someone files one.
- **Plugin authors** can move faster than the core roadmap: ship `klasp-plugin-X` and link to it from your project; if it gets traction, it can graduate into a built-in recipe in a future minor release.

This roadmap is a snapshot of intent. It's revisited at each minor version boundary. The dates slip when reality demands; the shape is what matters.
