#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";

import {
  createRegistryTestSigningKey,
  signSingleRegistryVersion,
} from "./lib/registry-test-signing.mjs";

const options = parseArgs(process.argv.slice(2));
const runx = path.resolve(options.runxBin);
const expectedVersion = options.expectedVersion;
const worker = path.join(
  path.dirname(runx),
  process.platform === "win32" ? "runx-js-worker.exe" : "runx-js-worker",
);
const root = mkdtempSync(path.join(os.tmpdir(), "runx-release-candidate-"));
const home = path.join(root, "home");
const receipts = path.join(root, "receipts");
const registry = path.join(root, "registry");
const signingKey = createRegistryTestSigningKey({
  keyId: "runx-release-candidate-key",
  signerId: "runx-release-candidate",
});

try {
  requireFile(runx, "--runx-bin");
  requireFile(worker, "adjacent JavaScript worker");
  if (process.platform !== "win32") {
    chmodSync(runx, 0o755);
    chmodSync(worker, 0o755);
  }

  const version = run(["--version"]).stdout.trim();
  if (expectedVersion && !version.includes(expectedVersion)) {
    throw new Error(`candidate version mismatch: ${version}`);
  }

  writeFileSync(
    path.join(root, ".env"),
    [
      "RELEASE_SMOKE_MARKER=workspace-env",
      "RELEASE_SMOKE_UNDECLARED=must-not-cross",
      "",
    ].join("\n"),
  );
  const nested = proveNestedRegistryExecution();
  const environment = proveDeclaredEnvironment();
  const context = proveOperatorContext();
  const approval = proveSingleApproval();
  const scopes = proveOpaqueScopes();
  const interruption = await proveInterruption();

  process.stdout.write(`${JSON.stringify({
    status: "passed",
    version,
    checks: {
      adjacent_worker: true,
      nested_signed_registry: nested,
      declared_workspace_environment: environment,
      operator_context_digest: context,
      single_approval: approval,
      opaque_scope_passthrough: scopes,
      interruption,
    },
  })}\n`);
} finally {
  rmSync(root, { recursive: true, force: true });
}

function proveNestedRegistryExecution() {
  const child = path.join(root, "registry-child");
  writeSkill(
    child,
    "registry-child",
    `skill: registry-child
version: "1.0.0"
harness:
  cases:
    - name: release-candidate-child
      runner: run
      expect: { status: sealed }
runners:
  run:
    default: true
    steps:
      - id: run
        tool: { module: child.mjs }
        outputs:
          nested: object
        artifacts:
          named_emits: { nested: nested }
`,
  );
  writeFileSync(
    path.join(child, "child.mjs"),
    `export default (_inputs, context) => ({
  nested: {
    message: "registry child",
    frozen: Object.isFrozen(context.environment),
  },
});
`,
  );
  run([
    "registry",
    "publish",
    child,
    "--registry-dir",
    registry,
    "--owner",
    "acme",
    "--version",
    "1.0.0",
    "--json",
  ]);
  signSingleRegistryVersion(registry, signingKey);

  const parent = path.join(root, "registry-parent");
  writeSkill(
    parent,
    "registry-parent",
    `skill: registry-parent
runners:
  run:
    default: true
    result_from: [nested]
    steps:
      - id: nested
        skill: registry:acme/registry-child@1.0.0
`,
  );
  const output = runJson([
    "skill",
    parent,
    "--registry",
    registry,
    "--receipt-dir",
    receipts,
    "--json",
  ]);
  if (output.status !== "sealed") {
    throw new Error(`nested registry run was ${output.status}`);
  }
  const nested = output.result?.nested?.data;
  if (
    nested?.message !== "registry child"
    || nested?.frozen !== true
  ) {
    throw new Error(`nested registry result drifted: ${JSON.stringify(nested)}`);
  }
  return true;
}

