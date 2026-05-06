# klasp Plugin Protocol

## Status: Experimental (v0, `PLUGIN_PROTOCOL_VERSION = 0`)

> **This protocol is explicitly unstable.** It may change in any v0.3.x release
> without a deprecation period. It graduates to `PLUGIN_PROTOCOL_VERSION = 1`
> (stable, backward-compatible) only at klasp v1.0.
>
> Plugin authors targeting v0.3 should treat their plugin as experimental
> alongside the protocol. When v1.0 ships you will get a clear migration path
> from `0` → `1`.

The plugin protocol lets third-party binaries extend klasp's check system.
Plugins are separate processes that communicate with klasp over stdin/stdout
using JSON. A `PLUGIN_PROTOCOL_VERSION = 0` plugin speaks the wire format
described in this document.

---

## Discovery

Plugins follow the naming convention `klasp-plugin-<name>`. When a `klasp.toml`
check declares `type = "plugin"` with `name = "my-linter"`, klasp looks for
`klasp-plugin-my-linter` on `$PATH` using `which::which`.

Discovery during `klasp gate` is **lazy** — it only happens when a check with
`type = "plugin"` is encountered. There is no startup scan of `$PATH`. The
`klasp plugins list` subcommand (#42, W3) does perform a full `$PATH` scan to
enumerate all installed plugins, but that is explicit and read-only — gate
itself never enumerates.

**Example `klasp.toml` entry:**

```toml
[[checks]]
name = "my-linter"
[checks.source]
type = "plugin"
name = "my-linter"         # → looks for `klasp-plugin-my-linter` on $PATH
args = ["--strict"]        # optional: forwarded to the plugin on every --gate
[checks.source.settings]   # optional: opaque config blob forwarded verbatim
threshold = 10
```

If the binary is not on `$PATH`, klasp emits a `Verdict::Warn` with
`rule = "klasp::plugin"` and continues running the remaining checks.

---

## Subcommands

Each plugin binary must support exactly two subcommands:

### `--describe`

Prints the plugin's capabilities as a single JSON object to stdout, then exits 0.

```sh
klasp-plugin-my-linter --describe
```

**Output (`PluginDescribe`):**

```json
{
  "protocol_version": 0,
  "name": "klasp-plugin-my-linter",
  "config_types": ["my-linter"],
  "supports": { "verdict_v0": true }
}
```

| Field | Type | Description |
|---|---|---|
| `protocol_version` | `u32` | Must equal `PLUGIN_PROTOCOL_VERSION` (currently `0`). |
| `name` | `string` | Canonical plugin name including the `klasp-plugin-` prefix. |
| `config_types` | `string[]` | Informational: list of check `name` values this plugin supports. |
| `supports.verdict_v0` | `bool` | Must be `true`; declares compatibility with the v0 verdict protocol. |

klasp calls `--describe` before every `--gate` invocation. If `protocol_version`
does not equal klasp's `PLUGIN_PROTOCOL_VERSION`, klasp emits a `Verdict::Warn`
and skips the plugin. This is the forward-compatibility handshake.

### `--gate`

Reads a single JSON object from stdin, runs the check, writes a single JSON
object to stdout, then exits 0.

**Exit code contract:** the plugin MUST exit 0 even when the check produces
findings or a `fail` verdict. Non-zero exit is an infrastructure error, not a
check failure. Non-zero exit → `Verdict::Warn` (the plugin failed, not the
check).

```sh
klasp-plugin-my-linter --gate < gate-input.json > gate-output.json
```

---

## Wire Format

### Input: `PluginGateInput`

Written by klasp to the plugin's stdin before closing the pipe.

```json
{
  "protocol_version": 0,
  "schema_version": 2,
  "trigger": {
    "kind": "commit",
    "files": ["src/foo.rs", "src/bar.rs"]
  },
  "config": {
    "type": "my-linter",
    "args": ["--strict"],
    "settings": { "threshold": 10 }
  },
  "repo_root": "/abs/path/to/repo",
  "base_ref": "origin/main"
}
```

| Field | Type | Description |
|---|---|---|
| `protocol_version` | `u32` | `PLUGIN_PROTOCOL_VERSION` (`0`). |
| `schema_version` | `u32` | `KLASP_GATE_SCHEMA` value (currently `2`). |
| `trigger.kind` | `"commit" \| "push"` | Git event that triggered the gate. |
| `trigger.files` | `string[]` | Absolute paths of staged files in scope. Empty for push events or single-config mode. |
| `config.type` | `string` | Plugin name from `klasp.toml`. |
| `config.args` | `string[]` | Extra args from `klasp.toml`'s `args` field. |
| `config.settings` | `object \| null` | Opaque config from `klasp.toml`'s `[checks.source.settings]` block. |
| `repo_root` | `string` | Absolute path to the repository root. |
| `base_ref` | `string` | Merge-base ref (same as `KLASP_BASE_REF` env var). |

### Output: `PluginGateOutput`

Written by the plugin to stdout before exiting.

```json
{
  "protocol_version": 0,
  "verdict": "fail",
  "findings": [
    {
      "severity": "error",
      "rule": "my-linter/E001",
      "file": "src/foo.rs",
      "line": 42,
      "message": "line too long (120 > 100)"
    },
    {
      "severity": "warn",
      "rule": "my-linter/W002",
      "file": "src/bar.rs",
      "line": 7,
      "message": "unused import"
    }
  ]
}
```

| Field | Type | Description |
|---|---|---|
| `protocol_version` | `u32` | Must be `0`. |
| `verdict` | `"pass" \| "warn" \| "fail"` | Check outcome. |
| `findings` | `PluginFinding[]` | Zero or more findings. Empty array is valid for `pass`. |
| `findings[].severity` | `"info" \| "warn" \| "error"` | Severity of the individual finding. |
| `findings[].rule` | `string` | Rule identifier (e.g., `"ruff/E501"`). |
| `findings[].file` | `string \| null` | Relative or absolute file path. Optional. |
| `findings[].line` | `u32 \| null` | Line number (1-based). Optional. |
| `findings[].message` | `string` | Human-readable description of the finding. |

---

## Verdict Mapping

| Plugin `verdict` field | klasp `Verdict` | Gate exits |
|---|---|---|
| `"pass"` | `Verdict::Pass` | 0 |
| `"warn"` | `Verdict::Warn` with plugin findings | 0 |
| `"fail"` | `Verdict::Fail` with plugin findings | 2 (blocks agent) |

`Verdict::Fail` blocks the agent's tool call (exit 2 — the Claude Code
convention). `Verdict::Warn` renders a notice on stderr but allows the agent to
proceed.

