#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { startLoopbackRegistry } from "./lib/loopback-registry.mjs";

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
  await runRegistryResolverDogfood();
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
      "scripts/test-sweep.mjs",
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

async function runRegistryResolverDogfood() {
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
  // A hosted registry on loopback, which signs what it publishes.
  const registry = await startLoopbackRegistry({ publisher: "acme" });
  try {
    const skillDir = path.join(root, "echo");
    mkdirSync(skillDir, { recursive: true });
    writeFileSync(
      skillDirPath(skillDir, "SKILL.md"),
      "---\nname: echo\ndescription: Echo fixture for the registry resolver dogfood.\n---\n# Echo\n",
      "utf8",
    );
    writeFileSync(
      skillDirPath(skillDir, "run.yaml"),
      [
        "skill: echo",
        'version: "1.0.0"',
        "harness:",
        "  cases:",
        "    - name: registry-agent-boundary",
        "      runner: default",
        "      expect: { status: needs_agent }",
        "runners:",
        "  default:",
        "    default: true",
        "    steps:",
        "      - id: default",
        "        agent: { role: fixture, task: echo }",
        "        outputs:",
        "          result: object",
        "",
      ].join("\n"),
      "utf8",
    );

    const env = {
      ...dogfoodEnv,
      RUNX_HOME: path.join(root, "home"),
      // A fetched skill is added to the project's skills directory.
      RUNX_CWD: path.join(root, "project"),
      ...registry.env,
    };

    runStep({
      label: "publish a skill to the loopback registry",
      command: rustKernelBin,
      args: [
        "registry",
        "publish",
        skillDir,
        "--registry",
        registry.url,
        "--publisher",
        "acme",
        "--json",
      ],
      env: { ...env, ...registry.publisherEnv },
    });

    const result = spawnSync(
      rustKernelBin,
      [
        "skill",
        "acme/echo@1.0.0",
        "--registry",
        registry.url,
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
    // The pending agent request is handed back by reference, as an artifact.
    const requestPath = output.requests?.[0]?.artifact?.path;
    const request = requestPath ? JSON.parse(readFileSync(requestPath, "utf8")) : undefined;
    const skillDirectory = request?.invocation?.envelope?.execution_location?.skill_directory;
    if (!skillDirectory || !String(skillDirectory).includes(`${path.sep}added-skills${path.sep}`)) {
      throw new Error(`native registry resolver did not add the skill to the project's added skills: ${skillDirectory}`);
    }
    if (!statSync(path.join(skillDirectory, "SKILL.md")).isFile()) {
      throw new Error(`native registry resolver did not materialize ${skillDirectory}/SKILL.md`);
    }
    process.stdout.write(`[dogfood] native registry skill resolved to ${skillDirectory}\n`);
  } finally {
    await registry.stop();
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
