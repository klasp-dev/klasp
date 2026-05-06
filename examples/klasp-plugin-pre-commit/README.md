# klasp-plugin-pre-commit

Reference plugin for [klasp](https://github.com/klasp-dev/klasp) that wraps the
[pre-commit](https://pre-commit.com) framework and speaks the v0 plugin protocol.

> **This is the canonical "fork me" starting point** for third-party klasp plugin authors.
> To ship a real plugin, copy this directory into its own repository, rename the
> crate and binary, replace `src/runner.rs` with your own check logic, and publish
> wherever you like. Nothing from the klasp workspace needs to be imported.

---

## Status

Experimental — tracks `PLUGIN_PROTOCOL_VERSION = 0`. See [Protocol caveats](#protocol-caveats).

---

## Installation

### From source (recommended for the reference)

```sh
git clone https://github.com/klasp-dev/klasp
cd klasp/examples/klasp-plugin-pre-commit
cargo build --release
cp target/release/klasp-plugin-pre-commit ~/.local/bin/
```

Make sure `~/.local/bin` (or wherever you copied the binary) is on your `$PATH`.

### As a dependency in your own plugin crate

Do not add this crate as a dependency. The design intent is that you **fork** it.
Copy the `src/protocol.rs` types into your own crate — they are the wire contract,
not an importable library.

---

## Configuration (`klasp.toml`)

```toml
[[checks]]
name = "pre-commit"
triggers = [{ on = ["commit"] }]
timeout_secs = 60

[checks.source]
type = "plugin"
name = "pre-commit"          # → looks for `klasp-plugin-pre-commit` on $PATH
```

Optional `args` and `settings` are forwarded verbatim in `PluginGateInput` but
this reference plugin does not use them. A fork may parse `config.settings` for
custom hook-stage selection or config-path overrides.

---

## Behaviour

On `--gate` invocation the plugin:

1. Checks that `pre-commit` is on `$PATH`. If not, returns `warn` with
   `rule = "klasp-plugin-pre-commit/binary-missing"` and a hint to install via
   `pipx install pre-commit`.

2. Invokes `pre-commit run --hook-stage pre-commit` in `repo_root`:
   - Commit trigger: no ref-range flags (targets the staging area).
   - Push trigger: `--from-ref <base_ref> --to-ref HEAD`.

3. Parses hook failure lines (`"<hook>....Failed"`) from pre-commit stdout.
   This format is stable from pre-commit 3.0 through 4.x (tested versions).

4. Maps exit codes:
   | pre-commit exit | Plugin verdict |
   |---|---|
   | `0` | `pass` |
   | `1` + parseable failures | `fail` + per-hook findings |
   | `1` + no parseable output | `fail` + generic finding with stderr |
   | other / no exit code | `fail` + details |
   | binary missing | `warn` |

5. Caps findings at 100 to respect klasp's 16 MiB output bound. A sentinel
   `klasp-plugin-pre-commit/truncated` finding is appended when truncation occurs.

**Note on per-file/per-line info:** pre-commit does not emit machine-readable
file/line data in its summary output. All findings have `file = null` and
`line = null`. This matches the spirit of the built-in `pre_commit` recipe.

---

## Protocol caveats

```
PLUGIN_PROTOCOL_VERSION = 0
```

This protocol is **explicitly experimental**. It may change in any v0.3.x
release without a deprecation period. It graduates to `1` (stable,
backward-compatible) only at klasp v1.0.

Track [docs/plugin-protocol.md](https://github.com/klasp-dev/klasp/blob/main/docs/plugin-protocol.md)
for changes. When `protocol_version = 1` ships you will receive a clear migration
path from `0 -> 1`.

Plugin authors targeting v0.3 should treat their plugin as experimental alongside
the protocol.

---

## Forking this plugin into your own repo

1. Copy `examples/klasp-plugin-pre-commit/` into a new repository.
2. In `Cargo.toml`: rename `name` and the `[[bin]]` `name` to `klasp-plugin-<yourname>`.
3. In `src/main.rs`: update `PLUGIN_NAME` and `CONFIG_TYPES`.
4. In `src/runner.rs`: replace the `pre-commit` invocation with your own check logic.
5. `src/protocol.rs`: no changes needed — these types are the wire contract.
6. Update `README.md` (this file) with your plugin's documentation.
7. Build and place the binary on the user's `$PATH` as `klasp-plugin-<yourname>`.

The types in `src/protocol.rs` are **duplicated from klasp-core on purpose**.
Plugins are separate processes; importing klasp-core would create an unnecessary
coupling and would prevent independent versioning. The JSON field names are the
stable contract.

---

## Tested pre-commit versions

| Version | Status |
|---|---|
| 3.8.x | Tested |
| 4.0.x | Tested |

If you encounter a pre-commit version whose output format this plugin cannot
parse, it will fall back to a single generic finding with the raw stderr and you
should file an issue at https://github.com/klasp-dev/klasp/issues.
