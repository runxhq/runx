#!/usr/bin/env node

import { access, readdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  validateRunnerManifestYamlBatch,
  validateSkillMarkdownBatch,
} from "./lib/native-parser.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const workspaceRoot = path.resolve(scriptDir, "..");
const skillsRoot = path.join(workspaceRoot, "skills");
const outputPath = path.join(skillsRoot, "runx-maintained.lock.json");
const check = process.argv.includes("--check");

const packages = [];
for (const entry of (await readdir(skillsRoot, { withFileTypes: true })).sort((left, right) => left.name.localeCompare(right.name))) {
  if (!entry.isDirectory()) continue;
  const skillDir = path.join(skillsRoot, entry.name);
  const profilePath = path.join(skillDir, "run.yaml");
  try {
    await access(path.join(skillDir, "SKILL.md"));
    await access(profilePath);
  } catch {
    continue;
  }
  packages.push({
    markdown: await readFile(path.join(skillDir, "SKILL.md"), "utf8"),
    profileDocument: await readFile(profilePath, "utf8"),
  });
}
const skills = validateSkillMarkdownBatch(packages.map((entry) => entry.markdown));
const manifests = validateRunnerManifestYamlBatch(packages.map((entry) => entry.profileDocument));
const entries = packages.map(({ markdown, profileDocument }, index) => {
  const record = buildRunxMaintainedSkillLockRecord(markdown, profileDocument, skills[index], manifests[index]);
  return {
    skill_id: record.skill_id,
    version: record.version,
    digest: record.digest,
    catalog_visibility: record.catalog_visibility,
    catalog_role: record.catalog_role,
  };
});

await writeOrCheck(outputPath, `${JSON.stringify(entries, null, 2)}\n`);
console.log(`${check ? "checked" : "generated"} ${entries.length} runx-maintained skill locks`);

function buildRunxMaintainedSkillLockRecord(markdown, profileDocument, skill, manifest) {
  if (manifest.skill && manifest.skill !== skill.name) {
    throw new Error(`Runner manifest skill '${manifest.skill}' does not match skill '${skill.name}'.`);
  }

  const digest = createHash("sha256").update(markdown).digest("hex");
  const profileDigest = createHash("sha256").update(profileDocument).digest("hex");
  const versionSeed = createHash("sha256")
    .update(JSON.stringify({
      markdown_digest: digest,
      profile_digest: profileDigest,
    }))
    .digest("hex");
  return {
    skill_id: `${manifest.catalog?.owner ?? "runx"}/${slugifyRunxMaintainedSkillName(skill.name)}`,
    version: `sha-${versionSeed.slice(0, 12)}`,
    digest,
    catalog_visibility: manifest.catalog?.visibility ?? "internal",
    catalog_role: manifest.catalog?.role,
  };
}

function slugifyRunxMaintainedSkillName(value) {
  const slug = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  if (!slug) {
    throw new Error("Runx-maintained skill names cannot produce an empty registry slug.");
  }
  return slug;
}

async function writeOrCheck(filePath, contents) {
  if (check) {
    const current = await readFile(filePath, "utf8").catch(() => undefined);
    if (current !== contents) {
      throw new Error(`${path.relative(workspaceRoot, filePath)} is stale; run pnpm runx-maintained-lock:generate`);
    }
    return;
  }
  await writeFile(filePath, contents, "utf8");
}
