#!/usr/bin/env node
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { evaluateParserRequestResults } from "./lib/native-parser.mjs";
import { bundledCoreSkills } from "./classify-ci-changes.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const currentProfiles = new Map();
const selectedSkills = process.argv.includes("--core-only") ? new Set(bundledCoreSkills()) : null;

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
console.log(`skill catalog version and workflow-input checks ok${comparison}`);

function checkCurrentCatalog() {
  const findings = [];
  const skillsRoot = path.join(root, "skills");
  const documents = [];
  for (const entry of readdirSync(skillsRoot, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    if (selectedSkills && !selectedSkills.has(entry.name)) continue;
    const profilePath = path.join(skillsRoot, entry.name, "run.yaml");
    const manualPath = path.join(skillsRoot, entry.name, "SKILL.md");
    if (!existsSync(profilePath) || !existsSync(manualPath)) continue;
    documents.push({
      skill: entry.name,
      manual: readFileSync(manualPath, "utf8"),
      source: readFileSync(profilePath, "utf8"),
    });
  }
  // A run.yaml is read as the package its SKILL.md names, as the runtime
  // reads it.
  const manuals = evaluateParserRequestResults(documents.map(({ manual }) => ({
    kind: "parser.validateSkillMarkdown",
    markdown: manual,
  })));
  const results = evaluateParserRequestResults(documents.map(({ skill, source }, index) => ({
    kind: "parser.validateMethodManifestYaml",
    skill: manuals[index]?.value?.name ?? skill,
    yaml: source,
  })));
  for (const [index, document] of documents.entries()) {
    if (manuals[index]?.status !== "success") {
      findings.push(`skills/${document.skill}/SKILL.md: ${manuals[index]?.error?.message ?? "invalid manual"}`);
      continue;
    }
    const result = results[index];
    if (result?.status !== "success") {
      findings.push(`skills/${document.skill}/run.yaml: ${result?.error?.message ?? "invalid method manifest"}`);
      continue;
    }
    const profile = result.value;
    currentProfiles.set(document.skill, profile);
    if (!isSemver(profile?.version)) {
      findings.push(`skills/${document.skill}/run.yaml: version must be quoted semantic version x.y.z`);
    } else if (!document.source.split("\n").some((line) => line === `version: "${profile.version}"`)) {
      findings.push(`skills/${document.skill}/run.yaml: version must be quoted as "${profile.version}"`);
    }
    const unstated = methodsWithoutPurpose(profile);
    if (unstated.length > 0) {
      findings.push(
        `skills/${document.skill}/run.yaml: a skill of several methods states each one's purpose in its description, which a model chooses it by; ${unstated.join(", ")} ${unstated.length === 1 ? "states" : "state"} none`,
      );
    }
  }
  return findings;
}

/**
 * The methods of a skill of several that state no purpose. A skill's only
 * method is described by the skill's own description.
 */
function methodsWithoutPurpose(profile) {
  const methods = Object.entries(profile?.methods ?? {});
  if (methods.length < 2) return [];
  return methods
    .filter(([, method]) => typeof method?.description !== "string" || method.description.trim() === "")
    .map(([name]) => name);
}

function checkVersionDrift(base) {
  const candidatePaths = new Map();
  for (const changedPath of changedSkillPaths(base)) {
    const parts = changedPath.split("/");
    if (parts.length < 3 || parts[0] !== "skills") continue;
    const skill = parts[1];
    const relative = parts.slice(2).join("/");
    if (!skill || !relative) continue;
    if (selectedSkills && !selectedSkills.has(skill)) continue;
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
  if (parts[0] === "packets") return fileName.endsWith(".schema.json");
  // A package's tests and evals are files of their own, and part of what it
  // publishes.
  if (parts[0] === "tests") return parts.length === 2 && fileName.endsWith(".yaml");
  if (parts[0] === "evals") return parts.length === 2 && fileName.endsWith(".json");
  if (parts.includes("fixtures")) {
    return fileName.endsWith(".mjs") || fileName.endsWith(".js");
  }
  if (parts.includes("references")) return fileName.endsWith(".md");
  if (fileName.endsWith(".mjs") || fileName.endsWith(".js")) return true;
  if (fileName.endsWith(".manifest.json")) return true;
  return ["SKILL.md", "run.yaml", "eval_queries.json", "manifest.json", "run.mjs", "run.js"]
    .includes(fileName);
}

function skillVersion(profile) {
  const version = profile?.version;
  return isSemver(version) ? version : null;
}

function versionFinding(skill, baseVersion, currentVersion, changedPaths) {
  if (baseVersion === null || currentVersion === null) return null;
  // Labels identify immutable package bytes, not publication order. An
  // authorized correction can restore a package to its intended 0.x line.
  if (currentVersion !== baseVersion) return null;
  return [
    `skills/${skill}: publishable package material changed without a version change`,
    `  version: ${baseVersion} -> ${currentVersion}`,
    ...changedPaths.map((changedPath) => `  changed: ${changedPath}`),
  ].join("\n");
}

function isSemver(value) {
  return typeof value === "string" && /^\d+\.\d+\.\d+$/u.test(value);
}

// The base version is read from the base file's own version line, never by
// validating the base with the current parser: a base the current parser no
// longer accepts must still be compared, or a syntax change would pass
// unversioned.
function loadBaseProfiles(base, skills) {
  return new Map(skills.flatMap((skill) => {
    let source;
    try {
      source = git(["show", `${base}:skills/${skill}/run.yaml`]);
    } catch {
      return [];
    }
    const version = /^version: "(\d+\.\d+\.\d+)"$/mu.exec(source)?.[1];
    return version ? [[skill, { version }]] : [];
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
  assert(isPublishableRelative("packets/plan.v1.schema.json"), "a package packet is publishable");
  assert(!isPublishableRelative("fixtures/evidence.json"), "ordinary fixtures are not package material");
  assert(
    isPublishableRelative("fixtures/helper.mjs"),
    "a test helper is conservatively treated as package material",
  );
  assert(isPublishableRelative("tests/default-reads.yaml"), "a package's test is publishable");
  assert(isPublishableRelative("evals/evals.json"), "a package's evals are publishable");
  assert(isPublishableRelative("eval_queries.json"), "a package's eval queries are publishable");
  assert(versionFinding("new-skill", null, "0.1.0", ["skills/new-skill/SKILL.md"]) === null, "new skill is allowed");
  assert(versionFinding("old-skill", "0.1.0", null, ["skills/old-skill/SKILL.md"]) === null, "deleted skill is allowed");
  assert(versionFinding("same", "0.1.0", "0.1.0", ["skills/same/SKILL.md"]) !== null, "unchanged version fails");
  assert(versionFinding("bumped", "0.1.0", "0.1.1", ["skills/bumped/SKILL.md"]) === null, "increased version passes");
  assert(versionFinding("corrected", "1.0.0", "0.7.0", ["skills/corrected/run.yaml"]) === null, "corrected prerelease label passes");
  assert(methodsWithoutPurpose({ methods: { only: {} } }).length === 0, "a skill's only method needs no purpose of its own");
  assert(
    methodsWithoutPurpose({ methods: { read: { description: "Read it." }, write: {}, blank: { description: " " } } })
      .join(",") === "write,blank",
    "each method of several states its purpose",
  );
  console.log("skill version drift self-tests ok");
}

function assert(condition, message) {
  if (!condition) throw new Error(`self-test failed: ${message}`);
}
