import { generateKeyPairSync, sign, type KeyObject } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import os from "node:os";
import path from "node:path";

import { describe, expect, it } from "vitest";
import {
  REGISTRY_SIGNED_MANIFEST_SCHEMA,
  registryContentDigest,
  registrySignedManifestPayload,
  type UnsignedRegistryManifest,
} from "@runxhq/contracts";

/** One bounded source packet, as the research skill's default runner takes it. */
const SOURCE_PACKETS = JSON.stringify([{
  final_url: "https://example.com/api",
  status: 200,
  content_digest: `sha256:${"a".repeat(64)}`,
  extracted: "The API exposes a stable bounded operation contract.",
  provenance: { fetched_at: "2026-07-18T00:00:00Z", bytes: 52, redirects: [] },
}]);

describe("registry skill native fetch", () => {
  it("acquires, caches, and reruns a registry skill through the native resolver", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "runx-maintained-fetch-"));
    const projectDir = path.join(tempDir, "project");
    const globalHomeDir = path.join(tempDir, "home");
    const env = testEnv(projectDir, globalHomeDir);

    try {
      await mkdir(projectDir, { recursive: true });
      const registryDir = path.join(tempDir, "registry");
      const runxMaintainedLock = await runxMaintainedSkillLock("runx/research");
      publishLocalRegistrySkill({
        registryDir,
        subject: path.resolve("skills/research/SKILL.md"),
        profile: path.resolve("skills/research/run.yaml"),
        owner: "acme",
        version: runxMaintainedLock.version,
        env,
      });
      const skillRef = `acme/research@${runxMaintainedLock.version}`;

      const first = runNativeSkill(env, [
        skillRef,
        "--registry",
        registryDir,
        "--input",
        "objective=Decide whether the supplied evidence supports an integration.",
        "--input",
        `source_packets=${SOURCE_PACKETS}`,
        "--json",
      ]);
      const firstJson = parseJsonOutput(first, 2);
      expect((firstJson as { status?: string }).status).toBe("needs_agent");
      const firstPath = findAddedSkillPath(path.join(projectDir, "skills"), "acme/research");
      expect((await stat(path.join(firstPath, "SKILL.md"))).isFile()).toBe(true);
      expect((await stat(path.join(firstPath, "run.yaml"))).isFile()).toBe(true);

      const second = runNativeSkill(env, [
        skillRef,
        "--registry",
        registryDir,
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
    const env = testEnv(projectDir, globalHomeDir);

    try {
      await mkdir(projectDir, { recursive: true });
      const registryDir = path.join(tempDir, "registry");
      const runxMaintainedLock = await runxMaintainedSkillLock("runx/research");
      publishLocalRegistrySkill({
        registryDir,
        subject: path.resolve("skills/research/SKILL.md"),
        profile: path.resolve("skills/research/run.yaml"),
        owner: "acme",
        version: runxMaintainedLock.version,
        env,
      });

      const result = runNativeSkill(env, [
        `acme/research@${runxMaintainedLock.version}`,
        "--registry",
        registryDir,
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

  it("copies graph stages beside cached registry graph skills", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "runx-maintained-fetch-stages-"));
    const projectDir = path.join(tempDir, "project");
    const globalHomeDir = path.join(tempDir, "home");
    const env = testEnv(projectDir, globalHomeDir);

    try {
      await mkdir(projectDir, { recursive: true });
      const registryDir = path.join(tempDir, "registry");
      const lockEntry = await runxMaintainedSkillLock("runx/agency");
      publishLocalRegistrySkill({
        registryDir,
        subject: path.resolve("skills/agency/SKILL.md"),
        profile: path.resolve("skills/agency/run.yaml"),
        owner: "acme",
        version: lockEntry.version,
        env,
      });

      const result = runNativeSkill(env, [
        "inspect",
        `acme/agency@${lockEntry.version}`,
        "--registry",
        registryDir,
        "--json",
      ]);
      const output = parseJsonOutput(result, 0);
      expect((output as { status?: string }).status).toBe("ok");
      const skillPath = findAddedSkillPath(path.join(projectDir, "skills"), "acme/agency");
      for (const stage of ["plan", "reduce"]) {
        expect(
          (await stat(
            path.join(skillPath, "graph", stage, "run.yaml"),
          )).isFile(),
          stage,
        ).toBe(true);
      }
    } finally {
      await rm(tempDir, { recursive: true, force: true });
    }
  });

  it("runs cached registry graph skills through native data operations", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "runx-maintained-fetch-data-store-"));
    const projectDir = path.join(tempDir, "project");
    const globalHomeDir = path.join(tempDir, "home");
    const env = testEnv(projectDir, globalHomeDir);

    try {
      await mkdir(projectDir, { recursive: true });
      const registryDir = path.join(tempDir, "registry");
      const lockEntry = await runxMaintainedSkillLock("runx/data-store");
      publishLocalRegistrySkill({
        registryDir,
        subject: path.resolve("skills/data-store/SKILL.md"),
        profile: path.resolve("skills/data-store/run.yaml"),
        owner: "acme",
        version: lockEntry.version,
        env,
      });

      const result = runNativeSkill(env, [
        `acme/data-store@${lockEntry.version}`,
        "append_event",
        "--registry",
        registryDir,
        "--input",
        "data_source_ref=local://runx-data-store/maintained-cache",
        "--input",
        "resource=board_events",
        "--input",
        "aggregate_id=posting-123",
        "--input",
        "expected_version=0",
        "--input",
        "idempotency_key=posting-123:create:v1",
        "--input",
        "event={\"type\":\"posting.created\",\"payload\":{\"title\":\"cached data-store smoke\"}}",
        "--json",
      ]);
      const output = parseJsonOutput(result, 0) as { status?: string };

      expect(output.status).toBe("sealed");
    } finally {
      await rm(tempDir, { recursive: true, force: true });
    }
  });
});

