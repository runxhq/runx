import { existsSync, readdirSync, statSync } from "node:fs";
import { mkdir, mkdtemp, rm, stat } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import os from "node:os";
import path from "node:path";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { startLoopbackRegistry } from "../../scripts/lib/loopback-registry.mjs";

/** A hosted registry on loopback: its URL, the environment that reaches it, and a publisher's. */
type LoopbackRegistry = Awaited<ReturnType<typeof startLoopbackRegistry>>;

/** One bounded source packet, as the research skill's default method takes it. */
const SOURCE_PACKETS = JSON.stringify([{
  final_url: "https://example.com/api",
  status: 200,
  content_digest: `sha256:${"a".repeat(64)}`,
  extracted: "The API exposes a stable bounded operation contract.",
  provenance: { fetched_at: "2026-07-18T00:00:00Z", bytes: 52, redirects: [] },
}]);

describe("registry skill native fetch", () => {
  // One registry holds both packages for every case, published once from a
  // project and home of the publisher's; each case installs into its own.
  let registry: LoopbackRegistry;
  let publisherDir: string;
  let researchRef: string;
  let githubRef: string;

  beforeAll(async () => {
    registry = await startLoopbackRegistry({ publisher: "acme" });
    publisherDir = await mkdtemp(path.join(os.tmpdir(), "runx-maintained-fetch-publisher-"));
    const publisherEnv = testEnv(path.join(publisherDir, "project"), path.join(publisherDir, "home"), registry);
    await mkdir(path.join(publisherDir, "project"), { recursive: true });
    researchRef = publishRegistrySkill(registry, path.resolve("skills/research"), publisherEnv);
    githubRef = publishRegistrySkill(registry, path.resolve("skills/github"), publisherEnv);
  });

  afterAll(async () => {
    await registry.stop();
    await rm(publisherDir, { recursive: true, force: true });
  });

  it("acquires, caches, and reruns a registry skill through the native resolver", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "runx-maintained-fetch-"));
    const projectDir = path.join(tempDir, "project");
    const globalHomeDir = path.join(tempDir, "home");
    const env = testEnv(projectDir, globalHomeDir, registry);

    try {
      await mkdir(projectDir, { recursive: true });
      const first = runNativeSkill(env, [
        researchRef,
        "--registry",
        registry.url,
        "--input",
        "objective=Decide whether the supplied evidence supports an integration.",
        "--input",
        `source_packets=${SOURCE_PACKETS}`,
        "--json",
      ]);
      const firstJson = parseJsonOutput(first, 2);
      expect((firstJson as { status?: string }).status).toBe("needs_agent");
      const firstPath = findAddedSkillPath(path.join(projectDir, ".runx", "added-skills"), "acme/research");
      expect((await stat(path.join(firstPath, "SKILL.md"))).isFile()).toBe(true);
      expect((await stat(path.join(firstPath, "run.yaml"))).isFile()).toBe(true);

      const second = runNativeSkill(env, [
        researchRef,
        "--registry",
        registry.url,
        "--input",
        "objective=Decide whether the supplied evidence supports an integration.",
        "--input",
        `source_packets=${SOURCE_PACKETS}`,
        "--json",
      ]);
      const secondJson = parseJsonOutput(second, 2);
      expect((secondJson as { status?: string }).status).toBe("needs_agent");
      expect((await stat(path.join(firstPath, "SKILL.md"))).isFile()).toBe(true);
    } finally {
      await rm(tempDir, { recursive: true, force: true });
    }
  });

  it("rejects a registry acquisition with a digest mismatch before caching", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "runx-maintained-fetch-digest-"));
    const projectDir = path.join(tempDir, "project");
    const globalHomeDir = path.join(tempDir, "home");
    const env = testEnv(projectDir, globalHomeDir, registry);

    try {
      await mkdir(projectDir, { recursive: true });
      const result = runNativeSkill(env, [
        researchRef,
        "--registry",
        registry.url,
        "--digest",
        `sha256:${"0".repeat(64)}`,
        "--input",
        "objective=Decide whether the supplied evidence supports an integration.",
        "--input",
        `source_packets=${SOURCE_PACKETS}`,
        "--json",
      ]);
      expect(result.status).toBe(1);
      expect(result.stderr).toBe("");
      expect(
        (JSON.parse(result.stdout) as { error?: { message?: string } }).error?.message,
      ).toContain("digest mismatch");
      expect(
        existsSync(path.join(projectDir, "skills", "acme", "research")),
      ).toBe(false);
    } finally {
      await rm(tempDir, { recursive: true, force: true });
    }
  });

  it("copies workflow stages beside cached registry workflow skills", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "runx-maintained-fetch-stages-"));
    const projectDir = path.join(tempDir, "project");
    const globalHomeDir = path.join(tempDir, "home");
    const env = testEnv(projectDir, globalHomeDir, registry);

    try {
      await mkdir(projectDir, { recursive: true });
      const result = runNativeSkill(env, [
        "inspect",
        researchRef,
        "--registry",
        registry.url,
        "--json",
      ]);
      const output = parseJsonOutput(result, 0);
      expect((output as { status?: string }).status).toBe("ok");
      // Inspecting is not using: the package is read into the home's inspection cache (N6).
      const skillPath = findAddedSkillPath(path.join(globalHomeDir, "inspected"), "acme/research");
      expect((await stat(path.join(skillPath, "graph", "analyze", "run.yaml"))).isFile()).toBe(true);
    } finally {
      await rm(tempDir, { recursive: true, force: true });
    }
  });

  it("runs cached registry workflow skills through native data operations", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "runx-maintained-fetch-store-"));
    const projectDir = path.join(tempDir, "project");
    const globalHomeDir = path.join(tempDir, "home");
    const env = testEnv(projectDir, globalHomeDir, registry);

    try {
      await mkdir(projectDir, { recursive: true });
      // The cursor method plans locally and persists through the store's own
      // operations; it calls no GitHub operation.
      const result = runNativeSkill(env, [
        githubRef,
        "plan_and_append_cursor",
        "--registry",
        registry.url,
        "--input",
        "data_source_uri=local://runx-github-sync/maintained-cache",
        "--input",
        "resource=github_sync_events",
        "--input",
        "aggregate_id=acme/app:triage-open",
        "--input",
        "expected_version=0",
        "--input",
        "idempotency_key=acme/app:triage-open:pull:v1",
        "--input",
        "repo=acme/app",
        "--input",
        "direction=pull",
        "--input",
        "resources={\"kind\":\"issues\",\"filters\":{\"state\":\"open\",\"labels\":[\"triage\"],\"limit\":25}}",
        "--input",
        "scope=read",
        "--json",
      ]);
      const output = parseJsonOutput(result, 0) as { status?: string };

      expect(output.status).toBe("sealed");
    } finally {
      await rm(tempDir, { recursive: true, force: true });
    }
  });
});

