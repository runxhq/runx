#!/usr/bin/env node

import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const inventoryPath = path.join(root, "docs", "demo-inventory.json");
const inventory = JSON.parse(readFileSync(inventoryPath, "utf8"));
const validGroups = ["featured", "runnable_preview", "fixture_support"];
const failures = [];

if (inventory.schema !== "runx.demo_inventory.v1") {
  failures.push("docs/demo-inventory.json has an unexpected schema");
}

const examplesDir = path.join(root, "examples");
const actualExampleDirs = readdirSync(examplesDir)
  .filter((entry) => statSync(path.join(examplesDir, entry)).isDirectory())
  .map((entry) => `examples/${entry}`)
  .sort();

const classified = new Map();
for (const group of validGroups) {
  const entries = inventory[group];
  if (!Array.isArray(entries)) {
    failures.push(`docs/demo-inventory.json is missing array ${group}`);
    continue;
  }
  for (const entry of entries) {
    const itemPath = typeof entry === "string" ? entry : entry?.path;
    if (typeof itemPath !== "string" || !isValidDemoPath(itemPath)) {
      failures.push(`${group} contains invalid path ${JSON.stringify(entry)}`);
      continue;
    }
    const previous = classified.get(itemPath);
    if (previous) {
      failures.push(`${itemPath} is classified as both ${previous} and ${group}`);
    }
    classified.set(itemPath, group);
    if (!demoPathExists(itemPath)) {
      failures.push(`${itemPath} is classified but no directory exists`);
    }
    if (group !== "fixture_support" && typeof entry.command !== "string") {
      failures.push(`${itemPath} is ${group} but has no command`);
    }
  }
}

for (const dir of actualExampleDirs) {
  if (!classified.has(dir)) {
    failures.push(`${dir} is not classified in docs/demo-inventory.json`);
  }
}

const docsDemos = readFileSync(path.join(root, "docs", "demos.md"), "utf8");
const examplesReadme = readFileSync(path.join(root, "examples", "README.md"), "utf8");

for (const entry of inventory.featured ?? []) {
  if (!docsDemos.includes(entry.path)) {
    failures.push(`docs/demos.md does not mention featured demo ${entry.path}`);
  }
}

for (const dir of actualExampleDirs) {
  if (!examplesReadme.includes(dir.replace("examples/", ""))) {
    failures.push(`examples/README.md does not mention ${dir}`);
  }
}

if (failures.length > 0) {
  for (const failure of failures) {
    console.error(`[demo-inventory] ${failure}`);
  }
  process.exit(1);
}

console.log(`demo inventory covers ${actualExampleDirs.length} example directories`);

function isValidDemoPath(itemPath) {
  return itemPath.startsWith("examples/") || itemPath.startsWith("skills/");
}

function demoPathExists(itemPath) {
  if (itemPath.startsWith("examples/")) {
    return actualExampleDirs.includes(itemPath);
  }
  if (!itemPath.startsWith("skills/")) {
    return false;
  }
  const skillDir = path.join(root, itemPath);
  return path.basename(itemPath) === itemPath.slice("skills/".length)
    && hasFile(path.join(skillDir, "SKILL.md"))
    && hasFile(path.join(skillDir, "run.yaml"));
}

function hasFile(filePath) {
  try {
    return statSync(filePath).isFile();
  } catch {
    return false;
  }
}
