#!/usr/bin/env node
// Bump every npm package.json under npm/ to the version supplied as argv[2].
//
// The main package's optionalDependencies entries are bumped in lockstep so
// that `@klasp-dev/klasp@X.Y.Z` resolves to `@klasp-dev/klasp-darwin-arm64@X.Y.Z`
// (and the rest), not whatever happened to be tagged latest on the registry.
//
// Usage:  node scripts/bump-npm-versions.mjs 0.1.0
//
// Called from .github/workflows/release.yml after deriving VERSION from the
// pushed tag. Stays vanilla Node.js (no external deps) so it can run on any
// CI runner image.

import { readFileSync, writeFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, "..");
const npmRoot = join(repoRoot, "npm");

const version = process.argv[2];
if (!version) {
  console.error("usage: bump-npm-versions.mjs <version>");
  process.exit(1);
}

if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`error: '${version}' is not a valid semver string`);
  process.exit(1);
}

function findPackageJsons(root) {
  const entries = readdirSync(root);
  const found = [];
  for (const entry of entries) {
    if (entry === "node_modules") continue;
    const full = join(root, entry);
    const st = statSync(full);
    if (st.isDirectory()) {
      const candidate = join(full, "package.json");
      try {
        statSync(candidate);
        found.push(candidate);
      } catch {
        // no package.json at this level — skip
      }
    }
  }
  return found;
}

const pkgFiles = findPackageJsons(npmRoot);
if (pkgFiles.length === 0) {
  console.error(`error: no package.json files found under ${npmRoot}`);
  process.exit(1);
}

for (const file of pkgFiles) {
  const raw = readFileSync(file, "utf8");
  const pkg = JSON.parse(raw);
  pkg.version = version;

  if (pkg.optionalDependencies) {
    for (const dep of Object.keys(pkg.optionalDependencies)) {
      if (dep.startsWith("@klasp-dev/")) {
        pkg.optionalDependencies[dep] = version;
      }
    }
  }

  writeFileSync(file, `${JSON.stringify(pkg, null, 2)}\n`);
  console.log(`bumped ${pkg.name} -> ${version}`);
}
