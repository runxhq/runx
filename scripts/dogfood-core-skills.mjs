#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  createRegistryTestSigningKey,
  signSingleRegistryVersion,
} from "./lib/registry-test-signing.mjs";

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const pnpm = process.platform === "win32" ? "pnpm.cmd" : "pnpm";
const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
const cargoTargetDir = process.env.CARGO_TARGET_DIR
  ? path.resolve(workspaceRoot, process.env.CARGO_TARGET_DIR)
  : path.join(workspaceRoot, "crates", "target");
const rustKernelBin = path.join(
  cargoTargetDir,
  "debug",
  process.platform === "win32" ? "runx.exe" : "runx",
);
const dogfoodEnv = {
  ...process.env,
  RUNX_RUST_CLI_BIN: rustKernelBin,
};
const registryResolverOnly = process.argv.includes("--registry-resolver");

if (registryResolverOnly) {
  runRegistryResolverDogfood();
  process.exit(0);
}

const steps = [
  {
    label: "build rust kernel and JavaScript worker",
    command: cargo,
    args: [
      "build",
      "--quiet",
      "--manifest-path",
      "crates/Cargo.toml",
      "-p",
      "runx-cli",
      "-p",
      "runx-js-worker",
      "--bins",
    ],
  },
  {
    label: "build workspace packages",
    command: pnpm,
    args: ["build"],
  },
  {
    label: "run workspace doctor",
    command: rustKernelBin,
    args: ["doctor", "--json"],
  },
  {
    label: "prove hosted payment boundary and local simulators",
    command: pnpm,
    args: ["exec", "vitest", "run", "tests/payment-skill-profile-validation.test.ts"],
  },
  {
    label: "prove runx-maintained skills with a fresh isolated caller",
    command: process.execPath,
    args: [
      "scripts/harness-sweep.mjs",
      "--no-build",
      "--runx-bin",
      rustKernelBin,
    ],
  },
];

for (const step of steps) {
  process.stdout.write(`\n[dogfood] ${step.label}\n`);
  const result = spawnSync(step.command, step.args, {
    stdio: "inherit",
    shell: false,
    cwd: workspaceRoot,
    env: dogfoodEnv,
  });
  if (result.status === 0) {
    continue;
  }
  process.exit(result.status ?? 1);
}

function runRegistryResolverDogfood() {
  runStep({
    label: "build native runx and JavaScript worker",
    command: cargo,
    args: [
      "build",
      "--quiet",
      "--manifest-path",
      "crates/Cargo.toml",
      "-p",
      "runx-cli",
      "-p",
      "runx-js-worker",
      "--bins",
    ],
  });

  const root = mkdtempSync(path.join(os.tmpdir(), "runx-registry-dogfood-"));
  mkdirSync(path.join(root, "project"), { recursive: true });
  try {
    const registryDir = path.join(root, "registry");
    const skillDir = path.join(root, "echo");
    mkdirSync(skillDir, { recursive: true });
    writeFileSync(skillDirPath(skillDir, "SKILL.md"), "---\nname: echo\n---\n# Echo\n", "utf8");
    writeFileSync(
      skillDirPath(skillDir, "run.yaml"),
      [
        "skill: echo",
        "harness:",
        "  cases:",
        "    - name: registry-agent-boundary",
        "      runner: default",
        "      expect: { status: needs_agent }",
        "runners:",
        "  default:",
        "    type: agent",
        "    default: true",
        "    agent: fixture",
        "    task: echo",
        "    outputs:",
        "      result: object",
        "",
      ].join("\n"),
      "utf8",
    );

    const signingKey = createRegistryTestSigningKey({
      keyId: "runx-dogfood-registry-ed25519",
      signerId: "runx-dogfood-registry",
    });
    const env = {
      ...dogfoodEnv,
      RUNX_HOME: path.join(root, "home"),
      // A fetched skill is added to the project's skills directory.
      RUNX_CWD: path.join(root, "project"),
      RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID: signingKey.keyId,
      RUNX_REGISTRY_MANIFEST_TRUST_KEY_BASE64: signingKey.publicKeyBase64,
      RUNX_REGISTRY_MANIFEST_TRUST_PUBLISHER: "acme",
    };

    runStep({
      label: "publish signed local registry skill",
      command: rustKernelBin,
      args: [
        "registry",
        "publish",
        skillDir,
        "--registry",
        registryDir,
        "--publisher",
        "acme",
        "--version",
        "1.0.0",
        "--json",
      ],
      env,
    });
    signSingleRegistryVersion(registryDir, signingKey);

    const result = spawnSync(
      rustKernelBin,
      [
        "skill",
        "acme/echo@1.0.0",
        "--registry",
        registryDir,
        "--json",
      ],
      {
        stdio: ["ignore", "pipe", "pipe"],
        shell: false,
        cwd: workspaceRoot,
        env,
        encoding: "utf8",
      },
    );
    if (result.status !== 2) {
      process.stderr.write(result.stderr || result.stdout);
      throw new Error(`native registry skill dogfood exited ${result.status}, expected 2`);
    }
    const output = JSON.parse(result.stdout);
    const skillDirectory = output.requests?.[0]?.invocation?.envelope?.execution_location?.skill_directory;
    if (!skillDirectory || !String(skillDirectory).includes(`${path.sep}skills${path.sep}`)) {
      throw new Error(`native registry resolver did not add the skill to the project's skills directory: ${skillDirectory}`);
    }
    if (!statSync(path.join(skillDirectory, "SKILL.md")).isFile()) {
      throw new Error(`native registry resolver did not materialize ${skillDirectory}/SKILL.md`);
    }
    process.stdout.write(`[dogfood] native registry skill resolved to ${skillDirectory}\n`);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function runStep(step) {
  process.stdout.write(`\n[dogfood] ${step.label}\n`);
  const result = spawnSync(step.command, step.args, {
    stdio: "inherit",
    shell: false,
    cwd: workspaceRoot,
    env: step.env ?? dogfoodEnv,
  });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

function skillDirPath(skillDir, file) {
  return path.join(skillDir, file);
}
