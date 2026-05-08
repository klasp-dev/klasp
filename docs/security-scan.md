# Security scan — known CVEs and best practices

Date: 2026-05-08
Scope: `klasp-dev/klasp` Rust workspace (5 crates), npm shim package, PyPI
maturin wheel, and CI/CD workflows.
Tooling: `cargo-audit 0.22.1` against `RustSec/advisory-db` (1068 advisories
loaded), manual review of CI workflows and source.

## Summary

| Area                            | Result                              |
| ------------------------------- | ----------------------------------- |
| Rust dependency CVEs            | **0 vulnerabilities, 0 warnings**   |
| Yanked / unmaintained crates    | None reported                       |
| Network / TLS deps              | None present (large CVE class N/A)  |
| `unsafe` blocks in non-test src | 0                                   |
| Hard-coded secrets              | None                                |
| CI third-party action pinning   | SHA-pinned (good)                   |
| CI publishing                   | OIDC trusted publishing for npm/PyPI |

No known CVEs were found and the project follows most current best practices.
A handful of small hardening opportunities are listed under
[Recommendations](#recommendations).

## Method

1. `cargo audit` (RustSec) over `Cargo.lock` — 109 transitive crates scanned.
2. `cargo tree` review of direct dependencies.
3. Manual review of `.github/workflows/{ci,release}.yml` for action pinning,
   permissions scope, secret handling, and publishing flow.
4. Source grep for: `unsafe`, command/shell execution sites, file-permission
   bits, embedded secrets, panics, and common injection patterns.
5. Inspection of distribution packaging (`npm/`, `pypi/`) including the
   `npm/klasp/bin/klasp.js` shim.

## Findings

### 1. Dependency CVEs — clean

`cargo audit` scanned 109 crates in `Cargo.lock` and reported **0
vulnerabilities and 0 advisories** (informational/unmaintained/yanked
included). Direct dependencies are limited to mainstream, well-maintained
crates: `clap`, `serde`, `serde_json`, `toml`, `thiserror`, `anyhow`,
`tracing`, `regex`, `which`, `tempfile`, `serde_yaml_ng`, `quick-xml`,
`rayon`, `insta` (dev-only).

The workspace pulls in **no HTTP/TLS stack** (no `openssl`, `reqwest`,
`hyper`, `rustls`, `tokio`), which removes the most CVE-prone class of Rust
deps entirely. YAML support uses `serde_yaml_ng` (the maintained fork) rather
than the abandoned `serde_yaml`.

### 2. `unsafe` Rust — minimal, justified

Two `unsafe` blocks exist in the entire tree, both in test code in
`klasp-core/src/protocol.rs:196,202`. They wrap `std::env::set_var` /
`remove_var`, which became `unsafe` in newer Rust editions. Each has a
`SAFETY:` comment explaining the single-threaded test usage. No `unsafe` in
non-test source.

### 3. Subprocess execution — safe by construction

The crate launches several subprocesses (`git`, `sh`, `cargo`, `pytest`,
`pre-commit`, `fallow`, plugin binaries). All call sites use
`std::process::Command::new(...).args([...])` style (`klasp/src/git.rs`,
`klasp/src/sources/{cargo,pytest,pre_commit,fallow,plugin}/...`), which
**bypasses the shell** and is not subject to argument injection.

The single `sh -c <command>` invocation lives in
`klasp/src/sources/shell.rs:146-148` and runs commands declared in the
project's own `klasp.toml`. This is by design — the same trust model as
`pre-commit` hooks or `Makefile` targets — and the user's `klasp.toml` is
already trusted source under their VCS control.

Plugin binary discovery uses `which::which("klasp-plugin-<name>")`
(`klasp/src/sources/plugin.rs:99`), respecting `PATH`. This is the same
contract as `cargo-<subcommand>` and is the documented v0.1 design (no
network fetch, no install-time download). Plugin output is bounded by
`MAX_PLUGIN_OUTPUT_BYTES` to prevent OOM via chatty subprocesses.

### 4. Filesystem writes — atomic with deliberate modes

Hook installation (`klasp-agents-{claude,codex,aider}/src/surface.rs`) uses
the `tempfile + rename` pattern (`atomic_write`), with mode applied to the
temp file *before* the rename. This avoids a window where a concurrent
`git commit` could observe the hook with `0o600` and fail with `EACCES`.
Modes are explicit: `0o644` for config, `0o755` for executables, with
`current_mode()` preserving any pre-existing user mode bits. No
world-writable paths.

### 5. Secrets / credentials — none in tree

Repo-wide grep for `SECRET|TOKEN|PASSWORD|API_KEY` outside CI workflow YAML
(where they reference GitHub secrets correctly) and license headers turned
up no hard-coded credentials. `.gitignore` covers the standard sets
(`target/`, `node_modules/`, `.npmrc`, `__pycache__`, `.venv`, IDE files).

### 6. CI/CD — well-hardened

`.github/workflows/ci.yml` and `release.yml`:

- **Workflow-level least privilege**: `ci.yml` declares
  `permissions: contents: read`. `release.yml` keeps default-deny and grants
  `contents: write` and `id-token: write` only on the jobs that need them.
- **Third-party actions are SHA-pinned** (`dtolnay/rust-toolchain`,
  `Swatinem/rust-cache`, `astral-sh/setup-uv`, `PyO3/maturin-action`,
  `pypa/gh-action-pypi-publish`, `softprops/action-gh-release`). This is the
  GitHub-recommended posture — supply-chain compromise of any of those
  actions cannot move klasp's release line without a manual SHA bump.
- **OIDC trusted publishing**: npm publishes use `--provenance` (requires
  `id-token: write`); PyPI uses OIDC by default and only falls back to
  `PYPI_TOKEN` if explicitly configured. No long-lived publish tokens by
  default.
- **Idempotent publishes**: each `cargo publish` / `npm publish` step
  pre-checks the registry HTTP code and skips if the version already exists
  — a partial-success retry will not error, and crates.io poll-loops bound
  the wait at 180 s.
- **Tag-derived version, manifest in lockstep**: `prepare-version` extracts
  the semver from the pushed tag and `bump-source-versions.mjs` /
  `bump-npm-versions.mjs` propagate it across `Cargo.toml`, `pyproject.toml`,
  and every `package.json`. No drift between channels.

### 7. npm shim — clean

`npm/klasp/bin/klasp.js` resolves the per-platform sub-package via
`require.resolve`, then `spawnSync`s the binary with the user's `argv`. No
`postinstall`, no install-time network fetch, no shell. Sub-packages set
`os` and `cpu` so npm's optional-dep resolver picks the right one.

## Recommendations

These are best-practice nudges, not vulnerabilities.

1. **Add `cargo-audit` to CI.** Today the lockfile happens to be clean; a
   future dependency bump could land a CVE silently. A `cargo audit` step in
   `ci.yml` on every PR keeps the bar from drifting. Suggested:

   ```yaml
   - uses: rustsec/audit-check@<sha>
     with:
       token: ${{ secrets.GITHUB_TOKEN }}
   ```

2. **SHA-pin first-party `actions/*` too.** `actions/checkout@v4`,
   `actions/setup-node@v4`, `actions/setup-python@v5`,
   `actions/upload-artifact@v4`, `actions/download-artifact@v4` are tag-pinned.
   GitHub itself recommends SHA pins even for first-party actions because
   tags are mutable. Low priority — these are extremely high-trust — but
   it's the consistent hardening sweep.

3. **Add a `SECURITY.md`.** GitHub surfaces it in the Security tab and tells
   reporters how to disclose. A short file pointing at a private email or
   `gh security` advisory contact is enough.

4. **Populate `.pre-commit-config.yaml`.** The current placeholder
   (`repos: []`) is a deliberate no-op; landing at minimum a secret-scan
   hook (`gitleaks`, `detect-secrets`) would catch accidental credential
   commits before they reach the remote. The file's own comment notes this
   is a planned follow-up.

5. **Consider `cargo-deny` for license + advisory + source policy.** It
   subsumes `cargo-audit` and adds duplicate-dep detection, license
   allow-listing, and registry source checks. Optional — `cargo-audit`
   alone covers the CVE need.

## Reproduce locally

```sh
cargo install cargo-audit --locked
cargo audit
```

Expected output: `Scanning Cargo.lock for vulnerabilities (109 crate
dependencies)` followed by a clean exit (status 0).
