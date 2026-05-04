# klasp

[**klasp.dev**](https://klasp.dev) · [crates.io](https://crates.io/crates/klasp) · [npm](https://www.npmjs.com/package/klasp) · [PyPI](https://pypi.org/project/klasp/) · [GitHub](https://github.com/klasp-dev/klasp)

> Block AI coding agents (Claude Code, Cursor, Codex, Aider) on the same quality gates your humans hit at `git commit`.

**Status: name-reservation placeholder.** The `0.0.0` publish on each registry exists only to claim the name. The v0.1 implementation is in design. Star to follow.

## What klasp will be

One `gates.yaml`, one `klasp install`, and every agent-initiated `git commit` and `git push` runs through `pre-commit`, `fallow`, your test suite, or any shell command — blocking on failure exactly like a human's git hook would.

```yaml
# gates.yaml
version: 1
agents: auto                # detect Claude Code, Cursor, Codex, Aider
gates:
  - id: pre-commit
    triggers: [git_commit]
    run: pre-commit run --hook-stage pre-commit --from-ref {base} --to-ref HEAD
  - id: fallow
    triggers: [git_commit, git_push]
    run: fallow audit --base {base} --quiet
    when: exists('.fallowrc.json')
```

## Distribution layout

| Package | Path | Purpose |
|---|---|---|
| Rust crate | `./` (root) | Canonical implementation |
| npm wrapper | `./npm/` | Distributes the binary to Node ecosystems |
| PyPI wrapper | `./pypi/` | Distributes the binary to Python ecosystems |

## License

Apache-2.0
