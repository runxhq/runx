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
const governanceControlRoot = path.join(fixtureRoot, "governance-control");

interface ContractFixture {
  readonly name: string;
  readonly scope: ContractScope;
  readonly description: string;
  readonly input?: unknown;
  readonly fixture_kind?: ContractFixtureKind;
  readonly expected: unknown;
}

interface ActAssignmentFixtureInput {
  readonly skill_ref: string;
  readonly runner: string;
  readonly source_ref?: string;
  readonly requested_at: string;
  readonly host: {
    readonly kind: "cli" | "api" | "comment" | "system";
    readonly trigger_ref?: string;
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
  readonly content_hash: string;
}

type JsonRecord = Readonly<Record<string, unknown>>;
type HostFixtureKind =
  | "event"
  | "resolution_request"
  | "resolution_response"
  | "run_result"
  | "run_state";
type GovernanceControlFixtureKind = "governance_control_set";
type ContractFixtureKind = HostFixtureKind | GovernanceControlFixtureKind;
type ContractScope =
  | "act-assignment"
  | "governance-control"
  | "host-protocol"
  | "paid-invocation"
  | "principal-id"
  | "receipt-composition"
  | "x402-v2";

const selectedScope = scopeArg();
const check = process.argv.includes("--check");

if (
  selectedScope !== undefined
  && selectedScope !== "act-assignment"
  && selectedScope !== "governance-control"
  && selectedScope !== "host-protocol"
  && selectedScope !== "paid-invocation"
  && selectedScope !== "principal-id"
  && selectedScope !== "receipt-composition"
  && selectedScope !== "x402-v2"
) {
  throw new Error(`unsupported contract fixture scope: ${selectedScope}`);
}

if (selectedScope === undefined || selectedScope === "act-assignment") {
  await writeFixtures(buildActAssignmentFixtures(), actAssignmentRoot);
}
if (selectedScope === undefined || selectedScope === "governance-control") {
  await writeFixtures(buildGovernanceControlFixtures(), governanceControlRoot);
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
if (selectedScope === undefined || selectedScope === "paid-invocation") {
  runPaidInvocationFixtures();
}
if (selectedScope === undefined || selectedScope === "principal-id") {
  runPrincipalIdFixtures();
}
if (selectedScope === undefined || selectedScope === "receipt-composition") {
  runReceiptCompositionFixtures();
}
if (selectedScope === undefined || selectedScope === "x402-v2") {
  runX402Fixtures();
}

function runReceiptCompositionFixtures(): void {
  const outputRoot = path.join(fixtureRoot, "receipt-composition");
  const configured = process.env.RUNX_RECEIPT_COMPOSITION_FIXTURES_BIN;
  const command = configured || (process.platform === "win32" ? "cargo.exe" : "cargo");
  const args = configured
    ? ["--out", outputRoot]
    : [
        "run",
        "--quiet",
        "--manifest-path",
        path.join(workspaceRoot, "crates", "Cargo.toml"),
        "-p",
        "runx-receipts",
        "--example",
        "runx-receipt-composition-fixtures",
        "--",
        "--out",
        outputRoot,
      ];
  if (check) args.push("--check");
  const result = spawnSync(command, args, { cwd: workspaceRoot, env: process.env, stdio: "inherit" });
  if (result.error) throw result.error;
  if ((result.status ?? 1) !== 0) process.exit(result.status ?? 1);
}

function runPaidInvocationFixtures(): void {
  const outputRoot = path.join(fixtureRoot, "paid-invocation");
  const fingerprintOracle = path.join(
    fixtureRoot,
    "canonical-json",
    "runx-paid-invocation-request-fingerprint-v1.oracles.json",
  );
  const configured = process.env.RUNX_PAID_INVOCATION_FIXTURES_BIN;
  const command = configured || (process.platform === "win32" ? "cargo.exe" : "cargo");
  const args = configured
    ? ["--out", outputRoot, "--schema-dir", path.join(workspaceRoot, "schemas"), "--packet-dir", path.join(workspaceRoot, "dist", "packets"), "--oracle-out", fingerprintOracle]
    : [
        "run",
        "--quiet",
        "--manifest-path",
        path.join(workspaceRoot, "crates", "Cargo.toml"),
        "-p",
        "runx-contracts",
        "--bin",
        "runx-paid-invocation-fixtures",
        "--",
        "--out",
        outputRoot,
        "--schema-dir",
        path.join(workspaceRoot, "schemas"),
        "--packet-dir",
        path.join(workspaceRoot, "dist", "packets"),
        "--oracle-out",
        fingerprintOracle,
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
        "--bin",
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
        "--bin",
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
        skill_ref: "outreach",
        runner: "rerun",
        source_ref: "github://sourcey/sourcey.com/issues/3",
        requested_at: "2026-04-25T14:00:00Z",
        host: {
          kind: "comment",
          trigger_ref: "https://github.com/sourcey/sourcey.com/issues/3#issuecomment-1",
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
        skill_ref: "docs.refresh",
        runner: "runx",
        source_ref: "local://workspace",
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
        skill_ref: "system.audit",
        runner: "system",
        requested_at: "2026-04-25T14:02:00Z",
        host: {
          kind: "system",
        },
      },
    ),
    actAssignmentFixture(
      "host-normalization",
      "Documents buildActAssignment host normalization: empty trigger_ref, scope_set, and actor fields are omitted.",
      {
        skill_ref: "host.normalize",
        runner: "runx",
        requested_at: "2026-04-25T14:03:00Z",
        host: {
          kind: "api",
          trigger_ref: "",
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
    skillRef: input.skill_ref,
    runner: input.runner,
    sourceRef: input.source_ref,
    requestedAt: input.requested_at,
    hostKind: input.host.kind,
    triggerRef: input.host.trigger_ref,
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
        skillRef: input.skill_ref,
        runner: input.runner,
        sourceRef: input.source_ref,
        inputOverrides: input.input_overrides,
      }),
      trigger_key: deriveActAssignmentTriggerKey({
        hostKind: input.host.kind,
        triggerRef: input.host.trigger_ref,
      }),
      content_hash: deriveActAssignmentContentHash(input.input_overrides),
    },
  };
}

function buildActAssignment(options: {
  readonly skillRef: string;
  readonly runner: string;
  readonly sourceRef?: string;
  readonly requestedAt?: string;
  readonly hostKind?: ActAssignmentHostContract["kind"];
  readonly triggerRef?: string;
  readonly scopeSet?: readonly string[];
  readonly actor?: ActAssignmentActorContract;
  readonly inputOverrides?: JsonRecord;
}): ActAssignmentContract {
  const inputOverrides = normalizeActAssignmentRecord(options.inputOverrides);
  const host = normalizeActAssignmentHost({
    kind: options.hostKind ?? "cli",
    trigger_ref: options.triggerRef,
    scope_set: options.scopeSet,
    actor: options.actor,
  });
  const sourceRef = normalizeNonEmptyString(options.sourceRef);
  const requestedAt = normalizeNonEmptyString(options.requestedAt) ?? new Date().toISOString();

  return validateActAssignmentContract(pruneUndefined({
    schema: "runx.act_assignment.v1",
    skill_ref: options.skillRef,
    runner: options.runner,
    source_ref: sourceRef,
    requested_at: requestedAt,
    host,
    input_overrides: inputOverrides,
    idempotency: {
      algorithm: "sha256",
      intent_key: deriveActAssignmentIntentKey({
        skillRef: options.skillRef,
        runner: options.runner,
        sourceRef,
        inputOverrides,
      }),
      trigger_key: deriveActAssignmentTriggerKey({
        hostKind: host.kind,
        triggerRef: host.trigger_ref,
      }),
      content_hash: deriveActAssignmentContentHash(inputOverrides),
    },
  }));
}

function deriveActAssignmentIntentKey(options: {
  readonly skillRef: string;
  readonly runner: string;
  readonly sourceRef?: string;
  readonly inputOverrides?: JsonRecord;
}): string {
  return canonicalSha256({
    skill_ref: options.skillRef,
    runner: options.runner,
    source_ref: normalizeNonEmptyString(options.sourceRef),
    input_overrides: normalizeActAssignmentRecord(options.inputOverrides),
  });
}

function deriveActAssignmentTriggerKey(options: {
  readonly hostKind: ActAssignmentHostContract["kind"];
  readonly triggerRef?: string;
}): string | undefined {
  const triggerRef = normalizeNonEmptyString(options.triggerRef);
  if (!triggerRef) {
    return undefined;
  }
  return canonicalSha256({
    host_kind: options.hostKind,
    trigger_ref: triggerRef,
  });
}

function deriveActAssignmentContentHash(inputOverrides?: JsonRecord): string {
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
  readonly trigger_ref?: string;
  readonly scope_set?: readonly string[];
  readonly actor?: ActAssignmentActorContract;
}): ActAssignmentHostContract {
  const actor = normalizeActAssignmentActor(value.actor);
  const scopeSet = normalizeStringArray(value.scope_set);
  return pruneUndefined({
    kind: value.kind,
    trigger_ref: normalizeNonEmptyString(value.trigger_ref),
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

function buildGovernanceControlFixtures(): readonly ContractFixture[] {
  const targetRef = reference("target", "runx:target:public-site");
  const opportunityRef = reference("opportunity", "runx:opportunity:docs-gap");
  const selectionCycleRef = reference("selection_cycle", "runx:selection_cycle:cycle_1");
  const selectionRef = reference("selection", "runx:selection:sel_1");
  const decisionRef = reference("decision", "runx:decision:dec_1");
  const receiptRef = reference("receipt", "runx:receipt:rcpt_1");
  const verificationRef = reference("verification", "runx:verification:ver_1");
  const evidenceRef = reference("artifact", "runx:artifact:evidence_1");
  const redactionPolicyRef = reference("redaction_policy", "runx:redaction_policy:public");
  const sourceRef = reference("signal", "runx:signal:sig_1");
  const fingerprint = {
    algorithm: "sha256",
    canonicalization: "runx.fingerprint.c14n.v1",
    derived_from: [sourceRef],
    value: "sha256:target",
  };
  const actRef = {
    act_id: "act_publish_feed",
    receipt_ref: receiptRef,
  };

  return [{
    name: "public-feed-proof",
    scope: "governance-control",
    description: "Governance control fixture covering target, opportunity, selection, reflection, and feed-entry proof bindings.",
    fixture_kind: "governance_control_set",
    expected: {
      feed_entry: {
        act_refs: [actRef],
        artifact_refs: [evidenceRef],
        decision_refs: [decisionRef],
        evidence_refs: [evidenceRef],
        feed_entry_id: "feed_1",
        receipt_refs: [receiptRef],
        opportunity_ref: opportunityRef,
        public_at: "2026-05-18T00:07:00Z",
        redaction_policy_ref: redactionPolicyRef,
        redaction_refs: [reference("redaction_policy", "runx:redaction:redaction_1")],
        schema: "runx.feed_entry.v1",
        selection_ref: selectionRef,
        summary: "The public entry cites a sealed receipt, contained act, decision, verification, and redaction policy.",
        target_ref: targetRef,
        title: "Runx published a proof-bound entry",
        verification_refs: [verificationRef],
      },
      opportunity: {
        discovered_at: "2026-05-18T00:01:00Z",
        evidence_refs: [evidenceRef],
        fingerprint,
        freshness_expires_at: "2026-05-19T00:00:00Z",
        opportunity_id: "opp_1",
        proposed_form: "observation",
        risk_score: 12,
        schema: "runx.opportunity.v1",
        source_refs: [sourceRef],
        summary: "Publish a clearer proof entry for the selected public surface.",
        target_ref: targetRef,
        value_score: 86,
      },
      reflection_entry: {
        act_refs: [actRef],
        decision_ref: decisionRef,
        evidence_refs: [evidenceRef],
        follow_up_refs: [],
        receipt_refs: [receiptRef],
        lessons: ["Keep feed projections tied to sealed receipts."],
        opportunity_ref: opportunityRef,
        recorded_at: "2026-05-18T00:06:00Z",
        reflection_id: "reflect_1",
        schema: "runx.reflection_entry.v1",
        selection_ref: selectionRef,
        summary: "Public proof entry was useful and low-risk.",
        target_ref: targetRef,
      },
      selection: {
        candidate_refs: [opportunityRef],
        cooldown_until: "2026-05-19T00:00:00Z",
        cycle_ref: selectionCycleRef,
        decision_ref: decisionRef,
        evidence_refs: [evidenceRef],
        opportunity_ref: opportunityRef,
        rank: 1,
        reason: "Highest value public proof candidate inside current authority.",
        schema: "runx.selection.v1",
        score: 91,
        selected: true,
        selected_at: "2026-05-18T00:03:00Z",
        selection_id: "sel_1",
      },
      selection_cycle: {
        chosen_selection_ref: selectionRef,
        closed_at: "2026-05-18T00:04:00Z",
        cycle_id: "cycle_1",
        decision_ref: decisionRef,
        fingerprint,
        receipt_ref: receiptRef,
        input_refs: [sourceRef],
        no_action_closure: null,
        opportunity_refs: [opportunityRef],
        ranked_selection_refs: [selectionRef],
        schema: "runx.selection_cycle.v1",
        started_at: "2026-05-18T00:00:00Z",
        state: "closed",
        target_refs: [targetRef],
      },
      skill_binding: {
        active: true,
        allowed_act_forms: ["observation"],
        authority_refs: [reference("grant", "runx:grant:public_evidence")],
        binding_id: "binding_1",
        created_at: "2026-05-18T00:00:00Z",
        harness_template_ref: reference("harness", "runx:harness_template:public_feed"),
        policy_refs: [redactionPolicyRef],
        schema: "runx.skill_binding.v1",
        scope_family: "publication",
        skill_ref: reference("artifact", "runx:skill:project-feed-entry"),
        updated_at: "2026-05-18T00:01:00Z",
      },
      target: {
        authority_refs: [reference("grant", "runx:grant:public_evidence")],
        cooldown: {
          state: "none",
        },
        created_at: "2026-05-18T00:00:00Z",
        fingerprint,
        lifecycle_state: "active",
        schema: "runx.target.v1",
        target_id: "target_1",
        target_ref: targetRef,
        title: "Runx public proof surface",
        updated_at: "2026-05-18T00:01:00Z",
        verification_recipe_refs: [reference("verification", "runx:verification_recipe:public_feed")],
      },
      target_transition_entry: {
        decision_ref: decisionRef,
        entry_id: "tte_1",
        from_state: "eligible",
        receipt_ref: receiptRef,
        reason_code: "selected",
        recorded_at: "2026-05-18T00:03:30Z",
        schema: "runx.target_transition_entry.v1",
        source_refs: [sourceRef],
        summary: "Target entered the active selector set.",
        target_ref: targetRef,
        to_state: "active",
      },
      thesis_assessment: {
        assessed_at: "2026-05-18T00:02:00Z",
        assessment_id: "assess_1",
        authority_cost: "low",
        evidence_refs: [evidenceRef],
        opportunity_ref: opportunityRef,
        proof_strength: "strong",
        rationale: "The entry improves public proof without broadening authority.",
        rubric_refs: [reference("external_url", "https://runx.ai/docs")],
        schema: "runx.thesis_assessment.v1",
        score: 91,
        target_ref: targetRef,
        thesis_ref: reference("external_url", "https://runx.ai/docs"),
      },
    },
  }];
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
        instructions_sha256: sha256Prefixed("Summarize receipt"),
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
    disposition: status,
    outcomeState: status,
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
