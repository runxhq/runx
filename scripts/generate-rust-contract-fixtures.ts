import { spawnSync } from "node:child_process";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  canonicalJsonStringify,
  sha256Prefixed,
  validateActAssignmentContract,
  type ActAssignmentActorContract,
  type ActAssignmentContract,
  type ActAssignmentHostContract,
} from "@runxhq/contracts";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const fixtureRoot = path.join(workspaceRoot, "fixtures", "contracts");
const sdkFixtureRoot = path.join(workspaceRoot, "fixtures", "sdk-rust");
const actAssignmentRoot = path.join(fixtureRoot, "act-assignment");

interface ContractFixture {
  readonly name: string;
  readonly scope: ContractScope;
  readonly description: string;
  readonly input?: unknown;
  readonly fixture_kind?: ContractFixtureKind;
  readonly expected: unknown;
}

interface ActAssignmentFixtureInput {
  readonly skill: string;
  readonly runner: string;
  readonly source_uri?: string;
  readonly requested_at: string;
  readonly host: {
    readonly kind: "cli" | "api" | "comment" | "system";
    readonly trigger_uri?: string;
    readonly scope_set?: readonly string[];
    readonly actor?: {
      readonly actor_id?: string;
      readonly display_name?: string;
      readonly role?: string;
      readonly provider_identity?: string;
    };
  };
  readonly input_overrides?: Readonly<Record<string, unknown>>;
}

interface ActAssignmentFixtureExpected {
  readonly envelope: unknown;
  readonly intent_key: string;
  readonly trigger_key?: string;
  readonly content_digest: string;
}

type JsonRecord = Readonly<Record<string, unknown>>;
type HostFixtureKind =
  | "event"
  | "resolution_request"
  | "resolution_response"
  | "run_result"
  | "run_state";
type ContractFixtureKind = HostFixtureKind;
type ContractScope =
  | "act-assignment"
  | "host-protocol"
  | "purchase"
  | "paid-skill-price"
  | "principal-id"
  | "purchase-claim"
  | "x402-v2";

const selectedScope = scopeArg();
const check = process.argv.includes("--check");

if (
  selectedScope !== undefined
  && selectedScope !== "act-assignment"
  && selectedScope !== "host-protocol"
  && selectedScope !== "purchase"
  && selectedScope !== "paid-skill-price"
  && selectedScope !== "principal-id"
  && selectedScope !== "purchase-claim"
  && selectedScope !== "x402-v2"
) {
  throw new Error(`unsupported contract fixture scope: ${selectedScope}`);
}

