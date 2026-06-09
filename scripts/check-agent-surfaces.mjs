#!/usr/bin/env node
// Guard: every agent surface crate (klasp-agents-*) must have a row in the
// conformance matrix at docs/agent-surfaces.md.
//
// klasp's wedge is "one config, many agents." That promise is only credible if
// every surface we ship is tracked in the public contract. This check makes a
// new surface crate landing without a matrix row a CI failure instead of a
// silent gap (issue #68).
//
// Usage:  node scripts/check-agent-surfaces.mjs
//
// Exit codes:
//   0  every surface crate has a matrix row
//   1  one or more surface crates are missing a row (or the matrix is absent)
//
// Stays vanilla Node.js (no external deps) so it runs on any CI runner image,
// mirroring scripts/bump-npm-versions.mjs.

import { readFileSync, readdirSync, existsSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, "..");
const matrixPath = join(repoRoot, "docs", "agent-surfaces.md");

// Maps the `klasp-agents-<key>` crate suffix to the display label that must
// appear as a matrix row. Keeping this explicit (rather than title-casing the
// suffix) means the label can stay human-friendly ("Claude Code", "Codex CLI")
// while the crate stays terse. A surface whose key is missing here falls back
// to the capitalised key, so a brand-new crate still gets checked — it just
// needs a row whose first cell starts with that capitalised word.
const LABELS = {
  claude: "Claude Code",
  codex: "Codex CLI",
  aider: "Aider",
};

function fail(msg) {
  console.error(`error: ${msg}`);
  process.exit(1);
}

if (!existsSync(matrixPath)) {
  fail(`conformance matrix not found at ${matrixPath}`);
}

const matrix = readFileSync(matrixPath, "utf8");

// Collect the first cell of every markdown table row, lowercased and trimmed.
// A surface counts as "documented" if any row's first cell starts with its
// expected label — robust to trailing notes, links, or extra columns.
const rowHeads = matrix
  .split("\n")
  .filter((line) => line.trimStart().startsWith("|"))
  .map((line) => {
    // line looks like: | Claude Code | ✓ | ... — take the first cell.
    const cells = line.split("|");
    return (cells[1] ?? "").trim().toLowerCase();
  })
  .filter((head) => head.length > 0);

// Discover surface crates: top-level dirs named klasp-agents-*.
const surfaceCrates = readdirSync(repoRoot, { withFileTypes: true })
  .filter((d) => d.isDirectory() && d.name.startsWith("klasp-agents-"))
  .map((d) => d.name)
  .sort();

if (surfaceCrates.length === 0) {
  fail("no klasp-agents-* surface crates found — is this the repo root?");
}

const missing = [];
for (const crate of surfaceCrates) {
  const key = crate.replace(/^klasp-agents-/, "");
  const label = (
    LABELS[key] ?? key.charAt(0).toUpperCase() + key.slice(1)
  ).toLowerCase();
  const documented = rowHeads.some((head) => head.startsWith(label));
  if (!documented) {
    missing.push({ crate, label });
  } else {
    console.log(`ok ${crate} -> row "${label}" present`);
  }
}

if (missing.length > 0) {
  console.error("");
  console.error(
    "The following agent surface crates have no row in docs/agent-surfaces.md:",
  );
  for (const { crate, label } of missing) {
    console.error(`  - ${crate} (expected a matrix row starting with "${label}")`);
  }
  console.error("");
  console.error(
    "Add a row to the matrix in docs/agent-surfaces.md (and link the tests that",
  );
  console.error(
    "prove any ✓ columns). See issue #68 for why every surface is a tracked contract.",
  );
  process.exit(1);
}

console.log(
  `\nall ${surfaceCrates.length} agent surface crate(s) are tracked in docs/agent-surfaces.md`,
);