---

## Error Handling

All plugin infrastructure errors produce a `Verdict::Warn` with a single finding:

```
rule = "klasp::plugin"
severity = "warn"
message = "plugin `<name>`: <reason>"
```

The gate continues running the remaining checks. Plugin errors never crash klasp
and never produce `Verdict::Fail` (only legitimate check findings can block the
agent).

| Failure mode | Trigger | Result |
|---|---|---|
| Binary not on `$PATH` | `which klasp-plugin-<name>` fails | `Verdict::Warn` |
| Non-zero exit on `--describe` | Plugin exits with code != 0 | `Verdict::Warn` |
| Malformed JSON on `--describe` | stdout is not valid JSON | `Verdict::Warn` |
| `protocol_version != 0` on `--describe` | Forward-compat check fails | `Verdict::Warn` |
| Non-zero exit on `--gate` | Plugin exits with code != 0 | `Verdict::Warn` |
| Malformed JSON on `--gate` | stdout is not valid JSON | `Verdict::Warn` |
| Timeout | Plugin exceeds `KLASP_PLUGIN_TIMEOUT_SECS` | `Verdict::Warn` |

**Timeout:** The default plugin timeout is 60 seconds. Override via the
`KLASP_PLUGIN_TIMEOUT_SECS` environment variable (integer seconds). This is
shorter than the default 120 s shell-check timeout because plugins that hang
are more likely misuse than intentionally long-running operations.

