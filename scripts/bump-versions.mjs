#!/usr/bin/env node
// One-shot version bump: sets the same version across the Cargo workspace,
// pypi/pyproject.toml, and every npm package.json (plus the wrapper's
// @klasp-dev/* optionalDependencies), then verifies they agree.
//
// Replaces the two-step convention (run bump-source-versions.mjs AND
// bump-npm-versions.mjs, hope you didn't forget one) with a single atomic
// command. The release workflow should call this instead of the two scripts.
//
// Usage:  node scripts/bump-versions.mjs <version>     e.g. 0.6.0 | 0.6.0-rc.1
// Vanilla Node.js, no deps.

import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));

const version = process.argv[2];
if (!version) {
  console.error("usage: bump-versions.mjs <version>");
  process.exit(1);
}

function run(script, args) {
  console.log(`\n$ node ${script} ${args.join(" ")}`);
  const r = spawnSync(process.execPath, [join(__dirname, script), ...args], {
    stdio: "inherit",
  });
  if (r.status !== 0) {
    console.error(`error: ${script} exited ${r.status}`);
    process.exit(r.status ?? 1);
  }
}

run("bump-source-versions.mjs", [version]); // Cargo + pypi (+ path-deps)
run("bump-npm-versions.mjs", [version]); // every npm package.json
run("check-version-sync.mjs", []); // assert they now agree

console.log(`\nall manifests bumped to ${version}`);
