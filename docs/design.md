# klasp v0.1 — Design

> **Status:** v0.1 implementation merged on `main` at [`234908e`](https://github.com/klasp-dev/klasp/commit/234908e) (PR [#17](https://github.com/klasp-dev/klasp/pull/17), W6-7). This document remains the build-target reference; deviations from the implementation that surfaced during W1-W7 are noted inline below where relevant. For the deferred items intentionally pushed to later milestones, see [`roadmap.md`](./roadmap.md).

klasp v0.1 commits to five core abstractions: `AgentSurface`, `CheckSource`, `GateProtocol`, `Verdict`, and `ConfigV1`. The implementation accepts ~450 LOC of overhead to buy a plugin-ready design that won't break existing users when v0.3 lands. This document explains each choice and names the alternatives rejected. For the milestone-by-milestone shape from v0.1 → v1.0, see [`roadmap.md`](./roadmap.md).

> **Implementation status.** All three distribution shells described below shipped as designed: the 3-crate Cargo workspace (`klasp-core`, `klasp-agents-claude`, `klasp`), the biome-style npm shim with per-platform sub-packages under `optionalDependencies`, and maturin-based PyPI wheels. The original `0.0.0` name-reservation publishes (single-crate Cargo manifest, plain npm package, Hatchling PyPI build) are superseded by the wiring landed in [W5 (PR #15)](https://github.com/klasp-dev/klasp/pull/15) and validated by the W6-7 dogfood. Items deferred to v0.2+ are tracked in [`roadmap.md`](./roadmap.md). Implementation notes that diverge from this design are flagged inline in the sections below and consolidated in [§17](#17-key-implementation-notes-w1-w7).

---

## 1. Problem

AI coding agents (Claude Code, Codex, Cursor, Aider, …) commit and push code on a developer's behalf. Every team running them at scale has the same complaint: **the agent ships work that fails the same checks a human would have hit at `git commit`** — `pre-commit`, `eslint`, `cargo clippy`, the project's test suite, a custom audit script. The agent doesn't know about those gates, the gates don't know about the agent, and the failure mode is "CI catches it three minutes later, the agent has already moved on, and the developer is reviewing a broken PR."

The natural response is *"just use git pre-commit hooks"*. Three things are wrong with that:

1. **Git hooks are bypassable.** `git commit --no-verify` exists, and an agent that's trained to be helpful when commits fail will absolutely use it.
2. **Git hooks fire too late.** By the time the hook runs, the agent has already typed the command. The agent's tool-call surface — Claude Code's `PreToolUse` hook (the callback Claude Code fires before executing any shell command) — fires *before* the shell ever sees `git`. That's the point of intervention where a structured "blocked, here's why" reply makes the agent self-correct rather than retry-with-no-verify.
3. **Git hooks don't ride along with clones.** A fresh worktree, a remote agent, a CI runner, a teammate's machine — none of them inherit `.git/hooks/`. They inherit `.claude/`, `AGENTS.md`, and the project's `klasp.toml` from the working tree.

Not every agent exposes a programmatic gate. **Claude Code does** (PreToolUse). **Codex does not** — its `AGENTS.md` is plain Markdown context, read by the model but not enforced by the runtime. v0.2's Codex support compensates by installing `git pre-commit` / `pre-push` hooks for actual enforcement, with the AGENTS.md managed block as the model-side advisory layer.

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

The runtime topology at the user's machine is a thin bash shim that exec's the Rust binary. The shim exists for **auditability** (a human reviewing the repo can see exactly what gets executed without trusting an opaque binary path in `.claude/settings.json`) and for the **schema-version handshake** (detailed in §3.3): the shim exports `KLASP_GATE_SCHEMA=N` before exec'ing the binary, so old shims and new binaries detect mismatch instead of silently misbehaving. The full topology:

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
    fn source_id(&self) -> &str;
    fn supports_config(&self, config: &CheckConfig) -> bool;
    fn run(&self, config: &CheckConfig, state: &RepoState)
        -> Result<CheckResult, anyhow::Error>;
}
```

Note that `source_id` returns `&str` (tied to `&self`'s lifetime), not `&'static str` — v0.3's subprocess plugins are discovered at runtime and have dynamic IDs (the binary's filename), which a `'static` return type cannot express.

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

#[derive(Deserialize)]
pub struct ToolInput {
    pub command: Option<String>,
}
```

The wire-protocol version is **separate from klasp's semver**. The hook script is generated once and committed to the repo. The `klasp` binary is upgraded independently. A user installing klasp 0.1, then upgrading to 0.2 without re-running `klasp install`, must not get silent wrong behaviour.

**Why an env var, not a JSON field.** The shim exports `KLASP_GATE_SCHEMA=1` before calling `klasp gate`. The binary reads it from the environment, not from `tool_input`. This specifically defends against the **JSON-injection** attack: an agent that put `schema_version: 99` into its tool input cannot force a fail-open path, because the binary never reads schema from stdin. It does **not** defend against an agent that overwrites `klasp-gate.sh` directly — a fully adversarial agent can trivially do that, and klasp's threat model (§6) already accepts this. The env-var design buys protection against the cheap attack, not the expensive one.

**Mismatch behaviour.** If `KLASP_GATE_SCHEMA` differs from the binary's `GATE_SCHEMA_VERSION`, the gate emits a one-line stderr notice (`"klasp-gate: schema mismatch (script=1, binary=2). Re-run 'klasp install' to update the hook. Failing open."`) and exits 0. Fail-open on every tooling error is non-negotiable; a broken gate must never wedge legitimate work.

### 3.4 `Verdict` (3-tier enum)

```rust
#[derive(Debug, Clone)]
pub enum Verdict {
    Pass,
    Warn { findings: Vec<Finding>, message: Option<String> },
    Fail { findings: Vec<Finding>, message: String },
}
```

Three tiers, not two: `Warn` is the gradient that lets new checks roll out without immediately blocking commits the day they turn on. The structured `Vec<Finding>` carries `{rule, message, file, line, severity}` so the block message rendered to Claude's stderr is actionable rather than a raw JSON dump. `Finding` derives `Clone` for the same reason — verdicts are aggregated and rendered, which requires copies.

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

A real `klasp.toml` looks like this:

```toml
version = 1

[gate]
agents = ["claude_code"]
policy = "any_fail"

[[checks]]
name = "ruff"
triggers = [{ on = ["commit"] }]
[checks.source]
type = "shell"
command = "ruff check ."

[[checks]]
name = "pytest"
triggers = [{ on = ["push"] }]
timeout_secs = 120
[checks.source]
type = "shell"
command = "pytest -q"
```

Every shell check sees `KLASP_BASE_REF` in its env (set to the merge-base ref klasp computed) so checks can scope themselves to the diff. Future versions add `[[trigger]]` blocks for non-git triggers and `[plugin]` sections for v0.3 subprocess plugins; v0.1 fails parsing on those sections, guiding the user to upgrade.

This sets up multi-version compatibility from day one without needing it yet.

> **Implementation note (W6-7).** The shipped `CheckConfig` struct in `klasp-core/src/config.rs` has four fields — `name`, `triggers`, `source`, `timeout_secs` — and intentionally **does not** ship a `verdict_path` field. v0.1's only `CheckSource` (`Shell`) maps the child's exit code to `Verdict::Pass | Fail` directly; the `verdict_path` design originally implied for parsing recipe-tool JSON is deferred to v0.2 named recipes (see [`recipes.md` §What's next](./recipes.md#whats-next)). `KLASP_BASE_REF` is wired through the `RepoState::base_ref` field rather than computed inside `Shell::run`, so the v0.2 named recipes (`pre_commit`, `fallow`, `pytest`, `cargo`) can read the merge-base off the same struct without re-implementing the resolution logic.

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

    // 7. Aggregate (Verdict derives Clone — see §3.4)
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

The trigger regex (in `trigger.rs`) is a Rust port of fallow's POSIX ERE pattern. Functionally equivalent — same edge cases — but compiled once via `OnceLock`:

```
(?:^|[\s;|&()])git\s+(?:commit|push)(?:\s|$)
```

Edge cases the regex deliberately misses (and the design accepts): `bash -c "git push"`, `eval "git commit"`, env-prefixed `GIT_DIR=... git push`, aliases like `gp`. The threat model is **honest agents we want to help**, not adversarial ones — the gate is best-effort, not a security boundary. Adversarial inputs can bypass it trivially (the agent could `bash -c "$(echo Z2l0... | base64 -d)"`); anyone treating klasp as a security boundary is misusing it.

The gate is **synchronous, no async runtime**. v0.1 runs checks sequentially via `Command::output()`. v0.2 will add parallel execution via `rayon` (chosen over `tokio` to keep the gate runtime free of an async runtime dependency).

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

A contract test in `klasp/tests/protocol_contract.rs` does two things: parses the golden fixture script in `tests/fixtures/klasp-gate-v1.sh` for its `KLASP_GATE_SCHEMA` export and asserts equality with `GATE_SCHEMA_VERSION`, **and** invokes `ClaudeCodeSurface::render_hook_script` and asserts the output also contains `KLASP_GATE_SCHEMA={GATE_SCHEMA_VERSION}`. Both must agree, so a developer who bumps the constant cannot satisfy the test by editing only the fixture.

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
        let mut child = Command::new(&self.binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        child.stdin.as_mut().unwrap().write_all(&payload)?;
        let output = child.wait_with_output()?;
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
| **npm** | `@klasp-dev/klasp` (main) + `@klasp-dev/klasp-<platform>-<arch>` (per-platform) | Biome-style ~20-line JS shim. See prose below. |
| **PyPI** | `klasp` | maturin wheel, one per platform tag. See prose below. |

**npm: how `optionalDependencies` avoids install-time downloads.** The main `@klasp-dev/klasp` package declares each per-platform sub-package (`@klasp-dev/klasp-darwin-arm64`, etc.) under `optionalDependencies`. Each sub-package's own `package.json` sets `os` and `cpu` fields. npm resolves only the sub-package matching the installer's machine and skips the rest. The main package's `bin/klasp.js` shim is a ~20-line script that uses `require.resolve('@klasp-dev/klasp-<platform>-<arch>')` to find the binary in the resolved sub-package and `child_process.spawnSync`s it. **No install-time network fetch** — the binary arrives as a verified npm tarball with the registry's standard SHA integrity check.

**PyPI: how maturin builds platform wheels.** `pyproject.toml` declares `[build-system] build-backend = "maturin"` and `[tool.maturin] bindings = "bin"`. CI runs maturin once per target platform; each run produces a wheel like `klasp-0.1.0-py3-none-macosx_11_0_arm64.whl`. The wheel contains the binary in its `<distname>.data/scripts/` directory. pip's standard wheel-tag resolution picks the right one for the user's machine. No PyO3, no Python code — the wheel exists purely to deliver the binary into a venv's `bin/`.

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
- `protocol_contract.rs`: parses fixture script's `KLASP_GATE_SCHEMA` AND invokes `render_hook_script()`; asserts both equal `GATE_SCHEMA_VERSION`. See §7.

### Mock-based tests (trait surface)

`klasp-core` provides `MockAgentSurface` and `MockCheckSource` behind `#[cfg(any(test, feature = "test-utils"))]`. Installer tests verify the orchestration logic (registry, filtering, dry-run) without filesystem side effects. Gate tests verify aggregation policies without forking subprocesses.

### Snapshot tests

`insta` snapshot of the rendered hook script. When the template changes, the developer reviews the diff explicitly. Prevents accidental script content drift.

---

## 11. Trade-offs and honest cost accounting

The clean-abstractions design pays for itself in five concrete places. Each is rationalised in §3 — this section is the LOC bottom line.

| Cost | Approx. LOC | Pays off at |
|---|---|---|
| `AgentSurface` trait | ~150 | v0.2 — Codex lands as a new crate, zero changes to existing code |
| `CheckSource` trait | ~100 | v0.2 — named recipes ship as new impls; v0.3 — plugins |
| `GateProtocol` schema versioning | ~80 | First binary upgrade post-install: clear message instead of silent wrong behaviour |
| 3-crate workspace | ~1 day setup friction | v0.3 — plugin authors depend on `klasp-core` cleanly |
| `ConfigV1` `version` field + `#[serde(tag)]` | ~20 (config side) + strict failure | v0.2 — old binaries reject unknown source types loudly |

**Total extra LOC vs minimal MVP: ~450.** Total v0.1 LOC: ~1800-2200.

---

## 12. External crates

| Crate | Type | Why |
|---|---|---|
| `clap` (derive) | runtime | CLI parsing. Derive over builder for documentation-as-types. |
| `serde` + `serde_json` + `toml` | runtime | Config and protocol (de)serialization. |
| `thiserror` + `anyhow` | runtime | Typed errors at module boundaries (`thiserror`), ergonomic propagation in CLI (`anyhow`). |
| `tracing` | runtime | Structured logging on the gate path. `RUST_LOG=debug` for diagnosis. |
| `regex` | runtime | Trigger pattern. Compiled once via `OnceLock`. |
| `which` | runtime | Binary detection in `AgentSurface::detect` and gate runner resolution. |
| `tempfile` | runtime + dev | Atomic writes for `settings.json` (production) and temp dirs in integration tests. |
| `insta` | dev only | Snapshot tests for the generated script. |

No HTTP client, no global state. All crates are mature and minimally maintained.

---

## 13. What v0.1 explicitly does not include

Versioned scope (Codex, named recipes, parallel execution, Cursor/Aider, plugins, hosted runtime) and permanent non-goals (telemetry, auto-fix, security-boundary semantics) are listed in [`roadmap.md`](./roadmap.md). The one v0.1-specific deferred item not in the roadmap: **a `klasp run` subcommand** that runs the configured checks from the CLI without involving an agent. Useful, but `pre-commit run` covers the same use case and adding it now bloats the v0.1 surface for marginal value. Reconsider in v0.2.

---

## 14. Open questions / known gaps

- **Monorepo config discovery.** v0.1 looks for `klasp.toml` at `$CLAUDE_PROJECT_DIR` then `cwd()`. A monorepo with per-package configs needs a richer resolution strategy. v0.2 will need to address this when the integration test fixture for monorepos lands.
- **Windows path handling in the bash shim.** The shim runs under Git for Windows' bash. Forward-slash paths in `settings.json`, but the Rust binary handles platform paths internally. Audit during week 3.
- **`verdict_path` is deferred to v0.2 (not in v0.1).** The originally-anticipated dot-notation `verdict_path` field on `CheckConfig` is not part of the shipped v0.1 schema — v0.1's only source (`Shell`) maps exit code to `Verdict::Pass | Fail` directly. v0.2's named recipes (`type = "pre_commit"`, `type = "fallow"`, etc.) parse tool-specific output formats internally; if a generic `verdict_path` ever ships, it lands alongside or after named recipes. Tracked under [`roadmap.md` §v0.2](./roadmap.md#v02--codex--named-recipes-target-3-months-from-v01).
- **Settings.json roundtrip preservation.** `serde_json::Value` normalises key order. Real `.claude/settings.json` files may have keys in a specific order users care about. Test against real fixtures and see if anyone complains.

---

## 15. Reference implementations

- **fallow-rs/fallow** — the prior art for `setup-hooks`. Read the generated `fallow-gate.sh` for the canonical bash pattern; klasp's shim is intentionally thinner because logic lives in the binary.
- **biomejs/biome** — the pattern for npm distribution (`@biomejs/biome` main + `@biomejs/cli-<platform>` optional deps). Klasp mirrors this exactly.
- **astral-sh/ruff** — the pattern for PyPI distribution via maturin with `bindings = "bin"`.
- **klasp itself** — the canonical v0.1 `klasp.toml` lives at [`/klasp.toml`](../klasp.toml) and runs `cargo check` + `cargo clippy -D warnings` on every commit attempt and `cargo test --workspace` on every push. The repo's `.claude/hooks/klasp-gate.sh` and `.claude/settings.json` are tracked in git so worktrees and contributor checkouts inherit the install. See [W6-7 (PR #17)](https://github.com/klasp-dev/klasp/pull/17).

---

## 16. Document conventions

This document uses Rust pseudocode where signatures are load-bearing for the design. The actual implementation will diverge in surface details (error type imports, lifetime annotations, derive macros) but must preserve the contracts described here. Where the design names a specific exit code, regex, env var name, or JSON path, those are commitments — changing them is a `GATE_SCHEMA_VERSION` bump.

Discussion happens on GitHub issues. Major design changes go through an `RFC-NNNN.md` PR in `docs/rfcs/` (a directory that doesn't exist yet — created when needed).

---

## 17. Key implementation notes (W1-W7)

The v0.1 implementation followed this design closely; the items below are the places where reality diverged enough to be worth flagging for future readers. None invalidate the architecture; all are surface-level corrections that the implementation surfaced.

- **`KLASP_BASE_REF` is carried on `RepoState`, not synthesised inside `Shell::run`.** The design (§3.5, §6) names the env var as a contract with shell checks, but doesn't say where the value originates. The shipped runtime resolves the merge-base once during gate setup, stores it as `RepoState::base_ref: String`, and `Shell::run_with_timeout` exports it onto the child env from there. This matters because v0.2's named recipes will read `state.base_ref` directly (without re-resolving) when they speak to recipe-tool-specific APIs. The fallback chain (upstream tracking branch → `origin/main` → `origin/master` → `HEAD~1`) lives in the resolution helper, not in `Shell`.

- **`verdict_path` is not in v0.1.** §3.5 originally implied a `verdict_path` field on `CheckConfig`. The shipped `CheckConfig` struct has exactly four fields (`name`, `triggers`, `source`, `timeout_secs`). The Shell source maps child exit codes to `Verdict::Pass | Fail` directly; richer parsing waits for v0.2's named recipes, which know their tool's output format internally and don't need a generic JSON-path projection. See [`recipes.md` §What's next](./recipes.md#whats-next).

- **`klasp-core` and `klasp-agents-claude` are publishable.** The 3-crate workspace (§2) was initially scoped with `klasp-core` and `klasp-agents-claude` as `publish = false` — only the `klasp` binary crate would publish. W5 (PR [#15](https://github.com/klasp-dev/klasp/pull/15)) flipped both library crates to publishable so `cargo publish` of the binary doesn't fail on missing path-dependency versions. Plugin authors targeting v0.3 will depend on `klasp-core` from crates.io as designed. No surface change; the design intent is honoured.

- **`x86_64-apple-darwin` is in the release matrix, not per-PR CI.** §9 names five v0.1 targets including darwin-x64. Per-PR CI runs four of them (the macOS-x64 runner is significantly slower and was dropped from the per-PR matrix during W3); the tag-triggered release workflow ([`release.yml`](../.github/workflows/release.yml)) builds darwin-x64 alongside the other four. Tracking issue: [#9](https://github.com/klasp-dev/klasp/issues/9) — reintroduce darwin-x64 to per-PR CI once a faster runner is available.

- **W3 follow-ups (PR [#14](https://github.com/klasp-dev/klasp/pull/14), issue [#12](https://github.com/klasp-dev/klasp/issues/12) closed).** Two W3 follow-up items landed after the W3 merge. First: a test-coverage gap on the existing source-runtime-error fail-open path. The W3 implementation in `gate.rs::run` (PR [#11](https://github.com/klasp-dev/klasp/pull/11)) already mapped `CheckSource` runtime errors to fail-open exit 0 with a stderr notice, but no end-to-end test exercised that wiring; PR #14 added the `source_runtime_error_fails_open` regression test (no behaviour change to the gate). Second: a real bug — the `Shell` source could leak its child process on timeout/interrupt; PR #14 added child-process reaping with regression coverage so killed-by-signal paths surface the signal in the finding. Documented in [`CHANGELOG.md`](../CHANGELOG.md) under the W3 follow-ups bullets.

- **W5 follow-ups (issue [#16](https://github.com/klasp-dev/klasp/issues/16) open).** Distribution wiring landed in W5 with three minor follow-ups still tracked on the issue (none blocking the v0.1.0 tag). Surface drift from the design is `none`; the items are pipeline polish.

For the milestone-by-milestone delivery record, see [`roadmap.md` §v0.1](./roadmap.md#v01--mvp-shipped-target-4-6-weeks-actual-7-weeks-w1-w7).
