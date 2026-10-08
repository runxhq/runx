#!/usr/bin/env node
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { evaluateParserRequestResults } from "./lib/native-parser.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const currentProfiles = new Map();

if (process.argv.includes("--self-test")) {
  runSelfTests();
  process.exit(0);
}

const staticFindings = checkCurrentCatalog();
const base = resolveBase();
const driftFindings = base ? checkVersionDrift(base) : [];
const findings = [...staticFindings, ...driftFindings];

if (findings.length > 0) {
  for (const finding of findings) console.error(finding);
  process.exit(1);
}

const comparison = base ? ` against ${base}` : " (version comparison skipped: no Git base)";
console.log(`skill catalog version and graph-input checks ok${comparison}`);

function checkCurrentCatalog() {
  const findings = [];
  const skillsRoot = path.join(root, "skills");
  const documents = [];
  for (const entry of readdirSync(skillsRoot, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const profilePath = path.join(skillsRoot, entry.name, "run.yaml");
    if (!existsSync(profilePath)) continue;
    const source = readFileSync(profilePath, "utf8");
    documents.push({ skill: entry.name, source });
  }
  const results = evaluateParserRequestResults(documents.map(({ source }) => ({
    kind: "parser.validateRunnerManifestYaml",
    yaml: source,
  })));
  for (const [index, document] of documents.entries()) {
    const result = results[index];
    if (result?.status !== "success") {
      findings.push(`skills/${document.skill}/run.yaml: ${result?.error?.message ?? "invalid runner manifest"}`);
      continue;
    }
    const profile = result.value;
    currentProfiles.set(document.skill, profile);
    if (!isSemver(profile?.version)) {
      findings.push(`skills/${document.skill}/run.yaml: version must be quoted semantic version x.y.z`);
    } else if (!document.source.split("\n").some((line) => line === `version: "${profile.version}"`)) {
      findings.push(`skills/${document.skill}/run.yaml: version must be quoted as "${profile.version}"`);
    }
  }
  return findings;
}

function checkVersionDrift(base) {
  const candidatePaths = new Map();
  for (const changedPath of changedSkillPaths(base)) {
    const parts = changedPath.split("/");
    if (parts.length < 3 || parts[0] !== "skills") continue;
    const skill = parts[1];
    const relative = parts.slice(2).join("/");
    if (!skill || !relative) continue;
    const paths = candidatePaths.get(skill) ?? [];
    paths.push(changedPath);
    candidatePaths.set(skill, paths);
  }
  const baseProfiles = loadBaseProfiles(base, [...candidatePaths.keys()]);
  const changedBySkill = new Map();
  for (const [skill, paths] of candidatePaths) {
    for (const changedPath of paths) {
      const relative = changedPath.split("/").slice(2).join("/");
      if (!isPublishableRelative(relative)) {
        continue;
      }
      const publishable = changedBySkill.get(skill) ?? [];
      publishable.push(changedPath);
      changedBySkill.set(skill, publishable);
    }
  }

  const findings = [];
  for (const [skill, changedPaths] of changedBySkill) {
    const baseVersion = skillVersion(baseProfiles.get(skill));
    const currentVersion = skillVersion(currentProfiles.get(skill));
    const finding = versionFinding(skill, baseVersion, currentVersion, changedPaths);
    if (finding) findings.push(finding);
  }
  return findings;
}

function changedSkillPaths(base) {
  const output = git(["diff", "--name-status", "--find-renames", base, "--", "skills"]);
  const paths = new Set();
  for (const line of output.split("\n")) {
    if (!line.trim()) continue;
    const fields = line.split("\t");
    const status = fields[0] ?? "";
    if (status.startsWith("R") || status.startsWith("C")) {
      if (fields[1]) paths.add(fields[1]);
      if (fields[2]) paths.add(fields[2]);
    } else if (fields[1]) {
      paths.add(fields[1]);
    }
  }
  for (const untracked of git(["ls-files", "--others", "--exclude-standard", "--", "skills"]).split("\n")) {
    if (untracked) paths.add(untracked);
  }
  return [...paths].sort();
}

function isPublishableRelative(relative) {
  const parts = relative.split("/");
  if (parts.some((part) => !part || part.startsWith("."))) return false;
  if (parts.some((part) => ["assets", "dist", "node_modules", "src", "target"].includes(part))) {
    return false;
  }
  const fileName = parts.at(-1) ?? "";
  if (parts.includes("fixtures")) {
    return fileName.endsWith(".mjs") || fileName.endsWith(".js");
  }
  if (parts.includes("references")) return fileName.endsWith(".md");
  if (fileName.endsWith(".mjs") || fileName.endsWith(".js")) return true;
  if (fileName.endsWith(".manifest.json")) return true;
  return ["SKILL.md", "run.yaml", "manifest.json", "run.mjs", "run.js", "harness.mjs", "harness.js"]
    .includes(fileName);
}

function skillVersion(profile) {
  const version = profile?.version;
  return isSemver(version) ? version : null;
}

function versionFinding(skill, baseVersion, currentVersion, changedPaths) {
  if (baseVersion === null || currentVersion === null) return null;
  if (compareSemver(currentVersion, baseVersion) > 0) return null;
  return [
    `skills/${skill}: publishable package material changed without a version increase`,
    `  version: ${baseVersion} -> ${currentVersion}`,
    ...changedPaths.map((changedPath) => `  changed: ${changedPath}`),
  ].join("\n");
}

function isSemver(value) {
  return typeof value === "string" && /^\d+\.\d+\.\d+$/u.test(value);
}

function compareSemver(left, right) {
  const leftParts = left.split(".").map(Number);
  const rightParts = right.split(".").map(Number);
  for (let index = 0; index < 3; index += 1) {
    const difference = leftParts[index] - rightParts[index];
    if (difference !== 0) return difference;
  }
  return 0;
}

function loadBaseProfiles(base, skills) {
  const documents = skills.flatMap((skill) => {
    try {
      return [{ skill, source: git(["show", `${base}:skills/${skill}/run.yaml`]) }];
    } catch {
      return [];
    }
  });
  const results = evaluateParserRequestResults(documents.map(({ source }) => ({
    kind: "parser.validateRunnerManifestYaml",
    yaml: source,
  })));
  return new Map(documents.flatMap((document, index) => {
    const result = results[index];
    return result?.status === "success"
      ? [[document.skill, result.value]]
      : [];
  }));
}

function resolveBase() {
  const baseArgIndex = process.argv.indexOf("--base");
  const requested = baseArgIndex >= 0 ? process.argv[baseArgIndex + 1] : undefined;
  const candidates = [requested, process.env.RUNX_SKILL_VERSION_BASE, process.env.GITHUB_BASE_SHA, "HEAD"];
  for (const candidate of candidates) {
    if (!candidate) continue;
    try {
      const commit = git(["rev-parse", "--verify", `${candidate}^{commit}`]).trim();
      return git(["merge-base", "HEAD", commit]).trim();
    } catch {
      // Try the next source. Static catalog checks still run without a base.
    }
  }
  return null;
}

function git(args) {
  return execFileSync("git", args, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
}

function runSelfTests() {
  assert(isPublishableRelative("SKILL.md"), "SKILL.md is publishable");
  assert(isPublishableRelative("references/operator.md"), "reference markdown is publishable");
  assert(!isPublishableRelative("fixtures/evidence.json"), "ordinary fixtures are not package material");
  assert(
    isPublishableRelative("fixtures/helper.mjs"),
    "a harness helper is conservatively treated as package material",
  );
  assert(versionFinding("new-skill", null, "0.1.0", ["skills/new-skill/SKILL.md"]) === null, "new skill is allowed");
  assert(versionFinding("old-skill", "0.1.0", null, ["skills/old-skill/SKILL.md"]) === null, "deleted skill is allowed");
  assert(versionFinding("same", "0.1.0", "0.1.0", ["skills/same/SKILL.md"]) !== null, "unchanged version fails");
  assert(versionFinding("bumped", "0.1.0", "0.1.1", ["skills/bumped/SKILL.md"]) === null, "increased version passes");
  console.log("skill version drift self-tests ok");
}

function assert(condition, message) {
  if (!condition) throw new Error(`self-test failed: ${message}`);
}
