# agentic-flow receipt schema and canonical diff hash

This document is the **single source of truth** for the agentic-flow receipt
schema and the canonical `diff_hash` recipe. It is cited by both sides of a
co-owned contract:

- **Writer** — the agentic-flow orchestrator (`~/.claude/agentic-flow/`, a
  separate repo). Each step writes its receipt on completion or skip, computing
  `diff_hash` with the exact pinned git command below.
- **Reader** — the `klasp-plugin-agentic-flow` reference plugin
  (`examples/klasp-plugin-agentic-flow/`). It only ever READS receipts; it never
  creates or mutates them.

The two sides MUST compute `diff_hash` **byte-for-byte identically**. Any change
to the recipe is a breaking change to the contract and must be made on both
sides in lockstep.

---

## Files

```
.agentic-flow/
  state.json                 # the cursor / index (per-clone run state)
  receipts/
    01-ideate.json
    06-simplify.json
    07-code-review.json
    ...
```

- One receipt file per step: `.agentic-flow/receipts/NN-step.json`.
- `NN` is the **zero-padded two-digit, 1-based** position of the step in
  `flow.yaml`'s `steps[]` list. The filename is `"NN-" + step.id`, e.g. the 7th
  step `code-review` → `07-code-review.json`.
- Both `state.json` and `receipts/` are per-clone run state. They are
  `.gitignore`d at the repo root (see the project `.gitignore`).

---

## Receipt fields

The plugin reads these. The orchestrator writes them. The reader struct uses
lenient `#[serde(default)]` on every field and ignores unknown extra fields for
forward-compat, so the writer may add fields without breaking older readers.

| field | type | required | notes |
|---|---|---|---|
| `step` | string | yes | `"NN-id"` — ties to flow.yaml position + id. |
| `status` | string | yes | `"completed"` \| `"skipped"` \| `"blocked"` (mirrors `state.json` `history[].outcome`). |
| `gating` | string | yes | `"auto"` \| `"user-confirm"` — copied from flow.yaml so the reader can enforce confirmation without re-parsing the manifest per receipt. |
| `branch` | string | yes (completed) | git branch the step ran on. |
| `base_ref` | string | yes (completed) | MUST equal `KLASP_BASE_REF` / `PluginGateInput.base_ref` for the receipt to be comparable. |
| `head` | string | yes (completed) | full HEAD sha when the step ran (advisory; `diff_hash` is authoritative). |
| `diff_hash` | string | yes (completed) | the load-bearing freshness field; `"sha256:<hex>"` per the recipe below. |
| `artifacts` | string[] | optional | machine-readable artifact list. |
| `verdict` | string | optional | the step's own outcome (e.g. `"pass"`); informational to the gate. |
| `user_confirmed` | bool | yes-if `gating=="user-confirm"` completed | `false`/absent otherwise. |
| `confirmation_id` | string | required-if `user_confirmed==true` | opaque id (NOT a transcript). |
| `confirmed_at` | string (RFC3339) | optional | companion to `confirmation_id`. |
| `skip_reason` | string | yes (skipped) | distinguishes a legit skip from a missing receipt. |
| `manifest_version` | u32 | optional | flow.yaml `version` echo. |
| `started_at` | string (RFC3339) | yes (completed) | |
| `completed_at` | string (RFC3339) | yes (completed) | |

Example completed receipt:

```json
{
  "step": "07-code-review",
  "status": "completed",
  "gating": "auto",
  "branch": "feature/thing",
  "base_ref": "origin/main",
  "head": "abc123def4567890abc123def4567890abc123de",
  "diff_hash": "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
  "artifacts": ["review.md"],
  "verdict": "pass",
  "user_confirmed": false,
  "manifest_version": 1,
  "started_at": "2026-05-08T10:00:00Z",
  "completed_at": "2026-05-08T10:02:00Z"
}
```

---

## `state.json` (the index / cursor)

Version-1 shape that the plugin reads:

| field | type | notes |
|---|---|---|
| `version` | u32 | gate warns if `!= 1`. |
| `current_step` | string | the cursor (id of the step in progress). |
| `skipped` | string[] | bare step ids intentionally skipped (e.g. `"ideate"`). |
| `history` | object[] | `{ id, ran_at, outcome, reason \| artifact }` — intended ordering + outcomes. |

The plugin uses `state.json` as the index/cursor and **receipts as the per-step
source of truth**. A completed receipt beats a `skipped` entry in `state.json`;
a step listed as completed in `history[]` but with no receipt counts as MISSING.

---

## Reconciliation rule (what the reader enforces)

For each required step at the trigger's depth:

1. a receipt with `status="completed"` that is **fresh** → OK;
2. a `status="skipped"` receipt OR the bare id present in `state.json.skipped[]`
   → legitimately absent, OK;
3. no receipt AND not in `skipped[]` → **MISSING** (error).

A step in `state.json.skipped[]` *with* a completed receipt is allowed
(completed beats skipped). A stale upstream receipt invalidates itself and all
downstream receipts. The suggested resume point is the earliest failing step.

---

## Canonical diff hash recipe (PINNED — byte-for-byte)

```
primary = git -C <repo_root> diff --no-color --no-ext-diff <base_ref>...HEAD

# On the COMMIT trigger ONLY, also fold in the staged delta:
staged  = git -C <repo_root> diff --no-color --no-ext-diff --cached

diff_hash = "sha256:" + lowercase_hex( SHA-256( primary  [ ++ staged on commit ] ) )
```

Pinned details that both sides MUST honour:

- flags **`--no-color --no-ext-diff`** (exactly these, in this order after `diff`);
- **three-dot** (`...`) range = merge-base of `<base_ref>` and `HEAD`;
- `<base_ref>` is `PluginGateInput.base_ref` (== `KLASP_BASE_REF`);
- rename detection left at the **git default** (no `-M` / `--no-renames`);
- hash the **raw bytes** git emits — **no trailing-newline trimming**, no
  normalization;
- on the **commit** trigger, append the raw bytes of the `--cached` diff as a
  second component and feed both, in order (`primary` then `staged`), into a
  single SHA-256;
- on the **push** trigger, use the three-dot component **only**.

A commit-time receipt therefore stays comparable at push-time on the three-dot
component, while the staged component captures uncommitted work at commit time.
If a receipt's `base_ref` differs from the gate's `base_ref`, the receipt is
treated as **stale** (different comparison basis) regardless of `diff_hash`.

The reader implements this in
[`examples/klasp-plugin-agentic-flow/src/runner.rs`](../examples/klasp-plugin-agentic-flow/src/runner.rs)
(`canonical_diff_hash`); the plugin's integration tests assert parity against the
identical git command (`diff_hash_parity`).
