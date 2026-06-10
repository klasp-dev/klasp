#!/usr/bin/env node
// Invariant guard: the version is declared in three places — the Cargo
// workspace, every npm package.json, and pypi/pyproject.toml — bumped by two
// separate scripts. Nothing asserted they agree, so a half-finished release
// (npm wrapper at X, platform packages or pypi at Y) could ship broken. This
// check makes divergence a CI failure.
//
// Source of truth: Cargo `[workspace.package].version`.
//   - npm package.json `version` (and the main package's @klasp-dev/*
//     optionalDependencies) must equal it verbatim (npm uses raw semver).
//   - pypi `[project].version` must equal its PEP 440 transform
//     (e.g. 1.2.3-rc.1 -> 1.2.3rc1), reusing the release script's own mapping.
//
// Usage:  node scripts/check-version-sync.mjs   (exit 0 = in sync, 1 = drift)
// Vanilla Node.js, no deps.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { toPypiVersion } from "./bump-source-versions.mjs";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, "..");

const errors = [];
const note = (m) => errors.push(m);

function firstTomlVersion(relPath) {
  const src = readFileSync(join(repoRoot, relPath), "utf8");
  const m = src.match(/^version\s*=\s*"([^"]*)"\s*$/m);
  if (!m) {
    note(`${relPath}: no top-level version line found`);
    return null;
  }
  return m[1];
}

// Source of truth.
const cargoVersion = firstTomlVersion("Cargo.toml");
const expectedPypi = cargoVersion === null ? null : toPypiVersion(cargoVersion);

// pypi.
const pypiVersion = firstTomlVersion("pypi/pyproject.toml");
if (cargoVersion !== null && pypiVersion !== null && pypiVersion !== expectedPypi) {
  note(`pypi/pyproject.toml version "${pypiVersion}" != expected "${expectedPypi}" (from Cargo "${cargoVersion}")`);
}

// npm: every package.json under npm/.
const npmRoot = join(repoRoot, "npm");
for (const entry of readdirSync(npmRoot)) {
  if (entry === "node_modules") continue;
  const dir = join(npmRoot, entry);
  if (!statSync(dir).isDirectory()) continue;
  let pkgPath = join(dir, "package.json");
  let pkg;
  try {
    pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
  } catch {
    continue; // no package.json here
  }
  const rel = `npm/${entry}/package.json`;
  if (cargoVersion !== null && pkg.version !== cargoVersion) {
    note(`${rel} version "${pkg.version}" != Cargo "${cargoVersion}"`);
  }
  for (const [dep, spec] of Object.entries(pkg.optionalDependencies ?? {})) {
    if (dep.startsWith("@klasp-dev/") && spec !== cargoVersion) {
      note(`${rel} optionalDependencies["${dep}"] = "${spec}" != Cargo "${cargoVersion}"`);
    }
  }
  if (!errors.some((e) => e.startsWith(rel))) {
    console.log(`ok ${rel} @ ${pkg.version}`);
  }
}

if (errors.length > 0) {
  console.error("\nversion drift detected:");
  for (const e of errors) console.error(`  - ${e}`);
  console.error("\nRe-sync with: node scripts/bump-versions.mjs <version>");
  process.exit(1);
}

console.log(`\nall manifests agree: Cargo/npm = ${cargoVersion}, pypi = ${expectedPypi}`);