function testEnv(projectDir: string, globalHomeDir: string): NodeJS.ProcessEnv {
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
  delete env.RUNX_REGISTRY_DIR;
  delete env.RUNX_REGISTRY_URL;
  delete env.RUNX_REGISTRY_SOURCE_AUTHORITY;
  delete env.RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID;
  delete env.RUNX_REGISTRY_MANIFEST_TRUST_KEY_BASE64;
  delete env.RUNX_REGISTRY_MANIFEST_TRUST_OWNER;
  return env;
}

async function runxMaintainedSkillLock(skillId: string): Promise<{
  readonly skill_id: string;
  readonly version: string;
  readonly digest: string;
}> {
  const runxMaintainedLock = JSON.parse(
    await readFile(path.resolve("skills/runx-maintained.lock.json"), "utf8"),
  ) as ReadonlyArray<{
    readonly skill_id: string;
    readonly version: string;
    readonly digest: string;
  }>;
  const entry = runxMaintainedLock.find((candidate) => candidate.skill_id === skillId);
  if (!entry) {
    throw new Error(`Missing ${skillId} entry in runx-maintained.lock.json.`);
  }
  return entry;
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

function publishLocalRegistrySkill(input: {
  readonly registryDir: string;
  readonly subject: string;
  readonly owner: string;
  readonly version: string;
  readonly env: NodeJS.ProcessEnv;
  readonly profile?: string;
  readonly trustTier?: "verified" | "community";
}): void {
  input.env.RUNX_REGISTRY_DIR = input.registryDir;
  delete input.env.RUNX_REGISTRY_SOURCE_AUTHORITY;
  const signingKey = testManifestSigningKey();
  input.env.RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID = signingKey.keyId;
  input.env.RUNX_REGISTRY_MANIFEST_TRUST_KEY_BASE64 = signingKey.publicKeyBase64;
  input.env.RUNX_REGISTRY_MANIFEST_TRUST_OWNER = input.owner;
  const args = [
    "registry",
    "publish",
    input.subject,
    "--registry-dir",
    input.registryDir,
    "--publisher",
    input.owner,
    "--version",
    input.version,
    "--trust-tier",
    input.trustTier ?? "community",
    "--upsert",
    "--json",
  ];
  if (input.profile) {
    args.push("--profile", input.profile);
  }
  const result = spawnSync(input.env.RUNX_RUST_CLI_BIN ?? "runx", args, {
    cwd: process.cwd(),
    env: input.env,
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw new Error(`failed to publish local registry fixture: ${result.stderr || result.stdout}`);
  }
  signPublishedRegistryEntry(input.registryDir, signingKey);
}

interface TestManifestSigningKey {
  readonly keyId: string;
  readonly signerId: string;
  readonly publicKeyBase64: string;
  readonly privateKey: KeyObject;
}

let cachedManifestSigningKey: TestManifestSigningKey | undefined;

function testManifestSigningKey(): TestManifestSigningKey {
  if (cachedManifestSigningKey) {
    return cachedManifestSigningKey;
  }
  const keyPair = generateKeyPairSync("ed25519");
  const publicKeyDer = keyPair.publicKey.export({ format: "der", type: "spki" });
  const publicKeyRaw = Buffer.from(publicKeyDer).subarray(-32);
  cachedManifestSigningKey = {
    keyId: "runx-test-registry-ed25519",
    signerId: "runx-test-registry",
    publicKeyBase64: publicKeyRaw.toString("base64"),
    privateKey: keyPair.privateKey,
  };
  return cachedManifestSigningKey;
}

/** Signs a local registry record as the hosted registry would: the unified envelope over its content digest. */
function signPublishedRegistryEntry(registryDir: string, signingKey: TestManifestSigningKey): void {
  const entryPath = findSingleRegistryEntry(registryDir);
  const entry = JSON.parse(readFileSync(entryPath, "utf8")) as {
    skill_id: string;
    version: string;
    markdown: string;
    profile_document?: string;
    package_files?: readonly { readonly path: string; readonly content: string }[];
    signed_manifest?: unknown;
  };
  const unsigned: UnsignedRegistryManifest = {
    schema: REGISTRY_SIGNED_MANIFEST_SCHEMA,
    subject: { kind: "skill", ref: entry.skill_id },
    version: entry.version,
    content_digest: registryContentDigest([
      { path: "SKILL.md", content: entry.markdown },
      ...(entry.profile_document === undefined ? [] : [{ path: "run.yaml", content: entry.profile_document }]),
      ...(entry.package_files ?? []),
    ]),
    signer: { id: signingKey.signerId, key_id: signingKey.keyId },
  };
  const signature = sign(null, Buffer.from(registrySignedManifestPayload(unsigned), "utf8"), signingKey.privateKey);
  entry.signed_manifest = { ...unsigned, signature: { alg: "ed25519", value: `base64:${signature.toString("base64url")}` } };
  writeFileSync(entryPath, `${JSON.stringify(entry, null, 2)}\n`, "utf8");
}

function findSingleRegistryEntry(root: string): string {
  const matches: string[] = [];
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir)) {
      const entryPath = path.join(dir, entry);
      const stats = statSync(entryPath);
      if (stats.isDirectory()) {
        walk(entryPath);
      } else if (entryPath.endsWith(".json")) {
        matches.push(entryPath);
      }
    }
  };
  walk(root);
  if (matches.length !== 1) {
    throw new Error(`expected one registry fixture entry, found ${matches.length}`);
  }
  return matches[0];
}

function nativeRunxBinaryForTest(): string {
  const existing = process.env.RUNX_RUST_CLI_BIN;
  if (existing) {
    return existing;
  }
  const candidate = path.resolve("crates/target/debug/runx");
  return existsSync(candidate) ? candidate : "runx";
}
