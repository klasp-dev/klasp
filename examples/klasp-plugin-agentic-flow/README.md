# klasp-plugin-agentic-flow

Reference plugin for [klasp](https://github.com/klasp-dev/klasp) that audits the
workflow receipts written by the [agentic-flow](https://github.com/klasp-dev)
orchestrator and speaks the v0 plugin protocol.

It is a **read-only auditor**. It never runs the workflow, never infers state
from chat, and never creates or mutates receipts. It checks one thing: that the
required workflow steps for the current git event have a *completed and fresh*
(or legitimately *skipped*) receipt, and blocks the gate when they don't.

> **This is also a canonical "fork me" starting point** for third-party klasp
> plugin authors. To ship your own plugin, copy this directory into its own
> repository, rename the crate and binary, replace `src/runner.rs` with your own
> check logic, and publish wherever you like. Nothing from the klasp workspace
> needs to be imported.

---

## Status

Experimental — tracks `PLUGIN_PROTOCOL_VERSION = 0`. See [Protocol caveats](#protocol-caveats).

---

## What it audits

agentic-flow writes one receipt per step to `.agentic-flow/receipts/NN-step.json`
(e.g. `06-simplify.json`, `07-code-review.json`). `NN` is the zero-padded
two-digit 1-based position in `flow.yaml`. The plugin reconciles those receipts
against the manifest for the current trigger depth:

| git event | required steps (v1) |
|---|---|
| **commit** | the impl path is reached — `feature-dev` OR `dispatch-impl` has a completed/skipped receipt — AND every `user-confirm` step the manifest demands up to `current_step` has `user_confirmed=true`. |
| **push** | `06-simplify`, `07-code-review`, `08-review-handoff`, `09-quality-gates` each have a *fresh* completed (or legit-skipped) receipt. |

For each required completed receipt the plugin recomputes the **canonical diff
hash** and compares it to the receipt's `diff_hash`; a mismatch means the diff
changed after the step ran (the receipt is *stale*). A stale upstream receipt
invalidates itself and all downstream receipts — so the suggested resume point is
always the earliest failing step.

The single source of truth for the receipt schema and the diff-hash recipe is
[`docs/agentic-flow-receipts.md`](../../docs/agentic-flow-receipts.md) in the
klasp repository. The **writer** half (which creates the receipts) ships in the
agentic-flow orchestrator, a separate repo; the **reader** half lives in this
plugin's `src/receipt.rs` and `src/runner.rs`. They MUST stay byte-identical on
the diff-hash recipe.

---

## Findings

| condition | severity | rule | verdict contribution |
|---|---|---|---|
| required receipt missing (and not skipped) | error | `agentic-flow/missing-step` | fail |
| completed receipt is stale (diff changed) | error | `agentic-flow/stale-step` | fail |
| `user-confirm` step not confirmed | error | `agentic-flow/unconfirmed-step` | fail |
| manifest has a step the plugin doesn't know | warn | `agentic-flow/unknown-step` | warn |
| infra error (missing dir, malformed JSON/YAML, git failed) | warn | `klasp-plugin-agentic-flow/<suffix>` | warn |

Infrastructure and plugin errors are **always** `warn`, never `fail` — matching
klasp's universal "plugin error = warn" rule. The plugin itself always exits 0;
a non-zero exit would be misread by klasp as an infrastructure error.

When the verdict is `fail`, the earliest failing step's message carries the
resume hint, e.g.:

```
agentic-flow incomplete:
  - missing receipt: 07-code-review
  - stale receipt: 06-simplify was run before latest diff change
Next: run /agentic-flow resume --from 06
```

---

## Configuration (`klasp.toml`)

```toml
[[checks]]
name = "agentic-flow"
triggers = [{ on = ["push"] }]
timeout_secs = 30

[checks.source]
type = "plugin"
name = "agentic-flow"          # → looks for `klasp-plugin-agentic-flow` on $PATH

[checks.source.settings]
# All optional — these are the defaults.
manifest = "~/.claude/agentic-flow/flow.yaml"   # plugin expands `~` itself
state    = ".agentic-flow/state.json"            # repo-relative
receipts = ".agentic-flow/receipts/"             # repo-relative
# Optional depth override (see "Protocol v0 trigger limitation" below):
# phase  = "pr-merge"                             # commit | push | pr-create | pr-merge
```

`settings` is forwarded verbatim by klasp as an opaque JSON blob. Absolute paths
are honoured as-is; `state` and `receipts` are resolved against `repo_root` when
relative; `manifest` expands a leading `~` to `$HOME`.

---

## Protocol v0 trigger limitation (read this before pinning a deeper phase)

The issue's four-phase gate semantics (commit / push / pr-create / pr-merge)
**cannot** be transmitted as distinct triggers under protocol v0:
`PluginTriggerKind` is exactly `{Commit, Push}`, and klasp's gate collapses
custom triggers to a `Commit` sentinel. So `gh pr create` / `gh pr merge` arrive
at the plugin labelled `commit`.

This plugin therefore ships **commit and push depths natively** and exposes an
optional `settings.phase` escape hatch (`"pr-create"` | `"pr-merge"`) so a
`klasp.toml` check entry can pin a deeper depth manually until klasp grows
named-trigger → check-phase routing and a trigger-name field on the plugin wire.
**Do not** assume pr-create / pr-merge will ever arrive as a `kind` under
`protocol_version = 0`.

---

## Behaviour summary

On `--gate` the plugin:

1. Resolves `manifest` / `state` / `receipts` paths from `config.settings`
   (with the defaults above).
2. Loads `flow.yaml` as the source of truth for step order, `id`, `gating`,
   and `enabled`. Disabled steps are excluded from the required set.
3. Determines the required set by trigger depth (commit / push), honouring an
   optional `settings.phase` override.
4. Loads `state.json` (the cursor/index) and the per-step receipts.
5. Reconciles each required step: completed+fresh OK; status=skipped or id in
   `state.json.skipped[]` OK; otherwise missing → error.
6. Recomputes the canonical diff hash for each completed receipt and flags
   stale ones.
7. Enforces `user_confirmed == true` (with a `confirmation_id`) on
   `user-confirm` steps.
8. Maps findings to a verdict: any `error` → `fail`; only warns → `warn`; none
   → `pass`.

---

## Canonical diff hash

Writer and auditor MUST compute this byte-for-byte identically. Pinned recipe:

```
primary = git -C <repo_root> diff --no-color --no-ext-diff <base_ref>...HEAD
# on the COMMIT trigger only, also fold in the staged delta:
staged  = git -C <repo_root> diff --no-color --no-ext-diff --cached
diff_hash = "sha256:" + hex( sha256( primary [++ staged on commit] ) )
```

Pinned details: `--no-color --no-ext-diff`, three-dot (`...`, merge-base) range,
default rename detection, raw bytes (no trailing-newline trimming). `base_ref`
is `PluginGateInput.base_ref` (`KLASP_BASE_REF`). If a receipt's `base_ref`
differs from the gate's, the receipt is treated as stale (different comparison
basis). The full spec lives in
[`docs/agentic-flow-receipts.md`](../../docs/agentic-flow-receipts.md).

---

## Protocol caveats

```
PLUGIN_PROTOCOL_VERSION = 0
```

This protocol is **explicitly experimental**. It may change in any v0.3.x
release without a deprecation period, and graduates to `1` (stable) only at
klasp v1.0. Track
[docs/plugin-protocol.md](https://github.com/klasp-dev/klasp/blob/main/docs/plugin-protocol.md)
for changes.

---

## Forking this plugin into your own repo

1. Copy `examples/klasp-plugin-agentic-flow/` into a new repository.
2. In `Cargo.toml`: rename `name` and the `[[bin]]` `name` to `klasp-plugin-<yourname>`.
3. In `src/main.rs`: update `PLUGIN_NAME` and `CONFIG_TYPES`.
4. In `src/runner.rs` (and `src/receipt.rs`): replace the audit logic with your
   own check.
5. `src/protocol.rs`: no changes needed — these types are the wire contract.
6. Update `README.md` with your plugin's documentation.
7. Build and place the binary on the user's `$PATH` as `klasp-plugin-<yourname>`.

The types in `src/protocol.rs` are **duplicated from klasp-core on purpose**.
Plugins are separate processes; importing klasp-core would create unnecessary
coupling and prevent independent versioning. The JSON field names are the stable
contract.

---

## Installation

```sh
git clone https://github.com/klasp-dev/klasp
cd klasp/examples/klasp-plugin-agentic-flow
cargo build --release
cp target/release/klasp-plugin-agentic-flow ~/.local/bin/
```

Make sure `~/.local/bin` (or wherever you copied the binary) is on your `$PATH`.