function proveDeclaredEnvironment() {
  const skill = path.join(root, "environment-probe");
  writeSkill(
    skill,
    "environment-probe",
    `skill: environment-probe
runners:
  run:
    default: true
    steps:
      - id: run
        tool: { module: environment.mjs, environment: { required: [RELEASE_SMOKE_MARKER] } }
        outputs:
          environment: object
`,
  );
  writeFileSync(
    path.join(skill, "environment.mjs"),
    `export default (_inputs, context) => ({
  environment: {
    marker: context.environment.RELEASE_SMOKE_MARKER,
    frozen: Object.isFrozen(context.environment),
    hasUndeclared: Object.hasOwn(
      context.environment,
      "RELEASE_SMOKE_UNDECLARED",
    ),
    keys: Object.keys(context.environment).sort(),
  },
});
`,
  );
  const output = runJson([
    "skill",
    skill,
    "--receipt-dir",
    receipts,
    "--json",
  ]);
  if (
    output.status !== "sealed"
    || output.result?.environment?.marker !== "workspace-env"
    || output.result?.environment?.frozen !== true
    || output.result?.environment?.hasUndeclared !== false
    || JSON.stringify(output.result?.environment?.keys)
      !== JSON.stringify(["RELEASE_SMOKE_MARKER"])
  ) {
    throw new Error(
      "declared workspace environment did not reach JavaScript intact and exclusively",
    );
  }
  return true;
}

function proveOperatorContext() {
  const skill = path.join(root, "context-probe");
  const tail = "RUNX_RELEASE_CONTEXT_TAIL";
  const contextWitness = "Complete operating context remains available to the agent.\n"
    .repeat(22_000);
  writeSkill(
    skill,
    "context-probe",
    `skill: context-probe
runners:
  inspect:
    default: true
    steps:
      - id: inspect
        agent: { role: operator, task: inspect-context }
        outputs:
          report: object
`,
    [
      "This manual proves that a packaged candidate carries the complete operating",
      "context into an agent act. The final marker must survive without truncation.",
      "",
      contextWitness,
      `## Tail marker`,
      "",
      tail,
    ].join("\n"),
  );
  const result = run(
    ["skill", skill, "--receipt-dir", receipts, "--json"],
    { expectedStatuses: [2] },
  );
  if (preparedRunCount(result.stderr) !== 1) {
    throw new Error("operator context was not printed exactly once");
  }
  const output = parseJson(result.stdout, "operator context");
  const request = output.requests?.[0];
  if (request?.request_digest !== request?.artifact_ref?.digest) {
    throw new Error("agent request digest does not match its artifact");
  }
  const envelope = readArtifact(request?.artifact_ref).invocation?.envelope;
  const instructions = envelope?.instructions;
  if (
    typeof instructions !== "string"
    || Buffer.byteLength(instructions) <= 1024 * 1024
    || !instructions.includes(tail)
  ) {
    throw new Error("agent envelope omitted the complete skill manual");
  }
  const digest = `sha256:${createHash("sha256").update(instructions).digest("hex")}`;
  if (envelope.instructions_sha256 !== digest) {
    throw new Error("agent instructions digest does not match the delivered manual");
  }
  return true;
}

