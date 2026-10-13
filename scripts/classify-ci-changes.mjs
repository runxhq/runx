#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

// Changes no build or runtime proof reads: checked by the light lane alone.
const lightOnlyPatterns = [
  /^\.github\/dependabot\.yml$/u,
  /^\.scafld\/(?:runs|specs)\//u,
  /^(?:AGENTS|CHANGELOG|CLAUDE|CODE_OF_CONDUCT|CONTRIBUTING|CONVENTIONS|NOTICE|README|SECURITY)\.md$/u,
  /^LICENSE(?:\.|$)/u,
  /^docs\//u,
  /^llms\.txt$/u,
  /^plans\//u,
  // A vendor skill is product content the registry proves when it is
  // published; CI checks only that the lock still describes it.
  /^skills\/runx-maintained\.lock\.json$/u,
];

// Release tooling runs only in the release workflow: a change to it alone is
// checked by its own syntax and the version and notes checks, not the runtime
// corpus. The candidate smoke stays in the full lane, since verify:fast runs it.
const releasePatterns = [
  /^\.github\/workflows\/(?:release|publish-typescript-packages)\.yml$/u,
  /^packaging\//u,
  /^release\//u,
  /^scripts\/(?:build-channel-input(?:\.test)?|build-release-archives|check-publishable-package-manifests(?:\.test)?|check-rust-cli-release-artifacts|check-runx-cli-release-notes|global-npm-prefix|pack-typescript-packages|publish-npm-tarball|publish-winget-manifest|release-platform-matrix|release-rust-cli|runx-cli-release-profile|set-release-version|smoke-released-cli-live|smoke-runx-image)\.(?:mjs|sh|ts)$/u,
];

const windowsPatterns = [
  /^\.github\/workflows\/ci\.yml$/u,
  /^crates\/Cargo\.lock$/u,
  /^rust-toolchain\.toml$/u,
  /^crates\/runx-runtime\/Cargo\.toml$/u,
  /^crates\/runx-runtime\/src\/(?:adapters\/javascript\/supervisor\/(?:process|session)|path_util|private_file|process|receipts\/(?:local_key|store))\.rs$/u,
  /^crates\/runx-runtime\/src\/process\//u,
  /^scripts\/harness-sweep\.mjs$/u,
];

/** The core skills the CLI ships, bundled into its binary. */
export function bundledCoreSkills() {
  const bundle = JSON.parse(
    readFileSync(path.join(repoRoot, "crates", "runx-runtime", "bundled-skills.json"), "utf8"),
  );
  const entries = Array.isArray(bundle.skills) ? bundle.skills : Object.values(bundle.skills ?? bundle);
  return entries.map((entry) => String(entry.skill_id ?? entry.name).split("/").pop());
}

export function classifyFiles(files, coreSkills = bundledCoreSkills()) {
  const normalized = [...new Set(files.map((file) => file.trim()).filter(Boolean))].sort();
  const core = new Set(coreSkills);
  const skillOf = (file) => /^skills\/([^/]+)\//u.exec(file)?.[1];
  const vendorSkill = (file) => skillOf(file) !== undefined && !core.has(skillOf(file));
  const light = (file) => matches(lightOnlyPatterns, file) || vendorSkill(file);
  const changedCore = [...new Set(normalized.map(skillOf).filter((name) => name && core.has(name)))];
  const full = normalized.length === 0 || normalized.some((file) =>
    !light(file) && !matches(releasePatterns, file) && skillOf(file) === undefined
  );
  // The core skills ship in the binary: a runtime change proves all of them,
  // a core skill change proves that skill and its dependants.
  const coreSkillRuns = full ? [...core].sort() : changedCore.sort();
  return {
    files: normalized,
    full,
    skills: coreSkillRuns.length > 0,
    core_skills: coreSkillRuns,
    windows: normalized.length === 0 || normalized.some((file) => matches(windowsPatterns, file)),
    release: normalized.length === 0 || normalized.some((file) => matches(releasePatterns, file)),
    light: !full,
  };
}

function main() {
  const [baseRef, headRef = "HEAD"] = process.argv.slice(2);
  if (!baseRef) fail("usage: node scripts/classify-ci-changes.mjs <base-ref> [head-ref]");
  const classification = classifyFiles(changedFiles(baseRef, headRef));
  for (const key of ["full", "skills", "windows", "release", "light"]) {
    writeOutput(key, String(classification[key]));
  }
  writeOutput("core_skills", classification.core_skills.join(","));
  writeOutput("changed_files", classification.files.join(" "));
  writeOutput("summary", summarize(classification));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();

function matches(patterns, file) {
  return patterns.some((pattern) => pattern.test(file));
}

function changedFiles(base, head) {
  try {
    return execFileSync("git", ["diff", "--name-only", `${base}...${head}`], { encoding: "utf8" })
      .split(/\r?\n/u);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.warn(`warning: could not diff ${base}...${head}; running all checks. ${message.split("\n")[0]}`);
    return [];
  }
}

function summarize(input) {
  const lanes = [input.full ? "full" : undefined, input.skills ? `core skills (${input.core_skills.length})` : undefined,
    input.windows ? "windows" : undefined, input.release ? "release" : undefined,
    input.light ? "light" : undefined].filter(Boolean);
  return `${lanes.join("+")} checks for ${input.files.length} changed file(s)`;
}

function writeOutput(name, value) {
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `${name}=${value.replace(/\n/gu, " ")}\n`);
  else console.log(`${name}=${value}`);
}

function fail(message) {
  console.error(message);
  process.exit(2);
}
