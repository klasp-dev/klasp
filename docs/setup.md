# klasp setup — one-command first-run flow

`klasp setup` is the recommended entry point for new users. It runs the full
detect → narrow → write → install → doctor sequence in one command, so you go
from `git init` to green doctor without manual editing of `klasp.toml`.

## Quick start

```bash
# Copy-paste this in your repo:
klasp setup
```

That single command:

1. Detects existing quality gates (pre-commit, Husky, Lefthook, lint-staged,
   plain `.git/hooks`).
2. Detects which AI coding agents are installed on this machine (`~/.claude/`,
   `~/.codex/`, `~/.aider*`).
3. Writes `klasp.toml` with `[gate].agents` narrowed to what you actually have
   installed — no doctor FAILs from surfaces you don't run.
4. Runs `klasp install --agent all` against the narrowed list.
5. Runs `klasp doctor` and reports the result.

### Example: fresh repo with Claude Code only

```text
$ ls ~/
.claude/   ...

$ cd my-new-project
$ git init
$ klasp setup

klasp setup — detected 0 gate(s)
detected agents: claude_code
[...]
wrote klasp.toml
claude_code: installed

--- klasp doctor ---
OK    config: klasp.toml loaded OK
OK    hook[claude_code]: current (schema v2)
OK    settings[claude_code]: hook entry present
INFO  codex: not in [gate].agents, skipping
INFO  aider: not in [gate].agents, skipping
doctor: all checks passed

setup complete — `klasp doctor` passed with no FAILs.
```

The generated `klasp.toml` has `agents = ["claude_code"]` — exactly what you
have installed. No spurious FAILs for Codex or Aider.

## Flags

```
klasp setup [--interactive] [--dry-run]
```

| Flag              | Effect                                                              |
|-------------------|---------------------------------------------------------------------|
| (none)            | Non-interactive: detect, write, install, doctor in one shot.       |
| `--interactive`   | Prompts before writing `klasp.toml` and before installing surfaces.|
| `--dry-run`       | Print the detection plan and computed config; write nothing.        |

### `--dry-run`

Useful for seeing what setup *would* do before committing:

```bash
klasp setup --dry-run
```

### `--interactive`

Adds y/n prompts before each destructive step. Suitable for first-time use
when you want to inspect the plan before accepting it:

```bash
klasp setup --interactive
```

The prompts are:

1. "Mirror N detected gate(s) into klasp.toml?" — accepts/skips gate adoption.
2. "Write klasp.toml now?" — confirms the atomic write.
3. "Install agent surfaces now?" — confirms the install step.

Answering "n" at any prompt exits cleanly with a message explaining what to
run manually.

## Relationship to the 3-command flow

`klasp setup` is additive sugar over the three scriptable primitives:

```bash
# Equivalent 3-command flow (still supported for CI / scripts):
klasp init --adopt --mode mirror
klasp install --agent all
klasp doctor
```

`setup` adds:

- Machine-level agent detection (narrows `[gate].agents` automatically).
- Duplicate check-name deduplication (second `lint` becomes `lint-lefthook`).
- Green summary in one terminal session instead of three commands.

The primitives (`init`, `install`, `doctor`) remain unchanged and are still
the right choice for CI pipelines and scripted environments.

## Agent detection logic

`klasp setup` (and `klasp init --adopt --mode mirror`) probes the following
paths to determine which agents are installed on this machine:

| Agent         | Detected when                                           |
|---------------|---------------------------------------------------------|
| `claude_code` | `~/.claude/` directory exists                           |
| `codex`       | `~/.codex/` directory exists                            |
| `aider`       | `~/.aider`, `~/.aider.conf.yml`, or `~/.aiderignore` exists |

When none of the above are found, setup falls back to today's three-agent
default with a comment in `klasp.toml` explaining how to narrow the list
manually.

## Duplicate check names

When two gate managers (e.g. Husky *and* Lefthook) both define a hook named
`lint`, setup suffixes the second with the gate type:

```toml
[[checks]]
name = "lint"        # from Husky — bare name kept
...

[[checks]]
name = "lint-lefthook"  # from Lefthook — suffix added on collision
...
```

Doctor output becomes self-documenting:

```text
OK    path[lint]: `pnpm` found in PATH
OK    path[lint-lefthook]: `pnpm` found in PATH
```

## See also

- `docs/adopt.md` — detailed docs for `klasp init --adopt`.
- `klasp doctor --help` — diagnose an existing install.
- `klasp install --help` — install individual agent surfaces.