if (selectedScope === undefined || selectedScope === "act-assignment") {
  const fixtures = buildActAssignmentFixtures();
  await writeFixtures(fixtures, actAssignmentRoot);
  await writeFixtures(
    fixtures.filter(({ name }) => ["cli-no-trigger", "github-trigger"].includes(name)),
    path.join(sdkFixtureRoot, "act-assignment"),
  );
}
if (selectedScope === undefined || selectedScope === "host-protocol") {
  const fixtures = buildHostProtocolFixtures();
  await writeFixtures(fixtures, path.join(fixtureRoot, "host-protocol"));
  await writeFixtures(
    fixtures.filter(({ name }) =>
      ["inspect-host-state-needs-agent", "result-host-run-completed"].includes(name),
    ),
    path.join(sdkFixtureRoot, "host-protocol"),
  );
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
if (selectedScope === undefined || selectedScope === "purchase-claim") {
  runExampleFixtures("purchase-claim", "runx-purchase-claim-fixtures", "RUNX_PURCHASE_CLAIM_FIXTURES_BIN");
}
if (selectedScope === undefined || selectedScope === "x402-v2") {
  runX402Fixtures();
}

function runPurchaseFixtures(): void {
  const outputRoot = path.join(fixtureRoot, "purchase");
  const fingerprintOracle = path.join(
    fixtureRoot,
    "canonical-json",
    "runx-purchase-request-digest-v1.oracles.json",
  );
  const configured = process.env.RUNX_PURCHASE_FIXTURES_BIN;
  const command = configured || (process.platform === "win32" ? "cargo.exe" : "cargo");
  const args = configured
    ? ["--out", outputRoot, "--schema-dir", path.join(workspaceRoot, "schemas"), "--oracle-out", fingerprintOracle]
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
        "--oracle-out",
        fingerprintOracle,
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

function buildActAssignmentFixtures(): readonly ContractFixture[] {
  return [
    actAssignmentFixture(
      "github-trigger",
      "ASCII act assignment hashStable fixture. non-ASCII object keys are rejected until hash-stable-codepoint-cutover replaces localeCompare ordering.",
      {
        skill: "outreach",
        runner: "rerun",
        source_uri: "github://sourcey/sourcey.com/issues/3",
        requested_at: "2026-04-25T14:00:00Z",
        host: {
          kind: "comment",
          trigger_uri: "https://github.com/sourcey/sourcey.com/issues/3#issuecomment-1",
          scope_set: ["docs.write", "thread:push"],
          actor: {
            actor_id: "auscaster",
            display_name: "auscaster",
            provider_identity: "github:auscaster",
          },
        },
        input_overrides: {
          build_context: "Keep the MCP surface legible.",
          objective: "Refresh the docs preview.",
        },
      },
    ),
    actAssignmentFixture(
      "cli-no-trigger",
      "ASCII CLI fixture with no trigger key; documents the narrow hashStable scope before the non-ASCII codepoint cutover.",
      {
        skill: "docs.refresh",
        runner: "runx",
        source_uri: "local://workspace",
        requested_at: "2026-04-25T14:01:00Z",
        host: {
          kind: "cli",
        },
        input_overrides: {
          objective: "Refresh docs",
        },
      },
    ),
    actAssignmentFixture(
      "system-empty-inputs",
      "ASCII system fixture whose content hash is computed over an empty object; localeCompare behavior is intentionally unchanged here.",
      {
        skill: "system.audit",
        runner: "system",
        requested_at: "2026-04-25T14:02:00Z",
        host: {
          kind: "system",
        },
      },
    ),
    actAssignmentFixture(
      "host-normalization",
      "Documents buildActAssignment host normalization: empty trigger_uri, scope_set, and actor fields are omitted.",
      {
        skill: "host.normalize",
        runner: "runx",
        requested_at: "2026-04-25T14:03:00Z",
        host: {
          kind: "api",
          trigger_uri: "",
          scope_set: [],
          actor: {
            actor_id: "",
            display_name: "",
            role: "",
            provider_identity: "",
          },
        },
      },
    ),
  ].sort((left, right) => left.name.localeCompare(right.name));
}

function actAssignmentFixture(
  name: string,
  description: string,
  input: ActAssignmentFixtureInput,
): ContractFixture {
  const tsOptions = {
    skill: input.skill,
    runner: input.runner,
    sourceUri: input.source_uri,
    requestedAt: input.requested_at,
    hostKind: input.host.kind,
    triggerUri: input.host.trigger_uri,
    scopeSet: input.host.scope_set,
    actor: input.host.actor,
    inputOverrides: input.input_overrides,
  };
  const envelope = buildActAssignment(tsOptions);
  return {
    name,
    scope: "act-assignment",
    description,
    input,
    expected: {
      envelope,
      intent_key: deriveActAssignmentIntentKey({
        skill: input.skill,
        runner: input.runner,
        sourceUri: input.source_uri,
        inputOverrides: input.input_overrides,
      }),
      trigger_key: deriveActAssignmentTriggerKey({
        hostKind: input.host.kind,
        triggerUri: input.host.trigger_uri,
      }),
      content_digest: deriveActAssignmentContentDigest(input.input_overrides),
    },
  };
}

function buildActAssignment(options: {
  readonly skill: string;
  readonly runner: string;
  readonly sourceUri?: string;
  readonly requestedAt?: string;
  readonly hostKind?: ActAssignmentHostContract["kind"];
  readonly triggerUri?: string;
  readonly scopeSet?: readonly string[];
  readonly actor?: ActAssignmentActorContract;
  readonly inputOverrides?: JsonRecord;
}): ActAssignmentContract {
  const inputOverrides = normalizeActAssignmentRecord(options.inputOverrides);
  const host = normalizeActAssignmentHost({
    kind: options.hostKind ?? "cli",
    trigger_uri: options.triggerUri,
    scope_set: options.scopeSet,
    actor: options.actor,
  });
  const sourceUri = normalizeNonEmptyString(options.sourceUri);
  const requestedAt = normalizeNonEmptyString(options.requestedAt) ?? new Date().toISOString();

  return validateActAssignmentContract(pruneUndefined({
    schema: "runx.act_assignment.v1",
    skill: options.skill,
    runner: options.runner,
    source_uri: sourceUri,
    requested_at: requestedAt,
    host,
    input_overrides: inputOverrides,
    idempotency: {
      algorithm: "sha256",
      intent_key: deriveActAssignmentIntentKey({
        skill: options.skill,
        runner: options.runner,
        sourceUri,
        inputOverrides,
      }),
      trigger_key: deriveActAssignmentTriggerKey({
        hostKind: host.kind,
        triggerUri: host.trigger_uri,
      }),
      content_digest: deriveActAssignmentContentDigest(inputOverrides),
    },
  }));
}

function deriveActAssignmentIntentKey(options: {
  readonly skill: string;
  readonly runner: string;
  readonly sourceUri?: string;
  readonly inputOverrides?: JsonRecord;
}): string {
  return canonicalSha256({
    skill: options.skill,
    runner: options.runner,
    source_uri: normalizeNonEmptyString(options.sourceUri),
    input_overrides: normalizeActAssignmentRecord(options.inputOverrides),
  });
}

function deriveActAssignmentTriggerKey(options: {
  readonly hostKind: ActAssignmentHostContract["kind"];
  readonly triggerUri?: string;
}): string | undefined {
  const triggerUri = normalizeNonEmptyString(options.triggerUri);
  if (!triggerUri) {
    return undefined;
  }
  return canonicalSha256({
    host_kind: options.hostKind,
    trigger_uri: triggerUri,
  });
}

function deriveActAssignmentContentDigest(inputOverrides?: JsonRecord): string {
  return canonicalSha256(normalizeActAssignmentRecord(inputOverrides) ?? {});
}

function normalizeActAssignmentRecord(value: unknown): JsonRecord | undefined {
  if (!isRecord(value)) {
    return undefined;
  }
  const normalized = normalizeUnknown(value);
  return isRecord(normalized) && Object.keys(normalized).length > 0 ? normalized : undefined;
}

function normalizeActAssignmentHost(value: {
  readonly kind: ActAssignmentHostContract["kind"];
  readonly trigger_uri?: string;
  readonly scope_set?: readonly string[];
  readonly actor?: ActAssignmentActorContract;
}): ActAssignmentHostContract {
  const actor = normalizeActAssignmentActor(value.actor);
  const scopeSet = normalizeStringArray(value.scope_set);
  return pruneUndefined({
    kind: value.kind,
    trigger_uri: normalizeNonEmptyString(value.trigger_uri),
    scope_set: scopeSet.length > 0 ? scopeSet : undefined,
    actor,
  }) as ActAssignmentHostContract;
}

function normalizeActAssignmentActor(value: ActAssignmentActorContract | undefined): ActAssignmentActorContract | undefined {
  if (!value) {
    return undefined;
  }
  const actor = {
    actor_id: normalizeNonEmptyString(value.actor_id),
    display_name: normalizeNonEmptyString(value.display_name),
    role: normalizeNonEmptyString(value.role),
    provider_identity: normalizeNonEmptyString(value.provider_identity),
  };
  return Object.values(actor).some((entry) => typeof entry === "string" && entry.length > 0)
    ? pruneUndefined(actor) as ActAssignmentActorContract
    : undefined;
}

function normalizeStringArray(value: readonly string[] | undefined): readonly string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value
    .map((entry) => normalizeNonEmptyString(entry))
    .filter((entry): entry is string => typeof entry === "string");
}

function normalizeUnknown(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map((entry) => normalizeUnknown(entry));
  }
  if (!isRecord(value)) {
    return value;
  }
  const normalized: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (entry === undefined) {
      continue;
    }
    const normalizedEntry = normalizeUnknown(entry);
    if (normalizedEntry === undefined) {
      continue;
    }
    normalized[key] = normalizedEntry;
  }
  return normalized;
}

