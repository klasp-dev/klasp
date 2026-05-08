# Gate adoption (`klasp init --adopt`)

## What this does

`klasp init --adopt` detects existing gates in your repo and proposes a
`klasp.toml` that mirrors them so agents see the same checks your humans hit.
The original gate config files are never modified — klasp writes only its own
`klasp.toml`. Run `klasp doctor` after adopting to verify the adopted checks
are reachable on PATH.

## Modes

| Mode | Writes files? | What it does |
|---|---|---|
| `inspect` | No | Detects gates and prints a proposed plan. Safe to run any time. |
| `mirror` | `klasp.toml` only | Writes a `klasp.toml` that mirrors detected gates. Never touches hook configs. |
| `chain` | Not yet supported in v1 | Rejected with an explanatory message; use `--mode mirror` instead. |

Default when `--adopt` is passed without `--mode` is `inspect`.

## Detectors

- **pre-commit framework** — looks for `.pre-commit-config.yaml` or
  `.pre-commit-config.yaml`. Proposes a `type = "pre_commit"` check that
  delegates to the pre-commit binary. Never touches the config file or any
  `.git/hooks/pre-commit` that pre-commit itself manages.

- **Husky** — looks for `.husky/pre-commit` and `.husky/pre-push`. Reads the
  first substantive command in each hook script and proposes a matching shell
  check. Never edits `.husky/*` files.

- **Lefthook** — looks for `lefthook.yml` or `lefthook.yaml`. Parses
  `pre-commit` and `pre-push` command entries and proposes one shell check per
  command. Never edits the Lefthook config.

- **Plain `.git/hooks`** — looks for `.git/hooks/pre-commit` and
  `.git/hooks/pre-push` that are not attributed to another hook manager. Emits
  an inspect-only finding showing the file path. Mirror mode does not overwrite
  plain hooks under any circumstance.

- **lint-staged** — looks for a `"lint-staged"` key in `package.json` or a
  standalone `.lintstagedrc*` file. Proposes a shell check using a
  package-manager-aware command: `pnpm exec lint-staged` when `pnpm-lock.yaml`
  is present, `yarn lint-staged` when `yarn.lock` is present, `npx lint-staged`
  otherwise. Never touches package.json or lockfiles.

## Mirror vs chain

**Mirror** is the safe default. It writes a `klasp.toml` that adds klasp
coverage without touching any existing hook infrastructure. Your existing gate
(pre-commit, Husky, Lefthook) continues to run exactly as before; klasp runs
the same checks independently at the agent surface. This can mean a check runs
twice at commit time — once in the human hook, once via klasp. That duplication
is intentional: the agent surface fires *before* the git hook, so the agent is
informed earlier.

**Chain** (planned) would integrate klasp *into* the existing hook manager —
for example, appending a `klasp gate` call inside `.husky/pre-commit` — so
there is only one execution path. Chain mode requires careful managed-block
handling with a proven uninstall round-trip before it is safe to ship. It is
not supported in v1; `--mode chain` exits with code 2 and suggests `--mode mirror`.

After adopting, run `klasp doctor` to verify that the binaries referenced by
adopted checks are available on PATH. Doctor surfaces adopted `type = "pre_commit"`
checks whose `pre-commit` binary is missing and prints an install hint.

## Example session

```text
$ klasp init --adopt --mode inspect

Detected existing gates:

OK  pre-commit framework
    .pre-commit-config.yaml
    mirror: type = "pre_commit"

OK  husky pre-commit
    .husky/pre-commit runs: pnpm lint-staged
    mirror: command = "pnpm exec lint-staged"

WARN plain git hook
    .git/hooks/pre-push exists and is user-owned
    klasp will not overwrite it
    run with --mode chain to append a managed block, or mirror the command manually

Next:
  klasp init --adopt --mode mirror
  klasp install --agent all
  klasp doctor
```

Run `klasp init --adopt --mode mirror` to write the `klasp.toml`, then
`klasp install --agent all` to install the gate hook, and `klasp doctor` to
verify everything is healthy.

## Future work

- `klasp init --audit <stack>` — build on the same detector/planner machinery
  to suggest stack-specific audit recipes (Python, TypeScript, Rust, Go, etc.)
  rather than only mirroring existing gates.
- `klasp doctor --adoption` — re-run detection later and warn when the repo's
  existing gates have drifted from the checks declared in `klasp.toml`.
- `klasp adopt --interactive` — a guided terminal flow for teams that prefer a
  step-by-step experience; the v1 flow is intentionally scriptable and
  non-interactive.
