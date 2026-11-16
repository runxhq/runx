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
// A skill's version is one patch above what the registry published once it
// changes, and nothing more until it's published again (Kam, 15 November).
// Only the registry knows what it published, so the comparison runs where
// that read is made, at release: `--published <registry url>`.
const publishedIndex = process.argv.indexOf("--published");
const registry = publishedIndex >= 0 ? process.argv[publishedIndex + 1] : null;
const versionFindings = registry ? await checkPublished(registry) : [];
const findings = [...staticFindings, ...versionFindings];

if (findings.length > 0) {
  for (const finding of findings) console.error(finding);
  process.exit(1);
}

const comparison = registry ? ` and each version against what ${registry} published` : "";
console.log(`skill catalog checks ok${comparison}`);

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

function skillVersion(profile) {
  const version = profile?.version;
  return isSemver(version) ? version : null;
}

/**
 * The version a skill must state against what the registry published: the
 * published version while its content is what was published, and one patch
 * above it once its content changed.
 */
function expectedVersion(published, unchanged) {
  if (unchanged) return published;
  const [major, minor, patch] = published.split(".").map(Number);
  return `${major}.${minor}.${patch + 1}`;
}

/**
 * Each skill against what `registry` publishes of it under runx: its content
 * digest as `runx registry package` computes it, beside the published one. A
 * skill the registry doesn't publish has no version to follow yet.
 */
async function checkPublished(registry) {
  const runx = process.env.RUNX_RUST_CLI_BIN;
  if (!runx) return ["--published needs RUNX_RUST_CLI_BIN, the runx that computes each package's digest"];
  const findings = [];
  for (const [skill, profile] of currentProfiles) {
    const response = await fetch(`${registry}/v1/skills/runx/${encodeURIComponent(skill)}`, {
      headers: { "user-agent": "runx-version-check" },
    });
    if (response.status === 404) continue;
    if (!response.ok) {
      findings.push(`skills/${skill}: ${registry} answered ${response.status}, so its published version is unknown`);
      continue;
    }
    const published = (await response.json()).skill;
    if (!isSemver(published?.version) || typeof published?.content_digest !== "string") {
      findings.push(`skills/${skill}: ${registry} states no version and content digest for it`);
      continue;
    }
    const packaged = JSON.parse(
      execFileSync(runx, ["registry", "package", path.join(root, "skills", skill), "--json"], { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 }),
    );
    const unchanged = packaged?.registry?.content_digest === published.content_digest;
    const expected = expectedVersion(published.version, unchanged);
    if (profile.version !== expected) {
      findings.push(
        unchanged
          ? `skills/${skill}: unchanged since ${published.version} was published, so its version stays ${expected}, not ${profile.version}`
          : `skills/${skill}: changed since ${published.version} was published, so its version is ${expected}, not ${profile.version}`,
      );
    }
  }
  return findings;
}

function isSemver(value) {
  return typeof value === "string" && /^\d+\.\d+\.\d+$/u.test(value);
}

function runSelfTests() {
  assert(expectedVersion("0.4.13", true) === "0.4.13", "an unchanged skill keeps its published version");
  assert(expectedVersion("0.4.13", false) === "0.4.14", "a changed skill is one patch above it");
  assert(expectedVersion("0.9.9", false) === "0.9.10", "a patch past nine is ten");
  assert(methodsWithoutPurpose({ methods: { only: {} } }).length === 0, "a skill's only method needs no purpose of its own");
  assert(
    methodsWithoutPurpose({ methods: { read: { description: "Read it." }, write: {}, blank: { description: " " } } })
      .join(",") === "write,blank",
    "each method of several states its purpose",
  );
  console.log("skill catalog check self-tests ok");
}

function assert(condition, message) {
  if (!condition) throw new Error(`self-test failed: ${message}`);
}