function canonicalSha256(value: unknown): string {
  return sha256Prefixed(canonicalJsonStringify(pruneUndefined(value)));
}

function normalizeNonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function pruneUndefined<T>(value: T): T {
  if (Array.isArray(value)) {
    return value.map((entry) => pruneUndefined(entry)) as T;
  }
  if (!isRecord(value)) {
    return value;
  }
  const result: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (entry === undefined) {
      continue;
    }
    result[key] = pruneUndefined(entry);
  }
  return result as T;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function reference(type: string, uri: string): Readonly<Record<string, string>> {
  return { type, uri };
}

function buildHostProtocolFixtures(): readonly ContractFixture[] {
  const fixtures: ContractFixture[] = [
    ...hostResultFixtures(),
    ...hostStateFixtures(),
    ...eventFixtures(),
    hostFixture("resolution-input-request", "resolution_request", inputResolutionRequest()),
    hostFixture("resolution-approval-request", "resolution_request", approvalResolutionRequest()),
    hostFixture("resolution-agent-act-request", "resolution_request", agentActResolutionRequest()),
    hostFixture("resolution-response", "resolution_response", {
      actor: "human",
      payload: {
        answer: "Proceed",
      },
    }),
  ];
  return fixtures.sort((left, right) => left.name.localeCompare(right.name));
}

