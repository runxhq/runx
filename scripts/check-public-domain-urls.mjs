#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { bundledCoreSkills } from "./classify-ci-changes.mjs";

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const coreSkills = process.argv.includes("--core-only") ? new Set(bundledCoreSkills()) : null;
const ignoredFileExtensions = new Set([
  ".ico",
  ".jpg",
  ".jpeg",
  ".png",
  ".webp",
  ".woff",
  ".woff2",
]);

const staleDomainPattern = /(?<![A-Za-z0-9_])(?:https?:\/\/)?(?:schemas\.)?runx\.dev(?!\.v1)(?:\b|\/)/g;
const failures = [];

for (const relativePath of trackedFiles()) {
  scanFile(path.join(workspaceRoot, relativePath));
}

if (failures.length > 0) {
  console.error("Found stale public domain references. Use runx.ai instead.");
  for (const failure of failures) {
    console.error(`${failure.file}:${failure.line}:${failure.column}: ${failure.match}`);
  }
  process.exit(1);
}

console.log("checked public domain URLs");

function trackedFiles() {
  return execFileSync("git", ["ls-files"], { cwd: workspaceRoot, encoding: "utf8" })
    .split("\n")
    .filter(Boolean)
    .filter((file) => !coreSkills || !file.startsWith("skills/") || coreSkills.has(file.split("/")[1]));
}

function scanFile(filePath) {
  if (ignoredFileExtensions.has(path.extname(filePath).toLowerCase())) {
    return;
  }

  const stat = statSync(filePath, { throwIfNoEntry: false });
  if (!stat) {
    return;
  }
  if (stat.size > 5 * 1024 * 1024) {
    return;
  }

  let source;
  try {
    source = readFileSync(filePath, "utf8");
  } catch {
    return;
  }

  staleDomainPattern.lastIndex = 0;
  for (let match = staleDomainPattern.exec(source); match; match = staleDomainPattern.exec(source)) {
    failures.push({
      file: path.relative(workspaceRoot, filePath),
      line: lineNumber(source, match.index),
      column: columnNumber(source, match.index),
      match: match[0],
    });
  }
}

function lineNumber(source, offset) {
  return source.slice(0, offset).split("\n").length;
}

function columnNumber(source, offset) {
  const lineStart = source.lastIndexOf("\n", offset - 1) + 1;
  return offset - lineStart + 1;
}
