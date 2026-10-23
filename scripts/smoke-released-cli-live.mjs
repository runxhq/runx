#!/usr/bin/env node

import { execFile } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const npm = process.platform === "win32" ? "npm.cmd" : "npm";
const cliVersion = process.env.RUNX_SMOKE_RELEASE_VERSION?.trim() || await readCliVersion();
const registryBaseUrl = normalizeBaseUrl(
  process.env.RUNX_SMOKE_REGISTRY_BASE_URL,
  "https://api.runx.ai",
);
const skillId = process.env.RUNX_SMOKE_SKILL_ID?.trim() || "runx/github-issue-triage";
const smokeInstallationId = process.env.RUNX_SMOKE_INSTALLATION_ID?.trim() || "inst_release_smoke";

const [publisher, name] = parseSkillId(skillId);
const tempDir = await mkdtemp(path.join(os.tmpdir(), "runx-release-smoke-"));
const homeDir = path.join(tempDir, "home");
const skillsDir = path.join(tempDir, "skills");
const installStatePath = path.join(homeDir, "install.json");
const cliBin = path.join(tempDir, "node_modules", ".bin", process.platform === "win32" ? "runx.cmd" : "runx");

try {
  const skillDetail = await readJson(`${registryBaseUrl}/v1/skills/${encodeURIComponent(publisher)}/${encodeURIComponent(name)}`);
  const skillVersion = skillDetail?.skill?.version;
  const beforeCount = Number(skillDetail?.skill?.install_count ?? 0);
  if (typeof skillVersion !== "string" || skillVersion.length === 0) {
    throw new Error(`Unable to resolve live version for ${skillId}.`);
  }

  await execFileAsync(npm, ["init", "-y"], { cwd: tempDir, timeout: 60_000, maxBuffer: 8 * 1024 * 1024 });
  await execFileAsync(npm, ["install", "--silent", `${"@runxhq/cli"}@${cliVersion}`], {
    cwd: tempDir,
    timeout: 120_000,
    maxBuffer: 8 * 1024 * 1024,
  });

  await mkdir(homeDir, { recursive: true });
  await writeFile(
    installStatePath,
    `${JSON.stringify({
      version: 1,
      installation_id: smokeInstallationId,
      created_at: "2026-04-15T00:00:00.000Z",
    }, null, 2)}\n`,
    { mode: 0o600 },
  );

  const search = await runRunx(
    cliBin,
    ["registry", "search", name, "--registry", registryBaseUrl, "--json"],
    tempDir,
    homeDir,
  );
  const parsedSearch = JSON.parse(search.stdout);
  const searchSource = parsedSearch.registry?.source;
  const runxSearch = parsedSearch.registry?.results?.find((entry) => entry?.skill_id === skillId);
  if (!runxSearch || searchSource !== "remote") {
    throw new Error(`Released CLI did not return ${skillId} as a remote registry result.`);
  }

  const firstInstall = await runRunx(
    cliBin,
    ["add", `${skillId}@${skillVersion}`, "--registry", registryBaseUrl, "--to", skillsDir, "--json"],
    tempDir,
    homeDir,
  );
  const firstParsed = JSON.parse(firstInstall.stdout);
  const firstRegistry = firstParsed.registry;
  const firstInstallPayload = firstRegistry?.install;
  if (
    firstRegistry?.source !== "remote"
    || firstInstallPayload?.skill_id !== skillId
    || firstInstallPayload?.source !== "runx-registry"
  ) {
    throw new Error(`First released CLI install returned an unexpected payload for ${skillId}.`);
  }

  const midCount = Number((await readJson(`${registryBaseUrl}/v1/skills/${encodeURIComponent(publisher)}/${encodeURIComponent(name)}`))?.skill?.install_count ?? 0);

  const secondInstall = await runRunx(
    cliBin,
    ["add", `${skillId}@${skillVersion}`, "--registry", registryBaseUrl, "--to", skillsDir, "--json"],
    tempDir,
    homeDir,
  );
  const secondParsed = JSON.parse(secondInstall.stdout);
  const secondInstallPayload = secondParsed.registry?.install;
  if (!["installed", "unchanged"].includes(String(secondInstallPayload?.status))) {
    throw new Error(`Second released CLI install returned an unexpected status for ${skillId}.`);
  }

  const afterCount = Number((await readJson(`${registryBaseUrl}/v1/skills/${encodeURIComponent(publisher)}/${encodeURIComponent(name)}`))?.skill?.install_count ?? 0);
  const firstDelta = midCount - beforeCount;
  const secondDelta = afterCount - midCount;

  if (![0, 1].includes(firstDelta)) {
    throw new Error(`Expected first acquisition delta to be 0 or 1, received ${firstDelta}.`);
  }
  if (secondDelta !== 0) {
    throw new Error(`Expected repeated acquisition delta to be 0, received ${secondDelta}.`);
  }

  process.stdout.write(`${JSON.stringify({
    status: "success",
    cli_version: cliVersion,
    skill_id: skillId,
    live_version: skillVersion,
    install_count: {
      before: beforeCount,
      after_first: midCount,
      after_second: afterCount,
    },
    search: runxSearch,
    first_install: firstInstallPayload,
    second_install: secondInstallPayload,
  }, null, 2)}\n`);
} finally {
  await rm(tempDir, { recursive: true, force: true });
}

async function runRunx(cliPath, args, cwd, homeDir) {
  return await execFileAsync(cliPath, args, {
    cwd,
    env: {
      ...process.env,
      RUNX_CWD: cwd,
      RUNX_HOME: homeDir,
    },
    timeout: 120_000,
    maxBuffer: 8 * 1024 * 1024,
  });
}

async function readCliVersion() {
  const packageJson = JSON.parse(await readFile(path.resolve("packages/cli/package.json"), "utf8"));
  if (typeof packageJson.version !== "string" || packageJson.version.length === 0) {
    throw new Error("oss/packages/cli/package.json is missing a publishable version.");
  }
  return packageJson.version;
}

async function readJson(url) {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Request failed for ${url}: HTTP ${response.status}`);
  }
  return await response.json();
}

function normalizeBaseUrl(raw, fallback) {
  return (raw?.trim() || fallback).replace(/\/$/, "");
}

function parseSkillId(raw) {
  const [publisher, name] = raw.split("/", 2).map((value) => value?.trim() || "");
  if (!publisher || !name) {
    throw new Error("RUNX_SMOKE_SKILL_ID must use publisher/name form.");
  }
  return [publisher, name];
}
