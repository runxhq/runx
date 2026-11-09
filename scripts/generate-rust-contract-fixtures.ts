import { spawnSync } from "node:child_process";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { sha256Prefixed } from "@runxhq/contracts";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const fixtureRoot = path.join(workspaceRoot, "fixtures", "contracts");

interface ContractFixture {
  readonly name: string;
  readonly scope: ContractScope;
  readonly description: string;
  readonly input?: unknown;
  readonly fixture_kind?: ContractFixtureKind;
  readonly expected: unknown;
}

type HostFixtureKind = "event" | "resolution_request" | "resolution_response";
type ContractFixtureKind = HostFixtureKind;
type ContractScope =
  | "host-protocol"
  | "purchase"
  | "paid-skill-price"
  | "principal-id"
  | "registry-tier-statement"
  | "x402-v2";

const selectedScope = scopeArg();
const check = process.argv.includes("--check");

if (
  selectedScope !== undefined
  && selectedScope !== "host-protocol"
  && selectedScope !== "purchase"
  && selectedScope !== "paid-skill-price"
  && selectedScope !== "principal-id"
  && selectedScope !== "registry-tier-statement"
  && selectedScope !== "x402-v2"
) {
  throw new Error(`unsupported contract fixture scope: ${selectedScope}`);
}

if (selectedScope === undefined || selectedScope === "host-protocol") {
  const fixtures = buildHostProtocolFixtures();
  await writeFixtures(fixtures, path.join(fixtureRoot, "host-protocol"));
}
if (selectedScope === undefined || selectedScope === "purchase") {
  runPurchaseFixtures();
}
if (selectedScope === undefined || selectedScope === "paid-skill-price") {
  runPaidSkillPriceFixtures();
}
if (selectedScope === undefined || selectedScope === "principal-id") {
  runPrincipalIdFixtures();
}
if (selectedScope === undefined || selectedScope === "registry-tier-statement") {
  runExampleFixtures(
    "registry-tier-statement",
    "runx-registry-tier-statement-fixtures",
    "RUNX_REGISTRY_TIER_STATEMENT_FIXTURES_BIN",
  );
}
if (selectedScope === undefined || selectedScope === "x402-v2") {
  runX402Fixtures();
}

function runPurchaseFixtures(): void {
  const outputRoot = path.join(fixtureRoot, "purchase");
  const configured = process.env.RUNX_PURCHASE_FIXTURES_BIN;
  const command = configured || (process.platform === "win32" ? "cargo.exe" : "cargo");
  const args = configured
    ? ["--out", outputRoot, "--schema-dir", path.join(workspaceRoot, "schemas")]
    : [
        "run",
        "--quiet",
        "--manifest-path",
        path.join(workspaceRoot, "crates", "Cargo.toml"),
        "-p",
        "runx-contracts",
        "--example",
        "runx-purchase-fixtures",
        "--",
        "--out",
        outputRoot,
        "--schema-dir",
        path.join(workspaceRoot, "schemas"),
      ];
  if (check) args.push("--check");
  const result = spawnSync(command, args, { cwd: workspaceRoot, env: process.env, stdio: "inherit" });
  if (result.error) throw result.error;
  if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
}

function runPaidSkillPriceFixtures(): void {
  const outputRoot = path.join(fixtureRoot, "paid-skill-price");
  const configured = process.env.RUNX_PAID_SKILL_PRICE_FIXTURES_BIN;
  const command = configured || (process.platform === "win32" ? "cargo.exe" : "cargo");
  const args = configured
    ? ["--out", outputRoot]
    : [
        "run",
        "--quiet",
        "--manifest-path",
        path.join(workspaceRoot, "crates", "Cargo.toml"),
        "-p",
        "runx-contracts",
        "--example",
        "runx-paid-skill-price-fixtures",
        "--",
        "--out",
        outputRoot,
      ];
  if (check) args.push("--check");
  const result = spawnSync(command, args, { cwd: workspaceRoot, env: process.env, stdio: "inherit" });
  if (result.error) throw result.error;
  if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
}

