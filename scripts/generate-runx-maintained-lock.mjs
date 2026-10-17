#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { access, readdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  resolveNativeRunxBinary,
  validateRunnerManifestYamlBatch,
  validateSkillMarkdownBatch,
} from "./lib/native-parser.mjs";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const workspaceRoot = path.resolve(scriptDir, "..");
const skillsRoot = path.join(workspaceRoot, "skills");
const outputPath = path.join(skillsRoot, "runx-maintained.lock.json");
const check = process.argv.includes("--check");
const previous = new Map(JSON.parse(await readFile(outputPath, "utf8"))
  .map((entry) => [entry.skill_id, entry]));

const packages = [];
const runx = resolveNativeRunxBinary();
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
  const result = spawnSync(runx, ["registry", "package", skillDir, "--json"], {
    cwd: workspaceRoot, encoding: "utf8", maxBuffer: 64 * 1024 * 1024,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`Cannot prepare ${entry.name}: ${result.stderr || result.stdout}`);
  const response = JSON.parse(result.stdout);
  const prepared = response.registry?.package;
  const contentDigest = response.registry?.content_digest;
  if (response.status !== "success" || !prepared || !/^sha256:[0-9a-f]{64}$/u.test(contentDigest ?? "")) {
    throw new Error(`Native package preparation omitted the complete content identity for ${entry.name}.`);
  }
  packages.push({ markdown: prepared.markdown, profileDocument: prepared.profile_document, contentDigest });
}
const skills = validateSkillMarkdownBatch(packages.map((entry) => entry.markdown));
const manifests = validateRunnerManifestYamlBatch(packages.map((entry) => entry.profileDocument));
const entries = packages.map(({ markdown, profileDocument, contentDigest }, index) => {
  const record = buildRunxMaintainedSkillLockRecord(markdown, profileDocument, skills[index], manifests[index], contentDigest);
  return {
    skill_id: record.skill_id,
    version: record.version,
    digest: record.digest,
    content_digest: record.content_digest,
    catalog_visibility: record.catalog_visibility,
    catalog_role: record.catalog_role,
  };
});

await writeOrCheck(outputPath, `${JSON.stringify(entries, null, 2)}\n`);
console.log(`${check ? "checked" : "generated"} ${entries.length} runx-maintained skill locks`);

function buildRunxMaintainedSkillLockRecord(markdown, profileDocument, skill, manifest, contentDigest) {
  if (manifest.skill && manifest.skill !== skill.name) {
    throw new Error(`Runner manifest skill '${manifest.skill}' does not match skill '${skill.name}'.`);
  }

  const digest = createHash("sha256").update(markdown).digest("hex");
  const skillId = `${manifest.catalog?.owner ?? "runx"}/${slugifyRunxMaintainedSkillName(skill.name)}`;
  const locked = previous.get(skillId);
  // An immutable published label stays pinned while the complete package is
  // unchanged. Only new identities or changed bytes need a new digest label.
  const version = locked?.content_digest === contentDigest
    ? locked.version : `sha-${contentDigest.slice(7, 19)}`;
  return {
    skill_id: skillId,
    version,
    digest,
    content_digest: contentDigest,
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