function proveSingleApproval() {
  const skill = path.join(root, "approval-probe");
  writeSkill(
    skill,
    "approval-probe",
    `skill: approval-probe
runners:
  approve:
    default: true
    result_from: [apply]
    steps:
      - id: approve
        ask: { gate: release-candidate.single-approval, reason: Prove one consequential action has one approval owner. }
      - id: apply
        tool: fs.write
        scopes: [fs.write]
        inputs:
          repo_root: .
          path: approval-proof.txt
          contents: one host-attested human approval
    policy:
      guards:
        - step: apply
          value: $steps.approve.approval_decision.data.approved
          equals: true
`,
  );
  const pausedResult = run(
    [
      "skill",
      skill,
      "--receipt-dir",
      receipts,
      "--json",
    ],
    { expectedStatuses: [2] },
  );
  const paused = parseJson(pausedResult.stdout, "approval pause");
  if (paused.requests?.length !== 1 || paused.requests[0]?.kind !== "approval") {
    throw new Error("approval probe did not yield exactly one approval");
  }
  if (preparedRunCount(pausedResult.stderr) !== 1) {
    throw new Error("approval probe did not emit exactly one initial Prepared run");
  }
  const requestId = paused.requests[0].id;
  const agentAnswers = path.join(root, "agent-approval-answers.json");
  writeFileSync(
    agentAnswers,
    `${JSON.stringify({ answers: { [requestId]: { approved: true } } })}\n`,
  );
  const rejectedAgentResult = run(
    [
      "resume",
      paused.run_id,
      agentAnswers,
      "--receipt-dir",
      receipts,
      "--json",
    ],
    { expectedStatuses: [1] },
  );
  if (
    !`${rejectedAgentResult.stdout}\n${rejectedAgentResult.stderr}`.includes(
      "host-attested human",
    )
    || preparedRunCount(rejectedAgentResult.stderr) !== 0
  ) {
    throw new Error("agent-authored answer did not fail closed at the approval gate");
  }
  const answers = path.join(root, "approval-answers.json");
  writeFileSync(
    answers,
    `${JSON.stringify({ approvals: { [requestId]: { approved: true } } })}\n`,
  );
  const resumedResult = run([
    "resume",
    paused.run_id,
    answers,
    "--receipt-dir",
    receipts,
    "--json",
  ]);
  const resumed = parseJson(resumedResult.stdout, "approval resume");
  const diagnostics = readArtifact(resumed.diagnostics_ref);
  const actor = diagnostics.context?.step_outputs?.approve?.approval_decision?.data?.actor;
  const proof = path.join(root, "approval-proof.txt");
  if (
    resumed.status !== "sealed"
    || (resumed.requests?.length ?? 0) !== 0
    || actor !== "human"
    || !existsSync(proof)
    || readFileSync(proof, "utf8") !== "one host-attested human approval"
    || preparedRunCount(resumedResult.stderr) !== 0
  ) {
    throw new Error(
      `approved consequential run did not close through exactly one host-attested human gate: ${
        JSON.stringify({
          status: resumed.status,
          pending_requests: resumed.requests?.length ?? 0,
          actor,
          proof_exists: existsSync(proof),
          proof_contents: existsSync(proof) ? readFileSync(proof, "utf8") : null,
          prepared_runs_on_resume: preparedRunCount(resumedResult.stderr),
        })
      }`,
    );
  }
  return true;
}

function preparedRunCount(stderr) {
  return (stderr.match(/^Prepared run$/gmu) ?? []).length;
}

function proveOpaqueScopes() {
  const skill = path.join(root, "scope-probe");
  const scope = "provider:resource.write/a+b?tenant=one,two";
  writeSkill(
    skill,
    "scope-probe",
    `skill: scope-probe
runners:
  inspect:
    default: true
    result_from: [read]
    steps:
      - id: read
        tool: data.digest
        scopes: ["${scope}"]
`,
  );
  const output = runJson(["skill", "inspect", skill, "--json"]);
  if (!JSON.stringify(output).includes(scope)) {
    throw new Error("inspection changed or dropped an opaque skill scope");
  }
  return true;
}

