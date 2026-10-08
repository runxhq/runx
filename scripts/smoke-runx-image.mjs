#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";

const options = parseArgs(process.argv.slice(2));
const root = mkdtempSync(path.join(os.tmpdir(), "runx-image-smoke-"));
const skill = path.join(root, "skill");
const receipts = path.join(root, "receipts");

try {
  mkdirSync(skill);
  mkdirSync(receipts);
  mkdirSync(path.join(root, "home"));
  writeFileSync(path.join(skill, "SKILL.md"), `---
name: image-smoke
description: Verify the published image runs a JavaScript-backed skill.
---

# Image smoke
`);
  writeFileSync(path.join(skill, "X.yaml"), `skill: image-smoke
runners:
  run:
    default: true
    type: javascript
    module: probe.mjs
    outputs:
      value: string
`);
  writeFileSync(path.join(skill, "probe.mjs"), 'export default () => ({ value: "image-worker-ok" });\n');

  const version = runImage(["--version"]).trim();
  if (!version.includes(options.expectedVersion)) {
    throw new Error(`image version mismatch: ${version}`);
  }
  const output = JSON.parse(runImage([
    "skill", "/workspace/skill", "--receipt-dir", "/workspace/receipts", "--json",
  ]));
  if (output.status !== "sealed"
    || !JSON.stringify(output).includes("image-worker-ok")
    || !/^sha256:[0-9a-f]{64}$/u.test(output.receipt_id ?? "")) {
    throw new Error(`image skill did not seal the JavaScript result: ${JSON.stringify(output)}`);
  }
  const receiptFiles = readdirSync(receipts, { recursive: true })
    .filter((entry) => entry.endsWith(".json") && entry !== "index.json");
  if (receiptFiles.length === 0) {
    throw new Error("image skill produced no receipt file");
  }
  process.stdout.write(`${JSON.stringify({ status: "passed", image: options.image, version, js_worker: true, receipt: true })}\n`);
} finally {
  rmSync(root, { recursive: true, force: true });
}

function runImage(args) {
  const output = execFileSync("docker", [
    "run", "--rm", "--network", "none",
    "--user", `${process.getuid()}:${process.getgid()}`,
    "--mount", `type=bind,source=${root},target=/workspace`,
    "--workdir", "/workspace",
    "--env", "HOME=/workspace/home",
    "--env", "RUNX_HOME=/workspace/home",
    options.image,
    ...args,
  ], { encoding: "utf8", timeout: 90_000, maxBuffer: 16 * 1024 * 1024 });
  return output;
}

function parseArgs(args) {
  let image = "";
  let expectedVersion = "";
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--image") { image = args[++index] ?? ""; continue; }
    if (args[index] === "--expected-version") { expectedVersion = args[++index] ?? ""; continue; }
    throw new Error(`unknown argument: ${args[index]}`);
  }
  if (!/^ghcr\.io\/runxhq\/runx@sha256:[0-9a-f]{64}$/u.test(image)) {
    throw new Error("--image requires the immutable GHCR image digest");
  }
  if (!/^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/u.test(expectedVersion)) {
    throw new Error("--expected-version requires stable semver");
  }
  return { image, expectedVersion };
}
