import { existsSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { bundledCoreSkills } from "../classify-ci-changes.mjs";

export const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const skillsRoot = path.join(workspaceRoot, "skills");
const coreSkills = new Set(bundledCoreSkills());
const entryPath = path.join(workspaceRoot, "scripts/check-runtime-architecture-boundaries.mjs");
const moduleRoot = path.join(workspaceRoot, "scripts/runtime-architecture");

export function isArchitectureCheckFile(filePath) {
  return filePath === entryPath || filePath.startsWith(`${moduleRoot}${path.sep}`);
}

export function rustFiles(root) {
  const absoluteRoot = path.join(workspaceRoot, root);
  if (!existsSync(absoluteRoot)) {
    return [];
  }
  return walk(absoluteRoot).filter((filePath) => filePath.endsWith(".rs"));
}

export function productionRustSource(source) {
  return source.split(/\n#\[cfg\(test\)\]\s*\nmod\s+tests\b/u, 1)[0] ?? source;
}

export function skillProductionFiles() {
  const skillRoot = path.join(workspaceRoot, "skills");
  if (!existsSync(skillRoot)) return [];
  const extensions = new Set([".js", ".mjs", ".cjs", ".ts"]);
  return walk(skillRoot).filter((filePath) => {
    if (!extensions.has(path.extname(filePath))) return false;
    const segments = filePath.split(path.sep);
    if (segments.some((segment) => ["fixtures", "test", ".runx"].includes(segment))) return false;
    return !/\.(?:test|spec)\.(?:js|mjs|cjs|ts)$/u.test(filePath);
  });
}

export function walk(directory) {
  const entries = readdirSync(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    // Only bundled skills belong to the CLI's architecture gate.
    if (directory === skillsRoot && !coreSkills.has(entry.name)) continue;
    if (entry.name === "target") {
      continue;
    }
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...walk(entryPath));
    } else if (entry.isFile()) {
      files.push(entryPath);
    }
  }
  return files;
}

export function countMatches(source, pattern) {
  return [...source.matchAll(pattern)].length;
}

export function splitIdentifierParts(token) {
  return token
    .replace(/([a-z0-9])([A-Z])/gu, "$1_$2")
    .replace(/([A-Z]+)([A-Z][a-z])/gu, "$1_$2")
    .toLowerCase()
    .split(/_+/u)
    .filter(Boolean);
}

export function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

export function relative(filePath) {
  return path.relative(workspaceRoot, filePath);
}
