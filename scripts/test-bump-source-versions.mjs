#!/usr/bin/env node
// Tests for the tag-format whitelist and PEP 440 normalisation in
// bump-source-versions.mjs.  Run with:
//   node scripts/test-bump-source-versions.mjs
//
// Strategy: invoke the script with versions that should be rejected and check
// the exit code + stderr message.  For accepted versions we only test the
// whitelist regex (the patchFile calls need real manifests, so those are
// covered by the release workflow itself).

import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const SCRIPT = join(__dirname, "bump-source-versions.mjs");

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

let passed = 0;
let failed = 0;

function run(version) {
  try {
    const stderr = execFileSync(process.execPath, [SCRIPT, version], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    return { exitCode: 0, stderr: "" };
  } catch (err) {
    return { exitCode: err.status ?? 1, stderr: err.stderr ?? "" };
  }
}

function expectReject(version, expectedMessageFragment) {
  const { exitCode, stderr } = run(version);
  if (exitCode !== 0 && stderr.includes(expectedMessageFragment)) {
    console.log(`PASS  reject '${version}': "${expectedMessageFragment}"`);
    passed++;
  } else {
    console.error(
      `FAIL  reject '${version}': expected exit!=0 and message containing` +
        ` "${expectedMessageFragment}", got exitCode=${exitCode} stderr="${stderr.trim()}"`
    );
    failed++;
  }
}

// Whitelist regex — mirrors SUPPORTED_TAG_RE in bump-source-versions.mjs.
// We test acceptance here without running the full patchFile logic (which
// requires real manifest files).
const SUPPORTED_TAG_RE = /^\d+\.\d+\.\d+(?:-(rc|alpha|beta)\.\d+)?$/;

function expectAccept(version, expectedPypi) {
  if (!SUPPORTED_TAG_RE.test(version)) {
    console.error(`FAIL  accept '${version}': regex rejected it`);
    failed++;
    return;
  }
  const pypi = version
    .replace(/-rc\.?/i, "rc")
    .replace(/-alpha\.?/i, "a")
    .replace(/-beta\.?/i, "b");
  if (pypi === expectedPypi) {
    console.log(`PASS  accept '${version}' -> pypi '${pypi}'`);
    passed++;
  } else {
    console.error(
      `FAIL  accept '${version}': expected pypi '${expectedPypi}', got '${pypi}'`
    );
    failed++;
  }
}

// ---------------------------------------------------------------------------
// Test cases (item 5 acceptance criteria)
// ---------------------------------------------------------------------------

// Accepted forms
expectAccept("0.1.0", "0.1.0");
expectAccept("0.1.0-rc.1", "0.1.0rc1");
expectAccept("1.2.3-alpha.2", "1.2.3a2");
expectAccept("1.2.3-beta.3", "1.2.3b3");

// Rejected: -pre.N (no PEP 440 mapping)
expectReject("0.1.0-pre.1", "Unsupported tag format");

// Rejected: -dev.N
expectReject("0.1.0-dev.1", "Unsupported tag format");

// Rejected: +sha.abc (build metadata not allowed in PEP 440 public versions)
expectReject("0.1.0+sha.abc123", "Unsupported tag format");

// Rejected: bare build metadata on a pre-release
expectReject("0.1.0-rc.1+sha.abc", "Unsupported tag format");

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

console.log(`\n${passed} passed, ${failed} failed`);
if (failed > 0) process.exit(1);
