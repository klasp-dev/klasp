<!--
PR titles are checked by .github/workflows/pr-title.yml — use a semantic
prefix, e.g. feat(surfaces): …, fix(core): …, docs(readme): …
-->

## Summary

<!-- What does this change and why? Link the issue it closes. -->

## Checklist

- [ ] Tests cover the change (or it's docs/CI-only).
- [ ] `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --workspace` pass locally.
- [ ] If this PR adds or changes an agent surface, `docs/agent-surfaces.md` is updated (new/changed surface has a matrix row, every `✓` links the test that proves it).
- [ ] `CHANGELOG.md` updated under `[Unreleased]` for user-facing changes.
