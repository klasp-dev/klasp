# klasp v0.1 — Design

> **Status:** v0.1 design, pre-implementation. The repo currently ships only `0.0.0` name-reservation placeholders on crates.io / npm / PyPI. This document is the build target.

This is the architectural reference for klasp v0.1. It describes the abstractions, runtime flows, and trade-offs the implementation will commit to. Where a decision could plausibly go either way, the alternative is named and the choice is justified.

For the milestone-by-milestone shape from v0.1 → v1.0, see [`roadmap.md`](./roadmap.md).

---

## 1. Problem

AI coding agents (Claude Code, Codex, Cursor, Aider, …) commit and push code on a developer's behalf. Every team running them at scale has the same complaint: **the agent ships work that fails the same checks a human would have hit at `git commit`** — `pre-commit`, `eslint`, `cargo clippy`, the project's test suite, a custom audit script. The agent doesn't know about those gates, the gates don't know about the agent, and the failure mode is "CI catches it three minutes later, the agent has already moved on, and the developer is reviewing a broken PR."

The natural response is *"just use git pre-commit hooks"*. Three things are wrong with that:

1. **Git hooks are bypassable.** `git commit --no-verify` exists, and an agent that's trained to be helpful when commits fail will absolutely use it.
2. **Git hooks fire too late.** By the time the hook runs, the agent has already typed the command. The agent's tool-call surface (Claude Code's `PreToolUse`, etc.) fires *before* the shell ever sees `git`. That's the point of intervention where a structured "blocked, here's why" reply makes the agent self-correct rather than retry-with-no-verify.
3. **Git hooks don't ride along with clones.** A fresh worktree, a remote agent, a CI runner, a teammate's machine — none of them inherit `.git/hooks/`. They inherit `.claude/`, `AGENTS.md`, and the project's `klasp.toml` from the working tree.