async function proveInterruption() {
  if (process.platform === "win32") {
    return "covered_by_windows_process_containment";
  }
  const blockingWorker = ["/usr/bin/wc", "/bin/wc"].find((candidate) =>
    existsSync(candidate),
  );
  if (!blockingWorker) {
    throw new Error("interruption probe requires an absolute wc executable");
  }
  const skill = path.join(root, "interrupt-probe");
  writeSkill(
    skill,
    "interrupt-probe",
    `skill: interrupt-probe
runners:
  run:
    default: true
    steps:
      - id: run
        tool: { module: interrupt.mjs, timeout_seconds: 30 }
        outputs:
          completed: boolean
`,
  );
  writeFileSync(path.join(skill, "interrupt.mjs"), "export default () => ({ completed: true });\n");
  const child = spawn(
    runx,
    ["skill", skill, "--receipt-dir", receipts, "--json"],
    {
      cwd: root,
      env: {
        ...isolatedEnvironment(),
        RUNX_JS_WORKER_PATH: blockingWorker,
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let closedResult;
  const closed = new Promise((resolve) => {
    child.once("error", (error) => {
      closedResult = { error };
      resolve(closedResult);
    });
    child.once("close", (code, signal) => {
      closedResult = { code, signal };
      resolve(closedResult);
    });
  });
  await waitForDirectChildProcess(child, blockingWorker, () => closedResult);
  const startedAt = Date.now();
  if (!child.kill("SIGINT")) {
    throw new Error("candidate exited before SIGINT could be delivered");
  }
  let timeout;
  const result = await Promise.race([
    closed,
    new Promise((resolve) => {
      timeout = setTimeout(() => {
        child.kill("SIGKILL");
        resolve({ timeout: true });
      }, 5_000);
    }),
  ]);
  clearTimeout(timeout);
  if (result.timeout) {
    throw new Error("candidate did not stop after SIGINT");
  }
  if (result.error) {
    throw result.error;
  }
  if (Date.now() - startedAt >= 5_000 || result.code !== 130 || result.signal) {
    throw new Error(
      `candidate did not close interruption with exit 130: ${JSON.stringify(result)}`,
    );
  }
  return true;
}

async function waitForDirectChildProcess(child, expectedCommand, closed) {
  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline) {
    if (closed()) {
      const result = closed();
      throw new Error(
        `candidate exited before its JavaScript context became active: ${JSON.stringify(result)}`,
      );
    }
    const processes = spawnSync("ps", ["-axo", "ppid=,pid=,command="], {
      encoding: "utf8",
    });
    if (processes.error) throw processes.error;
    if (processes.status !== 0) {
      throw new Error(
        `ps failed while observing the active candidate: ${processes.stderr}`,
      );
    }
    if (
      processes.stdout.split("\n").some((line) => {
        const [parent, _processId, ...command] = line.trim().split(/\s+/u);
        return Number(parent) === child.pid && command.join(" ").includes(expectedCommand);
      })
    ) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  child.kill("SIGKILL");
  throw new Error("candidate did not start its JavaScript worker before SIGINT");
}

function writeSkill(directory, name, profile, manualBody = "") {
  mkdirSync(directory, { recursive: true });
  writeFileSync(
    path.join(directory, "SKILL.md"),
    `---
name: ${name}
description: Release-candidate integration probe.
---

# ${name}

This package exercises a bounded release-candidate invariant.

${manualBody}
`,
  );
  writeFileSync(path.join(directory, "run.yaml"), profile);
}

function runJson(args, expectedStatuses = [0]) {
  const result = run(args, { expectedStatuses });
  return parseJson(result.stdout, args.join(" "));
}

function run(args, { expectedStatuses = [0] } = {}) {
  const result = spawnSync(runx, args, {
    cwd: root,
    env: isolatedEnvironment(),
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    timeout: 60_000,
  });
  if (result.error) throw result.error;
  if (!expectedStatuses.includes(result.status)) {
    throw new Error(
      `candidate ${args.join(" ")} exited ${result.status}\n${result.stderr}\n${result.stdout}`,
    );
  }
  return result;
}

function isolatedEnvironment() {
  return {
    ...Object.fromEntries(
      Object.entries(process.env).filter(([name]) => !name.startsWith("RUNX_")),
    ),
    INIT_CWD: root,
    RUNX_HOME: home,
    RUNX_REGISTRY_DIR: registry,
    RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID: signingKey.keyId,
    RUNX_REGISTRY_MANIFEST_TRUST_KEY_BASE64: signingKey.publicKeyBase64,
    RUNX_REGISTRY_MANIFEST_TRUST_OWNER: "acme",
  };
}

function parseJson(value, context) {
  try {
    return JSON.parse(value);
  } catch {
    throw new Error(`${context} did not return JSON: ${value}`);
  }
}

function readArtifact(reference) {
  if (reference?.schema !== "runx.project_artifact_ref.v1") {
    throw new Error("candidate omitted the expected project artifact reference");
  }
  const bytes = readFileSync(path.resolve(root, reference.path));
  const digest = `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
  if (bytes.length !== reference.bytes || digest !== reference.digest) {
    throw new Error("candidate artifact content does not match its reference");
  }
  return parseJson(bytes.toString("utf8"), "candidate artifact");
}

function requireFile(file, label) {
  if (!existsSync(file)) throw new Error(`${label} is missing: ${file}`);
}

function parseArgs(argv) {
  let runxBin = "";
  let expectedVersion = "";
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--runx-bin") {
      runxBin = argv[index + 1] ?? "";
      index += 1;
    } else if (argument === "--expected-version") {
      expectedVersion = argv[index + 1] ?? "";
      index += 1;
    } else {
      throw new Error(`unknown argument: ${argument}`);
    }
  }
  if (!runxBin) throw new Error("--runx-bin is required");
  return { runxBin, expectedVersion };
}