---

## Isolation Guarantees

Plugins run as **separate subprocesses**. They cannot mutate klasp's process
state (memory, file descriptors, signal handlers, exit code). All communication
is via the JSON protocol over stdin/stdout.

**Environment inheritance:** The plugin subprocess inherits klasp's full
environment. The following env vars are set explicitly (and are stable at v0):

| Env var | Value |
|---|---|
| `KLASP_BASE_REF` | Merge-base ref (e.g., `origin/main` or `HEAD~1`). |
| `KLASP_GATE_SCHEMA` | Gate wire-protocol version (currently `2`). |
| `KLASP_PROJECT_DIR` | Absolute path to the repo root klasp is gating. |
| `KLASP_PLUGIN_PROTOCOL_VERSION` | Plugin protocol version (currently `0`). |

Plugins may read any env var from the inherited environment. They must not rely
on env vars that are not listed above as stable — those are implementation
details that may change.

**What plugins cannot do:**

- Mutate klasp's process environment (they're a subprocess).
- Affect klasp's exit code directly (klasp reads the verdict from stdout, not the plugin exit code).
- Prevent klasp from continuing with other checks (all errors → `Verdict::Warn`, not abort).

---

## Forward Compatibility

The `protocol_version` field in both `PluginDescribe` and `PluginGateOutput`
allows klasp to detect version mismatches at runtime.

**How klasp handles versions:**

| Situation | Klasp behaviour |
|---|---|
| `protocol_version == 0` | Accept and proceed normally. |
| `protocol_version > 0` (future plugin) | `Verdict::Warn` with "update the plugin or wait for klasp v1.0" message. Skips the check. |
| `protocol_version` field missing from JSON | JSON parse error → `Verdict::Warn`. |

Plugin authors should always include `protocol_version` in both `--describe`
and `--gate` output. Future klasp versions that support `protocol_version = 1`
will still run `v0` plugins (backward compat is the stable protocol's job; `v0`
→ `v1` migration will be documented when `v1` ships at klasp v1.0).

---

## Example: Shell-Script Plugin

A minimal plugin that checks for `TODO` comments in staged files:

```bash
#!/usr/bin/env bash
# klasp-plugin-todo-check — finds TODO comments in staged Rust files.
# Install: cp klasp-plugin-todo-check /usr/local/bin/ && chmod +x ...
set -euo pipefail

if [[ "${1:-}" == "--describe" ]]; then
    printf '%s\n' '{
  "protocol_version": 0,
  "name": "klasp-plugin-todo-check",
  "config_types": ["todo-check"],
  "supports": {"verdict_v0": true}
}'
    exit 0
fi

if [[ "${1:-}" == "--gate" ]]; then
    input=$(cat)
    # Extract staged .rs files from the JSON input using jq (if available).
    files=$(printf '%s' "$input" | jq -r '.trigger.files[]? | select(endswith(".rs"))' 2>/dev/null || true)

    findings="[]"
    verdict="pass"

    if [[ -n "$files" ]]; then
        while IFS= read -r file; do
            [[ -f "$file" ]] || continue
            while IFS= read -r line_content; do
                lineno=$(echo "$line_content" | cut -d: -f1)
                printf -v finding \
                    '{"severity":"warn","rule":"todo-check/TODO","file":"%s","line":%s,"message":"TODO comment found"}' \
                    "$file" "$lineno"
                findings=$(printf '%s' "$findings" | jq ". + [$finding]")
                verdict="warn"
            done < <(grep -n 'TODO' "$file" || true)
        done <<< "$files"
    fi

    printf '{"protocol_version":0,"verdict":"%s","findings":%s}\n' "$verdict" "$findings"
    exit 0
fi

echo "unknown subcommand: ${1:-}" >&2
exit 1
```