/** A contract example that writes one fixture directory and takes only --out and --check. */
function runExampleFixtures(scope: string, example: string, binEnv: string): void {
  const outputRoot = path.join(fixtureRoot, scope);
  const configured = process.env[binEnv];
  const command = configured || (process.platform === "win32" ? "cargo.exe" : "cargo");
  const args = configured
    ? ["--out", outputRoot]
    : [
        "run",
        "--quiet",
        "--manifest-path",
        path.join(workspaceRoot, "crates", "Cargo.toml"),
        "-p",
        "runx-contracts",
        "--example",
        example,
        "--",
        "--out",
        outputRoot,
      ];
  if (check) args.push("--check");
  const result = spawnSync(command, args, { cwd: workspaceRoot, env: process.env, stdio: "inherit" });
  if (result.error) throw result.error;
  if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
}

function runPrincipalIdFixtures(): void {
  const outputRoot = path.join(fixtureRoot, "principal-id");
  const configured = process.env.RUNX_PRINCIPAL_ID_FIXTURES_BIN;
  const command = configured || (process.platform === "win32" ? "cargo.exe" : "cargo");
  const args = configured
    ? ["--out", outputRoot]
    : [
        "run",
        "--quiet",
        "--manifest-path",
        path.join(workspaceRoot, "crates", "Cargo.toml"),
        "-p",
        "runx-contracts",
        "--example",
        "runx-principal-id-fixtures",
        "--",
        "--out",
        outputRoot,
      ];
  if (check) args.push("--check");
  const result = spawnSync(command, args, { cwd: workspaceRoot, env: process.env, stdio: "inherit" });
  if (result.error) throw result.error;
  if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
}

function runX402Fixtures(): void {
  const outputRoot = path.join(fixtureRoot, "x402-v2");
  const configured = process.env.RUNX_X402_FIXTURES_BIN;
  const command = configured || (process.platform === "win32" ? "cargo.exe" : "cargo");
  const args = configured
    ? ["--out", outputRoot, "--schema-dir", path.join(workspaceRoot, "schemas")]
    : [
        "run",
        "--quiet",
        "--manifest-path",
        path.join(workspaceRoot, "crates", "Cargo.toml"),
        "-p",
        "runx-contracts",
        "--example",
        "runx-x402-fixtures",
        "--",
        "--out",
        outputRoot,
        "--schema-dir",
        path.join(workspaceRoot, "schemas"),
      ];
  if (check) args.push("--check");
  const result = spawnSync(command, args, { cwd: workspaceRoot, env: process.env, stdio: "inherit" });
  if (result.error) throw result.error;
  if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
}

