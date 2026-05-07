# Plugin Authoring Guide

> **Status:** Plugin protocol v0 — experimental. APIs may change in any v0.3.x patch.
> The protocol is stable from v1.0 onward.

klasp's plugin system lets you ship quality-gate checks as standalone binaries
without touching the klasp source. A plugin is any executable named
`klasp-plugin-<name>` that speaks the v0 wire protocol.

## Quick start: fork the reference plugin

The canonical starting point is [`examples/klasp-plugin-pre-commit/`](../examples/klasp-plugin-pre-commit/):

```bash
cp -r examples/klasp-plugin-pre-commit/ ../my-klasp-plugin
cd ../my-klasp-plugin
# Edit Cargo.toml — rename package + binary to klasp-plugin-<your-name>
# Edit src/runner.rs — replace the pre-commit invocation with your check
# Edit src/main.rs — update PLUGIN_NAME and CONFIG_TYPES constants
cargo build --release
```

Put the binary on `$PATH` and add a `[[checks]]` block to `klasp.toml`:

```toml
[[checks]]
name = "my-check"
triggers = [{ on = ["commit"] }]
[checks.source]
type = "plugin"
name = "my-name"   # klasp looks for `klasp-plugin-my-name` on PATH
```

Run `klasp doctor` to confirm the binary is found.

## Protocol version

```
PLUGIN_PROTOCOL_VERSION = 0
```

Every `--describe` response and every `--gate` response must include
`"protocol_version": 0`. klasp reads the `--describe` response before each
`--gate` invocation to verify forward-compatibility. A mismatch produces a
`Verdict::Warn` (never a hard failure) so the gate degrades gracefully.

The full wire-format specification lives in [`docs/plugin-protocol.md`](./plugin-protocol.md).

## Required flags

| Flag | Behaviour |
|---|---|
| `--describe` | Print `PluginDescribe` JSON to stdout; exit 0. |
| `--gate` | Read `PluginGateInput` JSON from stdin; print `PluginGateOutput` JSON to stdout; exit 0. |

The plugin **must exit 0** in all cases — even when `verdict = "fail"`. A
non-zero exit is an infrastructure error from klasp's perspective and produces
a `Verdict::Warn` in the gate output, not a hard block.

## Wire types (copy-paste contract)

```rust
// PluginDescribe — written to stdout on --describe
pub struct PluginDescribe {
    pub protocol_version: u32,   // must be 0
    pub name: String,            // e.g. "klasp-plugin-pre-commit"
    pub config_types: Vec<String>,
    pub supports: PluginSupports,
}
pub struct PluginSupports {
    pub verdict_v0: bool,        // must be true
}

// PluginGateOutput — written to stdout on --gate
pub struct PluginGateOutput {
    pub protocol_version: u32,   // must be 0
    pub verdict: PluginVerdict,  // "pass" | "warn" | "fail"
    pub findings: Vec<PluginFinding>,
}
pub struct PluginFinding {
    pub severity: String,   // "info" | "warn" | "error"
    pub rule: String,       // e.g. "my-plugin/rule-name"
    pub file: Option<String>,
    pub line: Option<u32>,
    pub message: String,
}
```

Do not import `klasp-core` for these types — copy them verbatim. The protocol
is the contract, not the library.

## Configuring in `klasp.toml`

```toml
[[checks]]
name = "my-check"
triggers = [{ on = ["commit"] }]
timeout_secs = 30
[checks.source]
type = "plugin"
name = "my-name"        # binary: klasp-plugin-my-name
args = ["--extra-flag"] # forwarded in PluginGateInput.config.args
```

### Disabling a plugin check

Remove or comment out the `[[checks]]` block. Or set
`[gate].policy = "all_fail"` if you want the gate to pass when the plugin
warns but other checks pass.

## Naming convention

Binaries must follow the `klasp-plugin-<name>` prefix. The prefix is how
`klasp doctor` probes PATH and how klasp constructs the binary name from the
`name` field in `klasp.toml`.

## See also

- [`docs/plugin-protocol.md`](./plugin-protocol.md) — full wire-format specification
- [`examples/klasp-plugin-pre-commit/README.md`](../examples/klasp-plugin-pre-commit/README.md) — reference implementation walkthrough
- [`klasp/tests/plugin_smoke.rs`](../klasp/tests/plugin_smoke.rs) — end-to-end smoke tests
- [`klasp/tests/plugin_protocol.rs`](../klasp/tests/plugin_protocol.rs) — protocol unit tests
