#!/usr/bin/env node
// Bump the workspace Cargo.toml version and pypi/pyproject.toml version to
// the supplied semver string. Called from the release workflow; not part of
// the per-PR CI.
//
// Why a script instead of `cargo set-version`? cargo-edit isn't a default
// tool and pulling it in for one-line bumps is over-engineering. We do
// targeted regex replacements on lines we own (the workspace.package.version
// line and the project.version line) — both are stable, single-occurrence,
// and easy to anchor.
//
// Also bumps the path-dependency version specifiers in
// klasp-agents-claude/Cargo.toml and klasp/Cargo.toml so that
// `cargo publish` accepts the dependency once klasp-core lands on the
// registry at the new version.
//
// Usage:  node scripts/bump-source-versions.mjs 0.1.0

import { readFileSync, writeFileSync } from "node:fs";
import { resolve, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, "..");

const version = process.argv[2];
if (!version) {
  console.error("usage: bump-source-versions.mjs <version>");
  process.exit(1);
}

if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`error: '${version}' is not a valid semver string`);
  process.exit(1);
}

// Whitelist the tag forms this project ships. Anything else (e.g. -pre.N,
// -dev.N, +sha.abc) would either be silently mangled by the PEP 440
// normalisation below or produce a string PyPI rejects outright.
// Supported: X.Y.Z  |  X.Y.Z-rc.N  |  X.Y.Z-alpha.N  |  X.Y.Z-beta.N
const SUPPORTED_TAG_RE =
  /^\d+\.\d+\.\d+(?:-(rc|alpha|beta)\.\d+)?$/;
if (!SUPPORTED_TAG_RE.test(version)) {
  console.error(
    `error: Unsupported tag format '${version}'. ` +
      "Supported: X.Y.Z | X.Y.Z-rc.N | X.Y.Z-alpha.N | X.Y.Z-beta.N"
  );
  process.exit(1);
}

function patchFile(path, replacements) {
  const original = readFileSync(path, "utf8");
  let modified = original;
  for (const { description, find, replace } of replacements) {
    if (!find.test(modified)) {
      console.error(`error: ${path}: pattern not found for ${description}`);
      process.exit(1);
    }
    modified = modified.replace(find, replace);
  }
  if (modified === original) {
    console.error(`warn: ${path}: no changes`);
    return;
  }
  writeFileSync(path, modified);
  console.log(`patched ${path}`);
}

// Workspace root Cargo.toml: bump `[workspace.package] version = "..."`.
patchFile(join(repoRoot, "Cargo.toml"), [
  {
    description: "workspace.package.version",
    find: /^version\s*=\s*"[^"]*"\s*$/m,
    replace: `version = "${version}"`,
  },
]);

// PyPI pyproject.toml: bump `[project] version = "..."`.
// PEP 440 prefers `1.0.0rc1` over `1.0.0-rc.1`. We pass the raw semver in
// here; if you want PEP 440 normalisation, do it in CI before calling.
const pypiVersion = version
  .replace(/-rc\.?/i, "rc")
  .replace(/-alpha\.?/i, "a")
  .replace(/-beta\.?/i, "b");
patchFile(join(repoRoot, "pypi", "pyproject.toml"), [
  {
    description: "project.version",
    find: /^version\s*=\s*"[^"]*"\s*$/m,
    replace: `version = "${pypiVersion}"`,
  },
]);

// Path-dependency version specifiers — these line up the published
// crate version with the dependency declared in downstream crates so
// `cargo publish` accepts them.
patchFile(join(repoRoot, "klasp-agents-claude", "Cargo.toml"), [
  {
    description: "klasp-core dep version",
    find: /klasp-core\s*=\s*\{\s*path\s*=\s*"\.\.\/klasp-core"\s*,\s*version\s*=\s*"[^"]*"\s*\}/,
    replace: `klasp-core = { path = "../klasp-core", version = "${version}" }`,
  },
]);

patchFile(join(repoRoot, "klasp", "Cargo.toml"), [
  {
    description: "klasp-core dep version",
    find: /klasp-core\s*=\s*\{\s*path\s*=\s*"\.\.\/klasp-core"\s*,\s*version\s*=\s*"[^"]*"\s*\}/,
    replace: `klasp-core = { path = "../klasp-core", version = "${version}" }`,
  },
  {
    description: "klasp-agents-claude dep version",
    find: /klasp-agents-claude\s*=\s*\{\s*path\s*=\s*"\.\.\/klasp-agents-claude"\s*,\s*version\s*=\s*"[^"]*"\s*\}/,
    replace: `klasp-agents-claude = { path = "../klasp-agents-claude", version = "${version}" }`,
  },
]);

console.log(`bumped source manifests to version ${version} (pypi: ${pypiVersion})`);
