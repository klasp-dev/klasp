#!/usr/bin/env node
// Generator: render the conformance matrix + proof tables in
// docs/agent-surfaces.md from the single source of truth docs/surfaces.json.
//
// The matrix used to be hand-maintained markdown, which let a `✓` drift from
// the test that backs it. Now the tables are GENERATED from surfaces.json and
// CI fails if the committed markdown doesn't match (`--check`). Combined with
// scripts/verify-surface-proofs.mjs (every `✓` has a real proof test) and the
// cargo-test job (those tests actually pass), a `✓` cannot lie.
//
// Usage:
//   node scripts/gen-agent-surfaces.mjs            # --check: exit 1 on drift
//   node scripts/gen-agent-surfaces.mjs --check    # same as default
//   node scripts/gen-agent-surfaces.mjs --write    # rewrite the markdown
//
// Vanilla Node.js, no deps (mirrors the other scripts/ tools).

import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(__dirname, "..");
const jsonPath = join(repoRoot, "docs", "surfaces.json");
const mdPath = join(repoRoot, "docs", "agent-surfaces.md");

const mode = process.argv.includes("--write") ? "write" : "check";

const data = JSON.parse(readFileSync(jsonPath, "utf8"));
const { capabilities, surfaces } = data;

function row(cells) {
  return `| ${cells.join(" | ")} |`;
}

function renderMatrix() {
  const header = row(["Surface", ...capabilities, "Notes"]);
  const sep = row(Array(capabilities.length + 2).fill("---"));
  const body = surfaces.map((s) =>
    row([s.name, ...capabilities.map((c) => s.cells[c] ?? "—"), s.notes ?? "—"]),
  );
  return [header, sep, ...body].join("\n");
}

function renderProofs() {
  const header = row(["Surface", "Columns proven", "Test file(s)"]);
  const sep = row(["---", "---", "---"]);
  const body = [];
  for (const s of surfaces) {
    for (const p of s.proofs ?? []) {
      const cols = p.columns.join(" / ");
      const link = `[\`${p.test}\`](../${p.test})`;
      body.push(row([s.name, cols, link]));
    }
  }
  return [header, sep, ...body].join("\n");
}

function escapeRe(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function replaceBlock(src, id, body) {
  const begin = `<!-- BEGIN GENERATED:${id} -->`;
  const end = `<!-- END GENERATED:${id} -->`;
  const re = new RegExp(`${escapeRe(begin)}[\\s\\S]*?${escapeRe(end)}`);
  if (!re.test(src)) {
    console.error(
      `error: marker block "${id}" not found in docs/agent-surfaces.md`,
    );
    process.exit(1);
  }
  return src.replace(re, `${begin}\n${body}\n${end}`);
}

const current = readFileSync(mdPath, "utf8");
let next = current;
next = replaceBlock(next, "matrix", renderMatrix());
next = replaceBlock(next, "proofs", renderProofs());

if (mode === "write") {
  if (next !== current) {
    writeFileSync(mdPath, next);
    console.log("docs/agent-surfaces.md regenerated from docs/surfaces.json");
  } else {
    console.log("docs/agent-surfaces.md already up to date");
  }
} else {
  if (next !== current) {
    console.error(
      "error: docs/agent-surfaces.md is out of sync with docs/surfaces.json.",
    );
    console.error("Run: node scripts/gen-agent-surfaces.mjs --write");
    process.exit(1);
  }
  console.log("ok docs/agent-surfaces.md matches docs/surfaces.json");
}