function hostResultFixtures(): readonly ContractFixture[] {
  return [
    hostFixture("result-host-run-needs-agent", "run_result", {
      status: "needs_agent",
      skillName: "diagnose-skill-run",
      runId: "run_needs_agent",
      requests: [inputResolutionRequest()],
      stepIds: ["collect"],
      stepLabels: ["Collect context"],
      events: [event("resolution_requested")],
    }),
    hostFixture("result-host-run-completed", "run_result", {
      status: "completed",
      skillName: "diagnose-skill-run",
      receiptId: "rx_completed",
      output: "done",
      events: [event("completed")],
    }),
    hostFixture("result-host-run-failed", "run_result", {
      status: "failed",
      skillName: "diagnose-skill-run",
      receiptId: "rx_failed",
      error: "adapter failed",
      events: [event("warning")],
    }),
    hostFixture("result-host-run-escalated", "run_result", {
      status: "escalated",
      skillName: "diagnose-skill-run",
      receiptId: "rx_escalated",
      error: "needs human review",
      events: [event("step_waiting_resolution")],
    }),
    hostFixture("result-host-run-denied", "run_result", {
      status: "denied",
      skillName: "diagnose-skill-run",
      receiptId: "rx_denied",
      reasons: ["authority denied"],
      events: [event("admitted")],
    }),
  ];
}

function hostStateFixtures(): readonly ContractFixture[] {
  return [
    hostFixture("inspect-host-state-needs-agent", "run_state", {
      status: "needs_agent",
      skillName: "diagnose-skill-run",
      runId: "run_needs_agent",
      requestedPath: "skills/review.md",
      resolvedPath: "/workspace/skills/review.md",
      selectedRunner: "runx",
      requests: [approvalResolutionRequest()],
      stepIds: ["approve"],
      stepLabels: ["Approve write"],
      lineage: lineage(),
    }),
    hostFixture("inspect-host-state-completed", "run_state", terminalState("completed", "verified")),
    hostFixture("inspect-host-state-failed", "run_state", terminalState("failed", "invalid")),
    hostFixture("inspect-host-state-escalated", "run_state", terminalState("escalated", "unverified")),
    hostFixture("inspect-host-state-denied", "run_state", terminalState("denied", "verified")),
  ];
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

function agentActResolutionRequest(): Readonly<Record<string, unknown>> {
  return {
    id: "req_act",
    kind: "agent_act",
    invocation: {
      id: "act_1",
      role: "analyst",
      task: "summarize-receipt",
      envelope: {
        current_context: [],
        historical_context: [],
        inputs: {},
        instructions: "Summarize receipt",
        instructions_digest: sha256Prefixed("Summarize receipt"),
        provenance: [],
        requirements: {
          declaration: {},
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

function terminalState(status: string, verificationStatus: string): Readonly<Record<string, unknown>> {
  return {
    status,
    kind: "harness",
    skillName: "diagnose-skill-run",
    runId: `run_${status}`,
    receiptId: `rx_${status}`,
    verification: {
      status: verificationStatus,
      reason: verificationStatus === "verified" ? undefined : "fixture verification state",
    },
    sourceType: "agent-task",
    startedAt: "2026-04-25T14:00:00Z",
    completedAt: "2026-04-25T14:01:00Z",
    actors: ["agent"],
    artifactTypes: ["receipt"],
    runnerProvider: "runx",
    approval: {
      gateId: "workspace-write",
      gateType: "mutation",
      decision: status === "denied" ? "denied" : "approved",
      reason: status === "denied" ? "authority denied" : undefined,
    },
    lineage: lineage(),
  };
}

function lineage(): Readonly<Record<string, unknown>> {
  return {
    kind: "rerun",
    sourceRunId: "run_source",
    sourceReceiptId: "rx_source",
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
