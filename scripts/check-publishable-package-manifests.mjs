import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const dependencySections = [
  "dependencies",
  "peerDependencies",
  "optionalDependencies",
];

const packages = await readWorkspacePackages();
const versions = new Map(
  packages.map(({ manifest }) => [manifest.name, manifest.version]),
);

for (const { directory, manifest: sourceManifest } of packages) {
  if (sourceManifest.private === true) continue;
  const manifest = rewriteManifestForPublish(sourceManifest, versions);
  for (const sectionName of dependencySections) {
    const section = manifest[sectionName];
    if (!section || typeof section !== "object") {
      continue;
    }
    for (const [dependencyName, spec] of Object.entries(section)) {
      if (typeof spec === "string" && spec.startsWith("workspace:")) {
        throw new Error(`${path.basename(directory)} ${sectionName}.${dependencyName} still rewrites to ${spec}.`);
      }
    }
  }
}

async function readWorkspacePackages() {
  const packages = [];
  const packageRoot = path.join(workspaceRoot, "packages");
  for (const entry of await readdir(packageRoot, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const directory = path.join(packageRoot, entry.name);
    try {
      const manifest = JSON.parse(await readFile(path.join(directory, "package.json"), "utf8"));
      if (typeof manifest.name === "string" && typeof manifest.version === "string") {
        packages.push({ directory, manifest });
      }
    } catch (error) {
      if (!isMissingFile(error)) throw error;
    }
  }
  return packages.sort((left, right) => left.directory.localeCompare(right.directory));
}

function rewriteManifestForPublish(manifest, versions) {
  const next = structuredClone(manifest);
  for (const sectionName of [...dependencySections, "devDependencies"]) {
    const section = next[sectionName];
    if (!isRecord(section)) continue;
    next[sectionName] = Object.fromEntries(
      Object.entries(section).map(([dependencyName, spec]) => [
        dependencyName,
        typeof spec === "string" ? rewriteWorkspaceProtocol(dependencyName, spec, versions) : spec,
      ]),
    );
  }
  return next;
}

function rewriteWorkspaceProtocol(dependencyName, spec, versions) {
  if (!spec.startsWith("workspace:")) return spec;
  const version = versions.get(dependencyName);
  if (!version) throw new Error(`Unable to resolve workspace version for ${dependencyName}.`);
  const requested = spec.slice("workspace:".length).trim();
  if (requested === "" || requested === "*" || requested === version) return version;
  if (requested === "^") return `^${version}`;
  if (requested === "~") return `~${version}`;
  const operator = requested[0];
  if ((operator === "^" || operator === "~") && allowsVersion(requested.slice(1), version, operator)) {
    return `${operator}${version}`;
  }
  throw new Error(`${dependencyName} ${spec} does not allow workspace version ${version}.`);
}

function allowsVersion(floor, current, operator) {
  const base = parseStableVersion(floor);
  const candidate = parseStableVersion(current);
  if (!base || !candidate) return false;
  const [major, minor, patch] = base;
  const [actualMajor, actualMinor, actualPatch] = candidate;
  if (actualMajor !== major || actualMinor < minor || (actualMinor === minor && actualPatch < patch)) {
    return false;
  }
  if (operator === "~") return actualMinor === minor;
  if (major > 0) return true;
  if (minor > 0) return actualMinor === minor;
  return actualMinor === 0 && actualPatch === patch;
}

function parseStableVersion(value) {
  if (!/^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/u.test(value)) return null;
  return value.split(".").map(Number);
}

function isRecord(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isMissingFile(error) {
  return Boolean(error && typeof error === "object" && "code" in error && error.code === "ENOENT");
}