[fallow-rs/fallow](https://github.com/fallow-rs/fallow) recognised this pattern first and shipped `fallow setup-hooks` to install a Claude Code gate around its own audit command. **klasp generalises that pattern**: any check command (pre-commit, fallow, pytest, ruff, custom shell), any AI agent surface, one config file.

The v0.1 scope deliberately stops at **Claude Code only**. v0.2 adds Codex. v0.3 widens to Cursor and Aider. See `roadmap.md`.

---

## 2. Architecture overview

klasp is a Rust workspace with three crates:

```
klasp/
├── klasp-core/             library — public traits, types, protocol
├── klasp-agents-claude/    impl crate — Claude Code AgentSurface
└── klasp/                  binary — the CLI users install
```

The split is not premature. It is the contract surface for the v0.3 plugin model, six months early. Plugin authors will depend on `klasp-core` and ship binaries that the main `klasp` CLI dispatches to. Putting that crate boundary in place at v0.1 means v0.3 plugins are an additive change, not a refactor that breaks compatibility for any existing user.

The runtime topology at the user's machine:

```
┌──────────────────────┐                    ┌────────────────────┐
│ Claude Code          │ stdin JSON         │ .claude/hooks/     │
│ (PreToolUse=Bash)    │ ─────────────────▶ │   klasp-gate.sh    │
│                      │                    │   (3-line shim)    │
│                      │ ◀─── exit 2 ────── │                    │
└──────────────────────┘                    └─────────┬──────────┘
                                                      │ exec
                                                      ▼
                                            ┌────────────────────┐
                                            │ klasp gate         │
                                            │ (Rust binary)      │
                                            │                    │
                                            │ ▶ parse stdin      │
                                            │ ▶ classify command │
                                            │ ▶ load klasp.toml  │
                                            │ ▶ run [[checks]]   │
                                            │ ▶ aggregate Verdict│
                                            └────────────────────┘
```

The bash shim is intentionally trivial. It exists for **auditability** (a human reviewing the repo can see exactly what gets executed without trusting an opaque binary path in `.claude/settings.json`) and for the **schema-version handshake**: the shim exports `KLASP_GATE_SCHEMA=N` before exec'ing the binary, so old shims and new binaries detect mismatch instead of silently misbehaving.

---

## 3. Core abstractions

Five abstractions earn their place in v0.1. Each is justified against the alternative of a flat struct + match arm.

### 3.1 `AgentSurface` (trait)

```rust
pub trait AgentSurface: Send + Sync {
    fn agent_id(&self) -> &'static str;
    fn detect(&self, repo_root: &Path) -> bool;
    fn install(&self, ctx: &InstallContext) -> Result<InstallReport, InstallError>;
    fn uninstall(&self, repo_root: &Path, dry_run: bool)
        -> Result<Vec<PathBuf>, InstallError>;
    fn render_hook_script(&self, ctx: &InstallContext) -> String;
    fn hook_path(&self, repo_root: &Path) -> PathBuf;
    fn settings_path(&self, repo_root: &Path) -> PathBuf;
}
```

**Why a trait, not a `match` over an enum.** Each agent's install path is structurally different: Claude Code merges into a JSON file with a defined schema; Codex writes managed-block markdown into `AGENTS.md`; Cursor writes to `.cursor/rules/*.mdc`; Aider edits `.aider.conf.yml`. These don't share helper code or state. An enum with a giant match arm in `install()` would make every new agent a touch on existing code, fail open-closed, and prevent third-party agent surfaces.

**Plugin readiness.** The trait is `Send + Sync` and object-safe. v0.3 plugins ship binaries that implement `AgentSurface` over a subprocess protocol; the registry that owns surfaces accepts both built-in (`Box::new(ClaudeCodeSurface)`) and discovered-at-runtime impls.

### 3.2 `CheckSource` (trait)

```rust
pub trait CheckSource: Send + Sync {
    fn source_id(&self) -> &'static str;
    fn supports_config(&self, config: &CheckConfig) -> bool;
    fn run(&self, config: &CheckConfig, state: &RepoState)
        -> Result<CheckResult, anyhow::Error>;
}
```

v0.1 ships exactly one impl: `Shell`. The trait is right anyway because v0.2 adds **named recipes** — `pre-commit` (knows pre-commit's stage flags and `--from-ref` semantics), `fallow` (knows the audit JSON schema), `pytest` (parses xdist output) — and v0.3 adds **subprocess plugins** that speak a defined protocol.

**Alternative:** a `Check { kind: CheckKind, command: Option<String>, recipe: Option<String> }` struct with `match kind { ... }` in `run()`. This collapses every execution strategy into one function and prevents shipping plugin crates separately. The trait wins because a v0.3 plugin author needs to depend on `klasp-core` and implement `CheckSource` — without the trait, they'd need to fork klasp.

### 3.3 `GateProtocol` (versioned schema)

```rust
pub const GATE_SCHEMA_VERSION: u32 = 1;

pub struct GateProtocol;

impl GateProtocol {
    pub fn parse(stdin: &str) -> Result<GateInput, GateError>;
    pub fn check_schema_env(env_value: u32) -> Result<(), GateError>;
}

#[derive(Deserialize)]
pub struct GateInput {
    pub tool_name: String,
    pub tool_input: ToolInput,
}
```

The wire-protocol version is **separate from klasp's semver**. The hook script is generated once and committed to the repo. The `klasp` binary is upgraded independently. A user installing klasp 0.1, then upgrading to 0.2 without re-running `klasp install`, must not get silent wrong behaviour.

**Why an env var, not a JSON field.** The shim exports `KLASP_GATE_SCHEMA=1` before calling `klasp gate`. The binary reads it from the environment, not from `tool_input`. The agent never controls the env var, so an agent that put `schema_version: 99` into its tool input cannot force a fail-open path.

**Mismatch behaviour.** If `KLASP_GATE_SCHEMA` differs from the binary's `GATE_SCHEMA_VERSION`, the gate emits a one-line stderr notice (`"klasp-gate: schema mismatch (script=1, binary=2). Re-run 'klasp install' to update the hook. Failing open."`) and exits 0. Fail-open on every tooling error is non-negotiable; a broken gate must never wedge legitimate work.

### 3.4 `Verdict` (3-tier enum)

```rust
pub enum Verdict {
    Pass,
    Warn { findings: Vec<Finding>, message: Option<String> },
    Fail { findings: Vec<Finding>, message: String },
}
```

Three tiers, not two: `Warn` is the gradient that lets new checks roll out without immediately blocking commits the day they turn on. The structured `Vec<Finding>` carries `{rule, message, file, line, severity}` so the block message rendered to Claude's stderr is actionable rather than a raw JSON dump.

**Alternative:** a `bool` (pass/fail). Rejected because Warn is genuinely needed for staged rollouts. **Alternative 2:** a `Verdict { score: f64 }` per [SonarQube]. Rejected because checks rarely return continuous scores, and tier semantics (block vs notice vs pass) is the actual decision the runtime makes.

### 3.5 `ConfigV1` (versioned config)

```rust
pub const CONFIG_VERSION: u32 = 1;

#[derive(Deserialize)]
pub struct ConfigV1 {
    pub version: u32,
    pub gate: GateConfig,
    #[serde(default)]
    pub checks: Vec<CheckConfig>,
}
```

Every `klasp.toml` declares `version = 1` at the top. When v2 arrives, the parser fails fast with a clear "this config is for klasp 0.5+, you're on 0.2" message rather than silently ignoring new sections. `CheckSourceConfig` is `#[serde(tag = "type")]`-tagged so unknown source types are also caught at parse time.

This sets up multi-version compatibility from day one without needing it yet.

---

## 4. Module layout

```
klasp-core/
├── src/
│   ├── lib.rs
│   ├── config.rs       # ConfigV1, CheckConfig, TriggerConfig
│   ├── verdict.rs      # Verdict enum, Finding, VerdictPolicy
│   ├── protocol.rs     # GateProtocol, GateInput, GATE_SCHEMA_VERSION
│   ├── surface.rs      # AgentSurface trait, InstallContext, InstallReport
│   ├── source.rs       # CheckSource trait, CheckResult, RepoState
│   ├── trigger.rs      # Trigger pattern matching (git commit/push regex)
│   ├── error.rs        # KlaspError hierarchy via thiserror
│   └── render.rs       # terminal output, block-message formatting
│
klasp-agents-claude/
├── src/
│   ├── lib.rs
│   ├── surface.rs      # ClaudeCodeSurface impl
│   ├── settings.rs     # surgical settings.json merge
│   └── hook_template.rs# const-string template for klasp-gate.sh
│
klasp/
├── src/
│   ├── main.rs
│   ├── cli.rs          # clap definitions
│   ├── cmd/
│   │   ├── mod.rs
│   │   ├── gate.rs     # the hot path
│   │   ├── install.rs
│   │   ├── uninstall.rs
│   │   ├── doctor.rs
│   │   └── init.rs
│   └── sources/
│       ├── mod.rs
│       └── shell.rs    # Shell CheckSource impl (v0.1's only source)
└── tests/
    ├── install_claude_code.rs
    ├── gate_flow.rs
    ├── protocol_contract.rs
    └── fixtures/
        ├── claude_commit_hook.json
        └── klasp-gate-v1.sh
```

Target LOC for v0.1: **1800-2200**. The abstractions add ~450 LOC over a flat single-crate design. That cost buys ~2000 LOC saved across v0.2 (Codex), v0.3 (plugin model), and v1.0 (stable schema).

---

## 5. Install flow

`klasp install [--agent <name>] [--dry-run] [--force]`

```rust
pub fn run(args: &InstallArgs) -> Result<()> {
    let repo_root = git::find_repo_root(&args.repo_root)?;
    let config = ConfigV1::load(&repo_root)?;

    let registry = SurfaceRegistry::default(); // ClaudeCodeSurface pre-registered
    let surfaces = registry.iter()
        .filter(|s| args.agent.as_ref().map_or(true, |a| s.agent_id() == a))
        .filter(|s| args.force || s.detect(&repo_root))
        .collect::<Vec<_>>();

    if surfaces.is_empty() {
        bail!("no matching agent surfaces detected; use --force to install anyway");
    }

    let ctx = InstallContext {
        repo_root: repo_root.clone(),
        dry_run: args.dry_run,
        force: args.force,
        schema_version: GATE_SCHEMA_VERSION,
    };

    let reports: Vec<InstallReport> = surfaces.iter()
        .map(|s| s.install(&ctx).context(format!("installing {}", s.agent_id())))
        .collect::<Result<_>>()?;

    render::install_reports(&reports, args.dry_run);
    Ok(())
}
```

Inside `ClaudeCodeSurface::install`:

1. Compute paths (`.claude/hooks/klasp-gate.sh`, `.claude/settings.json`)
2. Render the hook script via `render_hook_script(ctx)` — the script is a 3-line shim that exports `KLASP_GATE_SCHEMA=1` and `exec klasp gate "$@"`, with a `# klasp:managed` marker comment near the top
3. **Idempotency check:** if the hook file exists and contains the marker, return `InstallReport { already_installed: true, .. }`. If it exists *without* the marker, return `MarkerConflict` unless `--force`
4. If `--dry-run`, return the rendered script as preview without writing
5. Write the script, `chmod 0o755`, then call `settings::merge_hook_entry`
6. The merge logic: load `.claude/settings.json` as `serde_json::Value`, walk to `hooks.PreToolUse`, find or create the `Bash` matcher entry, push klasp's hook command **only if not already present** (idempotency), serialize back preserving every other field

The settings merge is the highest-risk function in v0.1. Sibling hooks (fallow's, the user's, anyone else's) must survive. Test fixtures include real `.claude/settings.json` files from production projects to verify preservation.

---

## 6. Gate flow

`klasp gate` — called by the bash shim with Claude Code's tool-call JSON on stdin.

```rust
pub fn run(args: &GateArgs) -> Result<ExitCode> {
    // 1. Schema handshake — env var, not stdin
    let schema_env: u32 = std::env::var("KLASP_GATE_SCHEMA")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(0);

    // 2. Parse stdin (fail-open on parse error)
    let stdin = io::read_to_string(io::stdin())?;
    let input = match GateProtocol::parse(&stdin) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("klasp-gate: could not parse input ({e}), skipping.");
            return Ok(ExitCode::SUCCESS);
        }
    };

    // 3. Schema mismatch — fail-open with notice
    if let Err(e) = GateProtocol::check_schema_env(schema_env) {
        eprintln!("klasp-gate: schema mismatch ({e}). Re-run `klasp install`.");
        return Ok(ExitCode::SUCCESS);
    }

    // 4. Trigger classification
    let command = match &input.tool_input.command {
        Some(c) => c,
        None => return Ok(ExitCode::SUCCESS),
    };
    let event = match Trigger::classify(command) {
        Some(e) => e,
        None => return Ok(ExitCode::SUCCESS), // not git commit/push, pass through
    };

    // 5. Load config (fail-open on missing/parse error)
    let repo_root = git::find_repo_root_from_cwd()?;
    let config = match ConfigV1::load(&repo_root) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("klasp-gate: config error ({e}), skipping.");
            return Ok(ExitCode::SUCCESS);
        }
    };

    let repo_state = RepoState { root: repo_root, git_event: event };
    let source_registry = SourceRegistry::default(); // Shell pre-registered

    // 6. Run checks
    let mut results: Vec<CheckResult> = Vec::new();
    for check in &config.checks {
        if !check.triggers_match(event) { continue; }
        let source = match source_registry.find_for(check) {
            Some(s) => s,
            None => {
                eprintln!("klasp-gate: no source for check '{}', skipping.", check.name);
                continue;
            }
        };
        match source.run(check, &repo_state) {
            Ok(result) => results.push(result),
            Err(e) => eprintln!(
                "klasp-gate: check '{}' runtime error ({e}), skipping.",
                check.name,
            ),
        }
    }

    // 7. Aggregate
    let verdicts: Vec<Verdict> = results.iter().map(|r| r.verdict.clone()).collect();
    let final_verdict = Verdict::merge(verdicts, config.gate.policy.clone());

    if final_verdict.is_blocking() {
        render::block_message(&final_verdict, &results);
        return Ok(ExitCode::from(2));
    }

    if matches!(final_verdict, Verdict::Warn { .. }) {
        render::warn_message(&final_verdict);
    }

    Ok(ExitCode::SUCCESS)
}
```

The trigger regex (in `trigger.rs`) mirrors fallow's pattern, ported to the `regex` crate and compiled once via `OnceLock`:

```
(?:^|[\s;|&()])git\s+(?:commit|push)(?:\s|$)
```

Edge cases the regex deliberately misses (and the design accepts): `bash -c "git push"`, `eval "git commit"`, env-prefixed `GIT_DIR=... git push`, aliases like `gp`. The threat model is **honest agents we want to help**, not adversarial ones — the gate is best-effort, not a security boundary. Adversarial inputs can bypass it trivially (the agent could `bash -c "$(echo Z2l0... | base64 -d)"`); anyone treating klasp as a security boundary is misusing it.

---

## 7. Schema versioning

The hook script committed to the repo:

```bash
#!/usr/bin/env bash
# klasp:managed v1 — generated by `klasp install`. Do not edit; re-run install instead.
export KLASP_GATE_SCHEMA=1
exec klasp gate "$@"
```

The `klasp-core` crate declares `pub const GATE_SCHEMA_VERSION: u32 = 1;`. The gate runtime reads `KLASP_GATE_SCHEMA` from the environment (set by the shim) and compares against the binary's `GATE_SCHEMA_VERSION`.

**Three upgrade scenarios, all safe:**

| Scenario | Script | Binary | Behaviour |
|---|---|---|---|
| Same version (normal) | `KLASP_GATE_SCHEMA=1` | `GATE_SCHEMA_VERSION=1` | Gate runs as designed. |
| Binary upgraded, script stale | `KLASP_GATE_SCHEMA=1` | `GATE_SCHEMA_VERSION=2` | Stderr notice, exit 0. User runs `klasp install` to update the script. |
| Script ahead of binary (downgrade) | `KLASP_GATE_SCHEMA=2` | `GATE_SCHEMA_VERSION=1` | Stderr notice, exit 0. User upgrades the binary. |

The version is a **monotone integer**, not semver. Schema bumps happen when the wire protocol changes — adding required fields, renaming verdict tiers, changing the JSON schema for findings. Binary releases bump semver freely; the schema only bumps when truly necessary.

A contract test in `klasp/tests/protocol_contract.rs` reads the golden fixture script in `tests/fixtures/klasp-gate-v1.sh`, parses its `KLASP_GATE_SCHEMA` export, and asserts equality with `GATE_SCHEMA_VERSION`. When a developer bumps the constant, this test fails until they ship a new fixture — forcing the conversation about backward compatibility.

---

## 8. Plugin model lookahead (v0.3)

Plugins are separate binaries named `klasp-plugin-<name>`, depending on `klasp-core`. The main `klasp` CLI discovers them by scanning `$PATH` for the prefix at startup. Each plugin satisfies one of:

- `AgentSurface` (e.g. `klasp-plugin-jules` for Jules)
- `CheckSource` (e.g. `klasp-plugin-pre-commit` for native pre-commit integration)

**Subprocess protocol (sketch):**

```rust
pub struct SubprocessPlugin { pub binary: PathBuf }

impl CheckSource for SubprocessPlugin {
    fn run(&self, config: &CheckConfig, state: &RepoState)
        -> Result<CheckResult, anyhow::Error>
    {
        let payload = serde_json::to_vec(&PluginRequest { config, state })?;
        let output = Command::new(&self.binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?
            .wait_with_input(&payload)?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }
}
```

The plugin protocol uses its own `PLUGIN_PROTOCOL_VERSION` constant — separate from `GATE_SCHEMA_VERSION` — so plugin upgrade and gate upgrade evolve independently.

This is **not implemented in v0.1**. The trait shape is the v0.1 commitment; the protocol is v0.3 work. The point of describing it now is to demonstrate the trait is right.

---

## 9. Distribution

| Channel | Package | Mechanism |
|---|---|---|
| **cargo** | `klasp` (binary crate) | `cargo install klasp` builds from source. Fastest path for Rust devs. |
| **npm** | `@klasp-dev/klasp` (main) + `@klasp-dev/klasp-<platform>-<arch>` (per-platform) | Biome-style ~20-line JS shim using `optionalDependencies` + `require.resolve`. No install-time download — npm's tarball integrity is the trust mechanism. |
| **PyPI** | `klasp` | maturin wheel, one per platform tag (`klasp-0.1.0-py3-none-macosx_11_0_arm64.whl`). `[tool.maturin] bindings = "bin"` — no PyO3, just the binary. |

**Platform matrix for v0.1:**
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-pc-windows-msvc`

Five platforms cover ~98% of users at the v0.1 scale. musl, win-arm64, freebsd are deferred until somebody files an issue.

**Release pipeline.** GitHub Actions matrix on tag push: build per-platform binary, upload artifact, then a publish job downloads all artifacts, stages them into the npm sub-packages and the PyPI wheel directories, and publishes in order: per-platform npm packages → main npm package (so `optionalDependencies` resolve) → PyPI wheels → cargo publish → GitHub Release.

---

## 10. Testing

### Unit tests (per-crate, `#[cfg(test)]`)

- `klasp-core/src/config.rs`: parse minimal TOML, parse with all optional fields, error on `version = 2`, error on missing required fields.
- `klasp-core/src/verdict.rs`: exit-code mapping, `Verdict::merge` aggregation under each `VerdictPolicy`, `is_blocking()` invariants.
- `klasp-core/src/protocol.rs`: `parse()` succeeds on real Claude payloads, fails on malformed JSON, fails on missing `tool_input`.
- `klasp-core/src/trigger.rs`: regex matches `git commit`, `git push`, `git -c x=y commit` (currently fails — documented), `&& git push`, rejects `forgit commit`, `mygit push`, `git committed` (hypothetical).
- `klasp-agents-claude/src/settings.rs`: merge into empty object, merge preserving existing PreToolUse entries (fixture: real fallow settings.json), merge idempotency, merge with malformed input.

### Integration tests (`klasp/tests/`)

- `install_claude_code.rs`: temp dir + `.git/`, run `ClaudeCodeSurface::install`, assert script exists with correct `KLASP_GATE_SCHEMA`, assert `settings.json` has the right entry, assert second install is no-op.
- `gate_flow.rs`: spawn `klasp gate` with synthetic Claude payload on stdin, assert exit 2 on `Fail`, exit 0 on `Pass`/`Warn`/`Error`.
- `protocol_contract.rs`: parse fixture script's `KLASP_GATE_SCHEMA` export, assert equality with `GATE_SCHEMA_VERSION`.

### Mock-based tests (trait surface)

`klasp-core` provides `MockAgentSurface` and `MockCheckSource` behind `#[cfg(any(test, feature = "test-utils"))]`. Installer tests verify the orchestration logic (registry, filtering, dry-run) without filesystem side effects. Gate tests verify aggregation policies without forking subprocesses.

### Snapshot tests

`insta` snapshot of the rendered hook script. When the template changes, the developer reviews the diff explicitly. Prevents accidental script content drift.

---

## 11. Trade-offs and honest cost accounting

The clean-abstractions design pays for itself in five concrete places. Here's where v0.1 takes a hit so v0.2 and beyond don't:

1. **`AgentSurface` trait — ~150 LOC over an enum + match.** Pays off the day Codex lands as a new crate, zero changes to existing code.

2. **`CheckSource` trait — ~100 LOC over a `Check.kind` enum.** Pays off in v0.2 when named recipes ship and v0.3 when subprocess plugins ship. The Shell impl in v0.1 is fully testable in isolation.

3. **`GateProtocol` schema versioning — ~80 LOC** (constant, env read, mismatch check, contract test, fixture). A v0.1-only tool would skip this. Pays off the first time a user upgrades klasp without re-running `klasp install` and gets a clear message instead of silent wrong behaviour.

4. **3-crate workspace — ~1 day of setup friction.** Workspace manifest, three `Cargo.toml` files, cross-crate import paths. Pays off in v0.3 when plugin authors depend on `klasp-core` without pulling in the binary or Claude impl.

5. **`ConfigV1` with `version` field and `#[serde(tag = "type")]` enums — strict failure mode.** Typos in `klasp.toml` produce hard parse errors instead of silently being ignored. More friction for early users. Pays off when v0.2 ships `Recipe` as a new source type and v0.1 users get a clear "upgrade klasp" message.

**Total extra LOC vs minimal MVP: ~450.** Total v0.1 LOC: ~1800-2200. Within the 2500-line ceiling the architect set.

---

## 12. External crates

| Crate | Why |
|---|---|
| `clap` (derive) | CLI parsing. Derive over builder for documentation-as-types. |
| `serde` + `serde_json` + `toml` | Config and protocol (de)serialization. |
| `thiserror` + `anyhow` | Typed errors at module boundaries (`thiserror`), ergonomic propagation in CLI (`anyhow`). |
| `tracing` | Structured logging on the gate path. `RUST_LOG=debug` for diagnosis. |
| `regex` | Trigger pattern. Compiled once via `OnceLock`. |
| `tempfile` | Integration tests; atomic writes for `settings.json`. |
| `which` | Binary detection in `AgentSurface::detect` and gate runner resolution. |
| `insta` | Snapshot tests for the generated script. |

No async runtime. The gate is sequential `Command::output()`. No HTTP client. No global state. All crates are mature and minimally maintained.

---

## 13. What v0.1 explicitly does not include

- **Codex / AGENTS.md surface** — v0.2.
- **Cursor / Aider surfaces** — v0.3.
- **Named recipes** (`type = "pre_commit"`, `type = "fallow"`) — v0.2.
- **Subprocess plugins** — v0.3 / v1.0.
- **Parallel check execution** — v0.2 (with `tokio` or `rayon`).
- **Hosted runtime / team rollups** — v1.0+.
- **Telemetry of any kind.** Never. Dev tools that phone home get torched on day one.
- **A `klasp run` that bypasses agents and just runs the checks** — useful but out of scope; users have `pre-commit run` for that.
- **Auto-fix capabilities** — out of scope, intentional. klasp gates; it doesn't write code.

---

## 14. Open questions / known gaps

- **Monorepo config discovery.** v0.1 looks for `klasp.toml` at `$CLAUDE_PROJECT_DIR` then `cwd()`. A monorepo with per-package configs needs a richer resolution strategy. v0.2 will need to address this when the integration test fixture for monorepos lands.
- **Windows path handling in the bash shim.** The shim runs under Git for Windows' bash. Forward-slash paths in `settings.json`, but the Rust binary handles platform paths internally. Audit during week 3.
- **`verdict_path` is dot-notation, not full JSONPath.** `.verdict` works, `.results[0].verdict` does not. Acceptable v0.1 limitation; v0.2 swaps to a real JSON pointer library if anyone hits it.
- **Settings.json roundtrip preservation.** `serde_json::Value` normalises key order. Real `.claude/settings.json` files may have keys in a specific order users care about. Test against real fixtures and see if anyone complains.

---

## 15. Reference implementations

- **fallow-rs/fallow** — the prior art for `setup-hooks`. Read the generated `fallow-gate.sh` for the canonical bash pattern; klasp's shim is intentionally thinner because logic lives in the binary.
- **biomejs/biome** — the pattern for npm distribution (`@biomejs/biome` main + `@biomejs/cli-<platform>` optional deps). Klasp mirrors this exactly.
- **astral-sh/ruff** — the pattern for PyPI distribution via maturin with `bindings = "bin"`.

---

## 16. Document conventions

This document uses Rust pseudocode where signatures are load-bearing for the design. The actual implementation will diverge in surface details (error type imports, lifetime annotations, derives) but must preserve the contracts described here. Where the design names a specific exit code, regex, env var name, or JSON path, those are commitments — changing them is a `GATE_SCHEMA_VERSION` bump.

Discussion happens on GitHub issues. Major design changes go through an `RFC-NNNN.md` PR in `docs/rfcs/` (a directory that doesn't exist yet — created when needed).