function testEnv(projectDir: string, globalHomeDir: string, registry: LoopbackRegistry): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    RUNX_CWD: projectDir,
    RUNX_HOME: globalHomeDir,
    RUNX_RUST_CLI_BIN: nativeRunxBinaryForTest(),
    RUNX_DEVELOPMENT_AUTO_APPROVE: "true",
    RUNX_TOOL_ROOTS: path.resolve("tools"),
  };
  delete env.RUNX_RECEIPT_SIGN_KID;
  delete env.RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64;
  delete env.RUNX_RECEIPT_SIGN_ISSUER_TYPE;
  delete env.RUNX_REGISTRY_URL;
  delete env.RUNX_PUBLIC_API_TOKEN;
  return { ...env, ...registry.env };
}

function runNativeSkill(env: NodeJS.ProcessEnv, args: readonly string[]): {
  readonly status: number | null;
  readonly stdout: string;
  readonly stderr: string;
} {
  const result = spawnSync(env.RUNX_RUST_CLI_BIN ?? "runx", ["skill", ...args], {
    cwd: env.RUNX_CWD ?? process.cwd(),
    env,
    encoding: "utf8",
  });
  return {
    status: result.status,
    stdout: result.stdout,
    stderr: result.stderr,
  };
}

function parseJsonOutput(result: {
  readonly status: number | null;
  readonly stdout: string;
  readonly stderr: string;
}, expectedStatus: number): unknown {
  expect(result.status, `stderr=${result.stderr}\nstdout=${result.stdout}`).toBe(expectedStatus);
  if (result.stderr) {
    expect(result.stderr).toContain("Prepared run");
  }
  return JSON.parse(result.stdout);
}

function findAddedSkillPath(skillsDir: string, skillId: string): string {
  const [owner, name] = skillId.split("/");
  if (!owner || !name) {
    throw new Error(`Invalid skill id ${skillId}.`);
  }
  const base = skillsDir;
  const matches: string[] = [];
  const walk = (directory: string): void => {
    if (!existsSync(directory)) {
      return;
    }
    for (const entry of readdirSync(directory)) {
      const entryPath = path.join(directory, entry);
      const stats = statSync(entryPath);
      if (!stats.isDirectory()) {
        continue;
      }
      if (
        existsSync(path.join(entryPath, "SKILL.md")) &&
        existsSync(path.join(entryPath, "run.yaml")) &&
        entryPath.includes(`${path.sep}${owner}${path.sep}${name}${path.sep}`)
      ) {
        // A package root; its graph stages below are part of it.
        matches.push(entryPath);
        continue;
      }
      walk(entryPath);
    }
  };
  walk(base);
  if (matches.length !== 1) {
    throw new Error(`expected one cached registry package for ${skillId}, found ${matches.length}`);
  }
  return matches[0];
}

/**
 * Publish the package at `skillDir` to `registry` as acme, as a publisher
 * does: the registry signs it, at the version its run.yaml states. Returns
 * its `acme/<name>@<version>`.
 */
function publishRegistrySkill(registry: LoopbackRegistry, skillDir: string, env: NodeJS.ProcessEnv): string {
  const result = spawnSync(
    env.RUNX_RUST_CLI_BIN ?? "runx",
    ["registry", "publish", skillDir, "--registry", registry.url, "--publisher", "acme", "--json"],
    { cwd: process.cwd(), env: { ...env, ...registry.publisherEnv }, encoding: "utf8" },
  );
  if (result.status !== 0) {
    throw new Error(`failed to publish ${skillDir}: ${result.stderr || result.stdout}`);
  }
  const published = (JSON.parse(result.stdout) as { registry?: { publish?: { skill_id?: string; version?: string } } })
    .registry?.publish;
  if (!published?.skill_id || !published.version) {
    throw new Error(`the publish of ${skillDir} named no version: ${result.stdout}`);
  }
  return `${published.skill_id}@${published.version}`;
}

function nativeRunxBinaryForTest(): string {
  const existing = process.env.RUNX_RUST_CLI_BIN;
  if (existing) {
    return existing;
  }
  const candidate = path.resolve("crates/target/debug/runx");
  return existsSync(candidate) ? candidate : "runx";
}
