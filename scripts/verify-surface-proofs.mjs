#!/usr/bin/env node
// Guard: every `✓` in the conformance matrix (docs/surfaces.json) is backed by
// a real, runnable test, and every agent surface crate has a matrix row.
//
// klasp's wedge is "one config, many agents," and the conformance matrix is the
// public proof of that promise. This check makes the proof MECHANICAL:
//
//   1. Every klasp-agents-* crate has a surface entry in docs/surfaces.json.
//   2. Every capability cell marked `✓` is covered by a `proofs` entry.
//   3. Every proof test file exists and contains at least one #[test] that is
//      NOT #[ignore]d (i.e. a test that actually runs).
//
// Combined with gen-agent-surfaces.mjs (the committed markdown can't drift from
// this file) and the cargo-test CI job (those tests pass), a `✓` cannot be
// committed without a real, runnable, passing test. Closes the gap where the
// old guard only checked that a row existed, not that any `✓` was load-bearing.
//
// Exit 0 = all proofs hold; 1 = at least one violation. Vanilla Node.js.

import { readFileSync, readdirSync, existsSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, "..");
const jsonPath = join(repoRoot, "docs", "surfaces.json");

// crate suffix -> matrix surface name (mirrors the LABELS map this script
// supersedes; a new crate without an entry here falls back to capitalised key).
const CRATE_LABELS = {
  claude: "Claude Code",
  codex: "Codex CLI",
  aider: "Aider",
};

const errors = [];
function err(msg) {
  errors.push(msg);
}

const data = JSON.parse(readFileSync(jsonPath, "utf8"));
const surfaceByName = new Map(data.surfaces.map((s) => [s.name, s]));

// 1. Every surface crate has a row.
const crates = readdirSync(repoRoot, { withFileTypes: true })
  .filter((d) => d.isDirectory() && d.name.startsWith("klasp-agents-"))
  .map((d) => d.name)
  .sort();
if (crates.length === 0) {
  err("no klasp-agents-* surface crates found — is this the repo root?");
}
for (const crate of crates) {
  const key = crate.replace(/^klasp-agents-/, "");
  const label =
    CRATE_LABELS[key] ?? key.charAt(0).toUpperCase() + key.slice(1);
  if (!surfaceByName.has(label)) {
    err(`surface crate ${crate} has no row in docs/surfaces.json (expected surface "${label}")`);
  } else {
    console.log(`ok ${crate} -> surface "${label}"`);
  }
}

// Cache of file path -> count of runnable (non-ignored) #[test] fns.
const runnableCache = new Map();
function runnableTestCount(relPath) {
  if (runnableCache.has(relPath)) return runnableCache.get(relPath);
  const abs = join(repoRoot, relPath);
  if (!existsSync(abs)) {
    runnableCache.set(relPath, null); // null = missing file
    return null;
  }
  const src = readFileSync(abs, "utf8");
  // Match an attribute cluster immediately preceding a fn. A test is runnable
  // when its cluster has #[test] and not #[ignore].
  const re = /((?:#\[[^\]]*\]\s*)+)fn\s+\w+/g;
  let count = 0;
  let m;
  while ((m = re.exec(src)) !== null) {
    const attrs = m[1];
    if (/#\[test\]/.test(attrs) && !/#\[ignore\b/.test(attrs)) count += 1;
  }
  runnableCache.set(relPath, count);
  return count;
}

// 2 + 3. Every `✓` cell is covered by a proof whose test file exists and has a
// runnable test.
for (const surface of data.surfaces) {
  const proofCols = new Set();
  for (const p of surface.proofs ?? []) {
    for (const c of p.columns) proofCols.add(c);
  }
  for (const [col, cell] of Object.entries(surface.cells)) {
    if (typeof cell === "string" && cell.startsWith("✓")) {
      if (!proofCols.has(col)) {
        err(`${surface.name}: column "${col}" is ✓ but has no proofs entry covering it`);
      }
    }
  }
  for (const p of surface.proofs ?? []) {
    const count = runnableTestCount(p.test);
    if (count === null) {
      err(`${surface.name}: proof test file does not exist: ${p.test}`);
    } else if (count === 0) {
      err(`${surface.name}: proof test file has no runnable (non-#[ignore]) test: ${p.test}`);
    } else {
      console.log(`ok ${surface.name}: ${p.test} (${count} runnable test(s)) backs ${p.columns.join(" / ")}`);
    }
  }
}

if (errors.length > 0) {
  console.error("\nconformance-matrix proof verification FAILED:");
  for (const e of errors) console.error(`  - ${e}`);
  console.error("\nFix docs/surfaces.json (and run `node scripts/gen-agent-surfaces.mjs --write`).");
  process.exit(1);
}

console.log(`\nall ✓ cells are backed by a runnable proof test; all ${crates.length} surface crate(s) tracked`);
