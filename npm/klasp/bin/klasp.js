#!/usr/bin/env node
// klasp npm shim — biome-style. Resolves the per-platform sub-package and
// execs the bundled binary. No install-time download; the binary arrived
// with the optional sub-package npm picked for this host.

"use strict";

const { spawnSync } = require("node:child_process");

const PLATFORM_MAP = {
  "darwin-arm64": "@klasp-dev/klasp-darwin-arm64",
  "darwin-x64": "@klasp-dev/klasp-darwin-x64",
  "linux-x64": "@klasp-dev/klasp-linux-x64-gnu",
  "linux-arm64": "@klasp-dev/klasp-linux-arm64-gnu",
  "win32-x64": "@klasp-dev/klasp-win32-x64",
};

const key = `${process.platform}-${process.arch}`;
const pkg = PLATFORM_MAP[key];

if (!pkg) {
  process.stderr.write(
    `klasp: no prebuilt binary for ${key}. ` +
      `Supported: ${Object.keys(PLATFORM_MAP).join(", ")}.\n` +
      `Install from cargo (\`cargo install klasp\`) or file an issue at ` +
      `https://github.com/klasp-dev/klasp/issues.\n`,
  );
  process.exit(1);
}

const ext = process.platform === "win32" ? ".exe" : "";
let binary;
try {
  binary = require.resolve(`${pkg}/klasp${ext}`);
} catch (err) {
  process.stderr.write(
    `klasp: optional dependency ${pkg} is not installed. ` +
      `Re-run \`npm install\` (or \`npm install --include=optional\`).\n` +
      `Original error: ${err && err.message ? err.message : err}\n`,
  );
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });

if (result.error) {
  process.stderr.write(`klasp: failed to spawn ${binary}: ${result.error.message}\n`);
  process.exit(1);
}

process.exit(result.status === null ? 1 : result.status);