async function writeFixtures(fixtures: readonly ContractFixture[], directory: string): Promise<void> {
  const expectedFiles = new Set<string>();

  for (const fixture of fixtures) {
    assertAsciiObjectKeys(fixture, fixture.name);
    const filePath = path.join(directory, `${fixture.name}.json`);
    expectedFiles.add(filePath);
    const content = `${stableJson(fixture)}\n`;
    if (check) {
      const existing = await readFixture(filePath);
      if (existing !== content) {
        throw new Error(`fixture is stale: ${path.relative(workspaceRoot, filePath)}`);
      }
      continue;
    }
    await mkdir(directory, { recursive: true });
    await writeFile(filePath, content);
  }

  if (check) {
    for (const filePath of await collectJsonFiles(directory)) {
      if (!expectedFiles.has(filePath)) {
        throw new Error(`stale fixture file: ${path.relative(workspaceRoot, filePath)}`);
      }
    }
  }

  console.log(`${check ? "checked" : "generated"} ${fixtures.length} contract fixtures`);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function reference(type: string, uri: string): Readonly<Record<string, string>> {
  return { type, uri };
}

function buildHostProtocolFixtures(): readonly ContractFixture[] {
  const fixtures: ContractFixture[] = [
    ...eventFixtures(),
    hostFixture("resolution-input-request", "resolution_request", inputResolutionRequest()),
    hostFixture("resolution-approval-request", "resolution_request", approvalResolutionRequest()),
    hostFixture("resolution-agent-act-request", "resolution_request", turnResolutionRequest()),
    hostFixture("resolution-response", "resolution_response", {
      actor: "human",
      payload: {
        answer: "Proceed",
      },
    }),
  ];
  return fixtures.sort((left, right) => left.name.localeCompare(right.name));
}

function eventFixtures(): readonly ContractFixture[] {
  return [
    "skill_loaded",
    "inputs_resolved",
    "auth_resolved",
    "resolution_requested",
    "resolution_resolved",
    "admitted",
    "executing",
    "step_started",
    "step_waiting_resolution",
    "step_completed",
    "warning",
    "completed",
  ].map((type) => hostFixture(`event-${type}`, "event", event(type)));
}

function hostFixture(name: string, fixtureKind: HostFixtureKind, expected: unknown): ContractFixture {
  return {
    name,
    scope: "host-protocol",
    description: `Host protocol ${fixtureKind} fixture generated from the TypeScript serializable wire subset.`,
    fixture_kind: fixtureKind,
    expected,
  };
}

function event(type: string): Readonly<Record<string, unknown>> {
  return {
    type,
    message: `event ${type}`,
    data: {
      fixture: type,
    },
  };
}

function inputResolutionRequest(): Readonly<Record<string, unknown>> {
  return {
    id: "req_input",
    kind: "input",
    questions: [
      {
        id: "objective",
        prompt: "What should runx do?",
        required: true,
        type: "string",
      },
    ],
  };
}

function approvalResolutionRequest(): Readonly<Record<string, unknown>> {
  return {
    id: "req_approval",
    kind: "approval",
    gate: {
      id: "workspace-write",
      reason: "Allow workspace write",
      type: "mutation",
      summary: {
        path: "docs/guide.md",
      },
    },
  };
}

function turnResolutionRequest(): Readonly<Record<string, unknown>> {
  return {
    id: "req_act",
    kind: "turn",
    invocation: {
      id: "act_1",
      task: "summarize-receipt",
      envelope: {
        current_context: [],
        historical_context: [],
        inputs: {},
        instructions: "Summarize receipt",
        instructions_digest: sha256Prefixed("Summarize receipt"),
        provenance: [],
        requirements: {
          execution_boundary: { kind: "remote_provider" },
        },
        run_id: "run_1",
        skill: "diagnose-skill-run",
        step_id: "step_1",
        trust_boundary: "test",
      },
      allowance: {
        operations: [{ operation: "issue.read", requirement: "github" }],
        tools: ["fs.read"],
      },
    },
  };
}

function scopeArg(): string | undefined {
  const index = process.argv.indexOf("--scope");
  if (index === -1) {
    return undefined;
  }
  return process.argv[index + 1];
}

async function readFixture(filePath: string): Promise<string> {
  try {
    return await readFile(filePath, "utf8");
  } catch (error) {
    if (isNodeError(error) && error.code === "ENOENT") {
      throw new Error(`missing fixture ${path.relative(workspaceRoot, filePath)}`);
    }
    throw error;
  }
}

async function collectJsonFiles(directory: string): Promise<readonly string[]> {
  const entries = await readdir(directory, { withFileTypes: true });
  const files: string[] = [];
  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...await collectJsonFiles(entryPath));
    } else if (entry.isFile() && entry.name.endsWith(".json")) {
      files.push(entryPath);
    }
  }
  return files.sort();
}

function stableJson(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map((item) => stableJson(item)).join(",")}]`;
  }
  if (value && typeof value === "object") {
    return `{${Object.entries(value)
      .filter(([, nested]) => nested !== undefined)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, nested]) => `${JSON.stringify(key)}:${stableJson(nested)}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function assertAsciiObjectKeys(value: unknown, label: string): void {
  if (Array.isArray(value)) {
    value.forEach((entry, index) => assertAsciiObjectKeys(entry, `${label}[${index}]`));
    return;
  }
  if (typeof value === "number" && !Number.isInteger(value)) {
    throw new Error(`non-integer numeric fixture value is out of scope before hash-stable-codepoint-cutover: ${label}`);
  }
  if (!value || typeof value !== "object") {
    return;
  }
  const keys = Object.keys(value);
  const localeKeys = [...keys].sort((left, right) => left.localeCompare(right));
  const codepointKeys = [...keys].sort(compareCodepoints);
  if (localeKeys.join("\0") !== codepointKeys.join("\0")) {
    throw new Error(
      `object key order differs between TS localeCompare and Rust codepoint sort before hash-stable-codepoint-cutover: ${label}`,
    );
  }
  for (const [key, nested] of Object.entries(value)) {
    if (!/^[\u0020-\u007e]+$/u.test(key)) {
      throw new Error(`non-ASCII object key is out of scope before hash-stable-codepoint-cutover: ${label}.${key}`);
    }
    assertAsciiObjectKeys(nested, `${label}.${key}`);
  }
}

function compareCodepoints(left: string, right: string): number {
  if (left < right) {
    return -1;
  }
  if (left > right) {
    return 1;
  }
  return 0;
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return Boolean(error && typeof error === "object" && "code" in error);
}
