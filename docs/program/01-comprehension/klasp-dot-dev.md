# Comprehension — klasp.dev (marketing site)

Program artifact · Phase 1 · repo `/Users/liammccarthy/Projects/klasp.dev` (separate from klasp).

## Intent & stack

Single-page static marketing site for klasp. **Astro 6**, `pnpm build` → `dist/`, deployed on
**AWS Amplify** (`amplify.yml`, Node 22, pnpm 10.28.2, `--frozen-lockfile`). One page:
`src/pages/index.astro` (hero, animated mini-demo, capability matrix, footer). No component test
suite; no CI build check (only OSV/supply-chain workflows exist).

## State at program start vs. now

| Item | Was | Now (this program) |
|---|---|---|
| Version (eyebrow/footer/release link/meta) | v0.4.0 (one minor stale) | v0.5.0 — fixed (commit on `claude/site-v0.5-sync`) |
| `klasp demo` mention | absent | still absent — deferred to S4 site enhancement |
| `klasp.toml` surface comment | "only claude_code" (stale) | file is **untracked**; edit left on disk, not committed (maintainer's tracking call) |
| CI build check for the site | none | none — proposed as S4 (would catch a broken Astro build before Amplify) |

## Cross-repo coupling

The site's version string + capability matrix must track klasp's actual feature set. Today this is
manual and drifted by a full minor. **Idea S4** (a generated `version + matrix` JSON the site renders)
would make drift structurally impossible. This is the one real klasp↔klasp.dev contract.
