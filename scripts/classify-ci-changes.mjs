#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const lightOnlyPatterns = [
  /^\.github\/dependabot\.yml$/u,
  /^\.scafld\/(?:runs|specs)\//u,
  /^(?:AGENTS|CHANGELOG|CLAUDE|CODE_OF_CONDUCT|CONTRIBUTING|CONVENTIONS|NOTICE|README|SECURITY)\.md$/u,
  /^LICENSE(?:\.|$)/u,
  /^docs\//u,
  /^llms\.txt$/u,
  /^plans\//u,
];

const skillPatterns = [
  /^skills\//u,
  /^skills\/runx-maintained\.lock\.json$/u,
  /^scripts\/(?:audit-core-skills|check-codex-export|check-skill-version-drift|generate-runx-maintained-lock|harness-sweep)\.mjs$/u,
];

const skillImpactPatterns = [
  /^\.github\/workflows\/ci\.yml$/u,
  /^crates\//u,
  /^fixtures\/(?:harness|kernel|skills)\//u,
  /^package\.json$/u,
  /^pnpm-lock\.yaml$/u,
  /^rust-toolchain\.toml$/u,
  /^schemas\//u,
];

const windowsPatterns = [
  /^\.github\/workflows\/ci\.yml$/u,
  /^crates\/Cargo\.lock$/u,
  /^rust-toolchain\.toml$/u,
  /^crates\/runx-runtime\/Cargo\.toml$/u,
  /^crates\/runx-runtime\/src\/(?:adapters\/javascript\/supervisor\/(?:process|session)|path_util|process|receipts\/store)\.rs$/u,
  /^crates\/runx-runtime\/src\/process\//u,
  /^scripts\/harness-sweep\.mjs$/u,
];

const knownPatterns = [
  ...lightOnlyPatterns,
  ...skillPatterns,
  ...skillImpactPatterns,
  /^\.github\//u,
  /^(?:bindings|dist|examples|fixtures|packages|packaging|release|schemas|scripts|tests|tools)\//u,
  /^crates\//u,
  /^(?:\.gitattributes|\.gitignore)$/u,
  /^SKILL\.md$/u,
  /^(?:pnpm-workspace|tsconfig\.[^.]+|vitest(?:\.[^.]+)*)\.(?:json|ts|yaml)$/u,
];

export function classifyFiles(files) {
  const normalized = [...new Set(files.map((file) => file.trim()).filter(Boolean))].sort();
  const skillsChanged = normalized.some((file) => matches(skillPatterns, file));
  const skillRuntimeChanged = normalized.some((file) => matches(skillImpactPatterns, file));
  const unknown = normalized.some((file) => !matches(knownPatterns, file));
  const full = normalized.length === 0 || normalized.some((file) =>
    !matches(lightOnlyPatterns, file) && !matches(skillPatterns, file)
  );
  return {
    files: normalized,
    full,
    // Skill packages and the native runtime can alter runx-maintained skill behavior.
    // Other known workspace changes stay behind the full correctness wall
    // without paying for an unrelated catalog-wide harness sweep.
    skills: normalized.length === 0 || skillsChanged || skillRuntimeChanged || unknown,
    windows: normalized.length === 0 || normalized.some((file) => matches(windowsPatterns, file)),
    light: !full && !skillsChanged,
  };
}

function main() {
  const [baseRef, headRef = "HEAD"] = process.argv.slice(2);
  if (!baseRef) fail("usage: node scripts/classify-ci-changes.mjs <base-ref> [head-ref]");
  const classification = classifyFiles(changedFiles(baseRef, headRef));
  for (const key of ["full", "skills", "windows", "light"]) {
    writeOutput(key, String(classification[key]));
  }
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
  const lanes = [input.full ? "full" : undefined, input.skills ? "skills" : undefined,
    input.windows ? "windows" : undefined, input.light ? "light" : undefined].filter(Boolean);
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
