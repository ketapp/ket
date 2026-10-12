#!/usr/bin/env node
// `npm audit --audit-level=high` for one package directory, minus the
// advisories listed in `.github/npm-audit-allow.json`.
//
// npm has no ignore list of its own, and an advisory with no patched release
// anywhere would otherwise hold every run red until upstream ships one. Each
// allowance names the advisory, the package and why it is safe to carry; one
// that npm no longer reports is called out so it can be deleted.
//
// Usage: node .github/scripts/npm-audit.mjs <dir>

import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const dir = process.argv[2];
if (!dir) {
  console.error("usage: npm-audit.mjs <dir>");
  process.exit(2);
}

const here = dirname(fileURLToPath(import.meta.url));
const allow = JSON.parse(readFileSync(join(here, "..", "npm-audit-allow.json"), "utf8"));
const allowed = new Map(
  allow.filter((entry) => entry.dir === dir).map((entry) => [entry.id, entry]),
);

// npm exits non-zero whenever it finds anything, so the status says nothing;
// the JSON is the answer.
const run = spawnSync("npm", ["audit", "--json", "--prefix", dir], {
  encoding: "utf8",
  maxBuffer: 64 * 1024 * 1024,
});
let report;
try {
  report = JSON.parse(run.stdout);
} catch {
  console.error(run.stderr || run.stdout);
  console.error(`npm audit for ${dir} did not return JSON`);
  process.exit(2);
}
if (report.error) {
  console.error(JSON.stringify(report.error, null, 2));
  process.exit(2);
}

const serious = new Set(["high", "critical"]);
const found = new Map();
for (const [name, vulnerability] of Object.entries(report.vulnerabilities ?? {})) {
  for (const via of vulnerability.via) {
    if (typeof via !== "object" || !serious.has(via.severity)) continue;
    const id = via.url.split("/").pop();
    found.set(id, { id, name, severity: via.severity, title: via.title, url: via.url });
  }
}

const blocking = [...found.values()].filter((advisory) => !allowed.has(advisory.id));
for (const advisory of found.values()) {
  const entry = allowed.get(advisory.id);
  if (entry) console.log(`allowed  ${advisory.id} ${advisory.name}: ${entry.reason}`);
}
for (const id of allowed.keys()) {
  if (!found.has(id)) console.log(`stale    ${id} is no longer reported; remove it from npm-audit-allow.json`);
}
for (const advisory of blocking) {
  console.log(`${advisory.severity.padEnd(8)} ${advisory.id} ${advisory.name}: ${advisory.title} ${advisory.url}`);
}

if (blocking.length > 0) {
  console.error(`${blocking.length} high or critical advisor${blocking.length === 1 ? "y" : "ies"} in ${dir}`);
  process.exit(1);
}
console.log(`${dir}: no unallowed high or critical advisories`);
