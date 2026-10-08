#!/usr/bin/env node
import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";

const workspaceRoot = process.cwd();
const bindingRoot = path.join(workspaceRoot, "bindings");
const allowedBindingFiles = new Set(["binding.json", "run.yaml"]);
const validStates = new Set([
  "draft",
  "harness_verified",
  "published",
  "retired",
]);

const findings = [];
const bindings = collectBindings(bindingRoot);

if (bindings.length === 0) {
  findings.push("bindings/ must contain at least one upstream binding or be removed entirely.");
}

for (const bindingPath of bindings) {
  validateBinding(bindingPath);
}

if (findings.length > 0) {
  for (const finding of findings) {
    console.error(`binding check: ${finding}`);
  }
  process.exit(1);
}

console.log(`upstream binding check ok (${bindings.length} binding${bindings.length === 1 ? "" : "s"})`);

function collectBindings(root) {
  const result = [];
  for (const owner of readDirectoryNames(root)) {
    const ownerDir = path.join(root, owner);
    for (const skill of readDirectoryNames(ownerDir)) {
      result.push(path.join(ownerDir, skill, "binding.json"));
    }
  }
  return result.sort();
}

function readDirectoryNames(dir) {
  return readdirSync(dir)
    .filter((entry) => statSync(path.join(dir, entry)).isDirectory())
    .sort();
}

function validateBinding(bindingPath) {
  const bindingDir = path.dirname(bindingPath);
  const relativeDir = path.relative(workspaceRoot, bindingDir);
  const [root, owner, skillName] = relativeDir.split(path.sep);
  if (root !== "bindings" || !owner || !skillName) {
    findings.push(`${relativeDir}: expected bindings/<owner>/<skill>/binding.json`);
    return;
  }

  for (const entry of readdirSync(bindingDir).sort()) {
    if (!allowedBindingFiles.has(entry)) {
      findings.push(`${relativeDir}: unexpected file ${entry}; bindings contain only binding.json and run.yaml`);
    }
  }

  const binding = readJson(bindingPath);
  const profilePath = path.join(bindingDir, "run.yaml");
  const profile = readText(profilePath);
  const expectedId = `${owner}/${skillName}`;
  const expectedProfilePath = `bindings/${owner}/${skillName}/run.yaml`;

  requireEqual(binding.schema, "runx.registry_binding.v1", `${relativeDir}: schema`);
  requireSetValue(binding.state, validStates, `${relativeDir}: state`);
  requireEqual(binding.skill?.id, expectedId, `${relativeDir}: skill.id`);
  requireEqual(binding.skill?.name, skillName, `${relativeDir}: skill.name`);
  requireString(binding.skill?.description, `${relativeDir}: skill.description`);

  requireEqual(binding.upstream?.host, "github.com", `${relativeDir}: upstream.host`);
  requireString(binding.upstream?.owner, `${relativeDir}: upstream.owner`);
  requireString(binding.upstream?.repo, `${relativeDir}: upstream.repo`);
  requireEqual(binding.upstream?.path, "SKILL.md", `${relativeDir}: upstream.path`);
  requireHex(binding.upstream?.commit, 40, `${relativeDir}: upstream.commit`);
  requireHex(binding.upstream?.blob_sha, 40, `${relativeDir}: upstream.blob_sha`);
  requireEqual(binding.upstream?.source_of_truth, true, `${relativeDir}: upstream.source_of_truth`);
  requirePinnedUrl(binding.upstream?.html_url, binding.upstream, `${relativeDir}: upstream.html_url`);
  requirePinnedUrl(binding.upstream?.raw_url, binding.upstream, `${relativeDir}: upstream.raw_url`);

  requireEqual(binding.registry?.owner, owner, `${relativeDir}: registry.owner`);
  requireString(binding.registry?.trust_tier, `${relativeDir}: registry.trust_tier`);
  requireString(binding.registry?.version, `${relativeDir}: registry.version`);
  requireEqual(binding.registry?.profile_path, expectedProfilePath, `${relativeDir}: registry.profile_path`);
  requireEqual(
    binding.registry?.materialized_package_is_registry_artifact,
    true,
    `${relativeDir}: registry.materialized_package_is_registry_artifact`,
  );

  requireString(binding.harness?.status, `${relativeDir}: harness.status`);
  requirePositiveInteger(binding.harness?.case_count, `${relativeDir}: harness.case_count`);
  requirePositiveInteger(binding.harness?.assertion_count, `${relativeDir}: harness.assertion_count`);
  if (!Array.isArray(binding.harness?.case_names) || binding.harness.case_names.length === 0) {
    findings.push(`${relativeDir}: harness.case_names must list at least one case`);
  }

  requireString(binding.publication?.status, `${relativeDir}: publication.status`);
  if (!Array.isArray(binding.tags) || binding.tags.length === 0) {
    findings.push(`${relativeDir}: tags must list at least one catalog tag`);
  }

  const profileSkill = profile.match(/^skill:\s*([A-Za-z0-9_.-]+)\s*$/m)?.[1];
  requireEqual(profileSkill, skillName, `${relativeDir}: run.yaml skill`);
  if (!/^runners:\s*$/m.test(profile)) {
    findings.push(`${relativeDir}: run.yaml must declare runners`);
  }
}

function readJson(file) {
  try {
    return JSON.parse(readText(file));
  } catch (error) {
    findings.push(`${path.relative(workspaceRoot, file)}: invalid JSON (${error.message})`);
    return {};
  }
}

function readText(file) {
  try {
    return readFileSync(file, "utf8");
  } catch (error) {
    findings.push(`${path.relative(workspaceRoot, file)}: cannot read file (${error.message})`);
    return "";
  }
}

function requireEqual(actual, expected, label) {
  if (actual !== expected) {
    findings.push(`${label} must be ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

function requireSetValue(actual, allowed, label) {
  if (!allowed.has(actual)) {
    findings.push(`${label} must be one of ${Array.from(allowed).join(", ")}, got ${JSON.stringify(actual)}`);
  }
}

function requireString(value, label) {
  if (typeof value !== "string" || value.trim().length === 0) {
    findings.push(`${label} must be a non-empty string`);
  }
}

function requirePositiveInteger(value, label) {
  if (!Number.isInteger(value) || value <= 0) {
    findings.push(`${label} must be a positive integer`);
  }
}

function requireHex(value, length, label) {
  if (typeof value !== "string" || !new RegExp(`^[a-f0-9]{${length}}$`, "i").test(value)) {
    findings.push(`${label} must be a ${length}-character hex digest`);
  }
}

function requirePinnedUrl(value, upstream, label) {
  requireString(value, label);
  if (typeof value !== "string") return;
  const expectedParts = [
    upstream?.owner,
    upstream?.repo,
    upstream?.commit,
    upstream?.path,
  ].filter(Boolean);
  for (const part of expectedParts) {
    if (!value.includes(part)) {
      findings.push(`${label} must include pinned upstream component ${part}`);
    }
  }
}
