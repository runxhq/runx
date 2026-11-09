#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { inspectGitCheckout, optionValue, writeJson } from "./lib/x402-conformance.mjs";

const DEFAULT_UPSTREAM_DIR = "/tmp/x402-upstream";
const DEFAULT_ENDPOINT = "/exact/evm/eip3009";
const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const sourcePin = readJson(path.join(repositoryRoot, "fixtures", "contracts", "x402-v2", "upstream-pin.json"));
const expectedUpstreamSha = sourcePin.revision;
const REQUIRED_ENV = [
  "SERVER_EVM_ADDRESS",
  "CLIENT_EVM_PRIVATE_KEY",
  "FACILITATOR_EVM_PRIVATE_KEY",
  // The current upstream e2e method checks these before applying the EVM-only filter.
  "SERVER_SVM_ADDRESS",
  "CLIENT_SVM_PRIVATE_KEY",
  "FACILITATOR_SVM_PRIVATE_KEY",
];

const args = process.argv.slice(2);

if (args.includes("--help") || args.includes("-h")) {
  usage(0);
}

const mode = args.includes("--run") ? "run" : "check";
const upstreamDir = optionValue(args, "--upstream-dir") || process.env.X402_UPSTREAM_DIR || DEFAULT_UPSTREAM_DIR;
const artifactDir =
  optionValue(args, "--artifact-dir") || process.env.RUNX_X402_CONFORMANCE_ARTIFACT_DIR || path.join(os.tmpdir(), "runx-x402-upstream-conformance");
const endpoint = optionValue(args, "--endpoint") || process.env.RUNX_X402_CONFORMANCE_ENDPOINT || DEFAULT_ENDPOINT;
const e2eDir = path.join(upstreamDir, "e2e");

const upstream = inspectGitCheckout(upstreamDir, path.join(e2eDir, "package.json"));
const sourceVerification = inspectSourcePins(upstreamDir, sourcePin.sources);
const missingEnv = REQUIRED_ENV.filter((name) => !process.env[name]);
const command = buildCommand({ e2eDir, artifactDir, endpoint });
const pinMatches = /^[0-9a-f]{40}$/u.test(expectedUpstreamSha) && upstream.sha === expectedUpstreamSha;
const report = {
  schema: "runx.x402.upstream_conformance.v1",
  mode,
  upstream_dir: upstreamDir,
  upstream_available: upstream.available,
  upstream_sha: upstream.sha,
  expected_upstream_sha: expectedUpstreamSha,
  pin_matches: pinMatches,
  source_pin_matches: sourceVerification.matches,
  source_pin_checked: sourceVerification.checked,
  source_pin_mismatches: sourceVerification.mismatches,
  artifact_dir: artifactDir,
  endpoint,
  required_env: REQUIRED_ENV,
  missing_env: missingEnv,
  command,
  can_run: upstream.available && pinMatches && sourceVerification.matches && missingEnv.length === 0,
  notes: [
    "This wraps the upstream x402 e2e method; it does not patch or copy upstream protocol code into runx.",
    "The upstream mock-facilitator is startup-only and intentionally fails if /verify or /settle are called.",
    "Use dedicated funded testnet wallets only; the upstream e2e method may move funds between configured wallets.",
  ],
};

if (mode === "check") {
  writeJson(report);
  process.exit(upstream.available && pinMatches && sourceVerification.matches ? 0 : 1);
}

if (!upstream.available) {
  writeJson(report);
  fail(`x402 upstream checkout not found at ${upstreamDir}`);
}
if (!pinMatches) {
  writeJson(report);
  fail(`x402 upstream checkout must be pinned to ${expectedUpstreamSha}`);
}
if (!sourceVerification.matches) {
  writeJson(report);
  fail("x402 upstream source files do not match the repo-owned SHA-256 pin");
}
if (missingEnv.length > 0) {
  writeJson(report);
  fail(`missing required environment variables: ${missingEnv.join(", ")}`);
}

mkdirSync(artifactDir, { recursive: true });
writeFileSync(path.join(artifactDir, "x402-upstream-conformance-preflight.json"), `${JSON.stringify(report, null, 2)}\n`);

const result = spawnSync(command[0], command.slice(1), {
  cwd: e2eDir,
  env: process.env,
  stdio: "inherit",
});
process.exit(result.status ?? 1);

function buildCommand({ e2eDir: dir, artifactDir: outDir, endpoint: endpointPath }) {
  return [
    "pnpm",
    "--dir",
    dir,
    "test",
    "--testnet",
    "--families=evm",
    "--versions=2",
    "--schemes=exact",
    "--clients=fetch",
    "--servers=express",
    "--facilitators=typescript",
    `--endpoints=${endpointPath}`,
    "--min",
    `--output-json=${path.join(outDir, "x402-upstream-e2e.json")}`,
    `--log=${path.join(outDir, "x402-upstream-e2e.log")}`,
  ];
}

function inspectSourcePins(dir, sources) {
  if (!Array.isArray(sources)) {
    return { matches: false, checked: 0, mismatches: [{ path: null, reason: "pin_sources_missing" }] };
  }
  const root = path.resolve(dir);
  const mismatches = [];
  let checked = 0;
  for (const source of sources) {
    if (typeof source?.path !== "string" || !/^sha256:[0-9a-f]{64}$/u.test(source?.digest ?? "")) {
      mismatches.push({ path: source?.path ?? null, reason: "invalid_pin_entry" });
      continue;
    }
    const sourcePath = path.resolve(root, source.path);
    if (sourcePath !== root && !sourcePath.startsWith(`${root}${path.sep}`)) {
      mismatches.push({ path: source.path, reason: "path_outside_checkout" });
      continue;
    }
    if (!existsSync(sourcePath)) {
      mismatches.push({ path: source.path, reason: "source_missing" });
      continue;
    }
    const actual = `sha256:${createHash("sha256").update(readFileSync(sourcePath)).digest("hex")}`;
    checked += 1;
    if (actual !== source.digest) {
      mismatches.push({ path: source.path, reason: "digest_mismatch", expected: source.digest, actual });
    }
  }
  return { matches: checked === sources.length && mismatches.length === 0, checked, mismatches };
}

function readJson(filePath) {
  try {
    return JSON.parse(readFileSync(filePath, "utf8"));
  } catch (error) {
    fail(`cannot read upstream pin: ${error instanceof Error ? error.message : String(error)}`);
  }
}

function fail(message) {
  process.stderr.write(`x402-upstream-conformance: ${message}\n`);
  process.exit(1);
}

function usage(code) {
  process.stderr.write(
    [
      "usage:",
      "  node scripts/x402-upstream-conformance.mjs --check [--upstream-dir DIR] [--artifact-dir DIR]",
      "  node scripts/x402-upstream-conformance.mjs --run [--upstream-dir DIR] [--artifact-dir DIR]",
      "",
      "default upstream dir: /tmp/x402-upstream",
      "",
      "minimal official scenario:",
      "  pnpm test --testnet --families=evm --versions=2 --schemes=exact --clients=fetch --servers=express --facilitators=typescript --endpoints=/exact/evm/eip3009 --min",
    ].join("\n") + "\n",
  );
  process.exit(code);
}
