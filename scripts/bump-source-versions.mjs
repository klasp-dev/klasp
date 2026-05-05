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
// Also bumps every path-dependency version specifier across the workspace so
// `cargo publish` accepts each dep once its target crate lands on the registry
// at the new version. The walker below auto-discovers member crates — adding
// a new workspace member requires no script change.
//
// Usage:  node scripts/bump-source-versions.mjs 0.1.0

import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
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

// Path-dependency version specifiers — line up the published crate version
// with dep declarations in downstream crates so `cargo publish` accepts them.
// Walks every workspace member's Cargo.toml and bumps every path-dep's version
// specifier. Adding a new member crate (e.g. `klasp-agents-codex` in W2)
// requires no script change.
const PATH_DEP_RE =
  /(\b[\w-]+\s*=\s*\{\s*path\s*=\s*"[^"]+"\s*,\s*version\s*=\s*")[^"]*("\s*\})/g;

const EXCLUDED_DIRS = new Set(["target", "node_modules"]);

function isMemberDir(entry, root) {
  if (!entry.isDirectory()) return false;
  if (entry.name.startsWith(".") || EXCLUDED_DIRS.has(entry.name)) return false;
  return existsSync(join(root, entry.name, "Cargo.toml"));
}

const memberCargoTomls = readdirSync(repoRoot, { withFileTypes: true })
  .filter((e) => isMemberDir(e, repoRoot))
  .map((e) => join(repoRoot, e.name, "Cargo.toml"));

let touched = 0;
for (const cargoToml of memberCargoTomls) {
  const original = readFileSync(cargoToml, "utf8");
  const modified = original.replace(PATH_DEP_RE, `$1${version}$2`);
  if (modified !== original) {
    writeFileSync(cargoToml, modified);
    console.log(`patched path-deps in ${cargoToml}`);
    touched++;
  }
}
if (touched === 0) {
  console.log("path-deps already at target version (nothing to patch)");
}

console.log(`bumped source manifests to version ${version} (pypi: ${pypiVersion})`);
