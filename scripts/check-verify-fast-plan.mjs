#!/usr/bin/env node
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const source = readFileSync(path.join(workspaceRoot, "scripts", "verify-fast.mjs"), "utf8");
const parallelSourceGroup = sliceBetween(
  source,
  'await runParallelGroup("source checks"',
  'await runSerialGroup("rust structure checks"',
);

for (const forbidden of [
  "rustfmt",
  "rust:crate-graph",
  "build rust binaries",
  "test:fast",
]) {
  if (parallelSourceGroup.includes(forbidden)) {
    throw new Error(`verify:fast launches ${forbidden} inside the parallel source-check group`);
  }
}

for (const required of [
  'step("readiness structural guard"',
  'step("demo inventory guard"',
  'step("release version sync"',
  'step("rustfmt"',
  'step("runtime architecture"',
  'step("deterministic module engine decision"',
  'step("core skill version drift"',
  'step("docs:api:check"',
  'await runSerialGroup("rust structure checks"',
  'step("build rust binaries"',
  'step("build workspace"',
]) {
  if (!source.includes(required)) {
    throw new Error(`verify:fast is missing required serialized step marker: ${required}`);
  }
}

console.log("verify:fast plan keeps release drift checks early and Rust-heavy checks serialized.");

function sliceBetween(contents, start, end) {
  const startIndex = contents.indexOf(start);
  if (startIndex === -1) {
    throw new Error(`missing start marker: ${start}`);
  }
  const endIndex = contents.indexOf(end, startIndex);
  if (endIndex === -1) {
    throw new Error(`missing end marker: ${end}`);
  }
  return contents.slice(startIndex, endIndex);
}