**`klasp.toml` to activate it:**

```toml
[[checks]]
name = "todo-check"
triggers = [{ on = ["commit"] }]
[checks.source]
type = "plugin"
name = "todo-check"
```

The plugin relies on `jq` for JSON parsing. For production plugins, use a
compiled binary that embeds JSON handling — shell plugins are fragile in the
face of filenames with special characters, large staged sets, and missing
tools.

---

## Disable list

Users can prevent a plugin from being invoked by adding it to a per-user
disable list. This is a klasp-side concept — it does not affect the plugin wire
format.

### File location

The default path is `~/.config/klasp/disabled-plugins.toml`.

Override the path at any time via the `KLASP_DISABLED_PLUGINS_FILE` environment
variable. This is the preferred mechanism for test isolation and for users who
prefer a non-standard config directory.

```sh
# Override for a single command:
KLASP_DISABLED_PLUGINS_FILE=/tmp/test-disabled.toml klasp plugins list

# Override permanently in your shell profile:
export KLASP_DISABLED_PLUGINS_FILE="$XDG_CONFIG_HOME/klasp/disabled.toml"
```

### Format

```toml
# ~/.config/klasp/disabled-plugins.toml
disabled = ["my-linter", "another-plugin"]
```

Single key `disabled`, value is a list of plugin names **without** the
`klasp-plugin-` prefix. An absent file or `disabled = []` are both treated as
"no plugins disabled". The file is created (including parent directories) on
the first `klasp plugins disable <name>` invocation.

### CLI

```sh
# Add a plugin to the disable list:
klasp plugins disable my-linter

# Disabled plugins appear in list output with a "disabled" status tag:
klasp plugins list
```

There is no `klasp plugins enable` command. To re-enable a plugin, remove its
name from the `disabled` list in the TOML file.

### Runtime semantics

When `klasp gate` evaluates a check whose `type = "plugin"` points at a
disabled plugin, klasp returns `Verdict::Pass` for that check without
spawning the plugin binary. The gate continues to run all other checks
normally.

This is intentionally a quiet skip — the user explicitly disabled the plugin,
so no warn-level noise is emitted. Use `klasp gate -v` (verbose, future flag)
to observe which checks were skipped.

### Concurrency

`klasp plugins disable` is **not** lock-protected. Two concurrent invocations
can lose a write (last writer wins). For v0.3, run `disable` commands
sequentially. A future release may add file locking if this proves
problematic in practice.

### Malformed file handling

If the disable list contains invalid TOML (typically from a hand-edit error):

- `klasp gate` degrades gracefully — `plugin_disable_load` writes a one-line
  warning to stderr and treats the list as empty (so no plugin is silently
  skipped because of a typo).
- `klasp plugins disable <name>` **refuses** to overwrite a malformed file —
  it returns an error pointing at the parse failure so the user can fix or
  delete the file manually rather than losing previously-disabled entries.

### Plugin name validation

Names accepted by `klasp plugins disable` (and required for the binary lookup
`klasp-plugin-<name>`) are restricted to ASCII letters, digits, `-`, and `_`.
Path separators, shell metachars, and control characters are rejected so the
on-disk TOML cannot be coerced into surprising shapes.

---

## Versioning Summary

| Constant | Value | Meaning |
|---|---|---|
| `PLUGIN_PROTOCOL_VERSION` | `0` | Experimental; may break in any v0.3.x. |
| `GATE_SCHEMA_VERSION` | `2` | The klasp gate wire protocol version (separate integer). |

The plugin protocol version is **independent** of `GATE_SCHEMA_VERSION` and of
klasp's semver. Most klasp releases will not change either. Changes to this
protocol are described in `CHANGELOG.md` under the affected minor version.
