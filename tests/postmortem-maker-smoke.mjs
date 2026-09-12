import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { composePostmortem } from "../skills/postmortem-maker/steps/compose.mjs";
import { deliverPostmortem } from "../skills/postmortem-maker/steps/deliver.mjs";
import { readIncident } from "../skills/postmortem-maker/steps/read_incident.mjs";
import { readOutbox } from "../skills/postmortem-maker/steps/read_outbox.mjs";
import { verifyPostmortem } from "../skills/postmortem-maker/steps/verify.mjs";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const packageRoot = path.join(repoRoot, "skills", "postmortem-maker");

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

async function runStep(step, envelope, stateDir) {
  const handlers = {
    "steps/read_incident.mjs": readIncident,
    "steps/read_outbox.mjs": readOutbox,
    "steps/compose.mjs": composePostmortem,
    "steps/deliver.mjs": deliverPostmortem,
    "steps/verify.mjs": verifyPostmortem,
  };
  const handler = handlers[step];
  if (!handler) throw new Error(`no local handler registered for ${step}`);
  const previousStateDir = process.env.RUNX_STATE_DIR;
  process.env.RUNX_STATE_DIR = stateDir;
  try {
    return await handler(envelope);
  } finally {
    if (previousStateDir === undefined) delete process.env.RUNX_STATE_DIR;
    else process.env.RUNX_STATE_DIR = previousStateDir;
  }
}

function assertCitations(incident, postmortem) {
  const eventById = new Map(incident.events.map((event) => [event.id, event]));
  for (const entry of postmortem.timeline) {
    const source = eventById.get(entry.evidence.event_id);
    assert.ok(source, `timeline citation ${entry.evidence.event_id} exists`);
    assert.equal(entry.evidence.quote, source.text, "timeline quote is verbatim");
    assert.equal(entry.statement, source.text, "timeline statement stays source-bound");
  }
  for (const citation of postmortem.root_cause.citations) {
    const source = eventById.get(citation.event_id);
    assert.ok(source, `root-cause citation ${citation.event_id} exists`);
    assert.equal(citation.quote, source.text, "root-cause quote is verbatim");
  }
}

async function executeCase(caseDirectory, stateDir) {
  const input = readJson(path.join(caseDirectory, "input.json"));
  const incidentOutput = await runStep("steps/read_incident.mjs", { inputs: { incident_source: input.incident_source } }, stateDir);
  const outboxOutput = await runStep("steps/read_outbox.mjs", { inputs: { publish_target: input.publish_target } }, stateDir);
  const compose = await runStep(
    "steps/compose.mjs",
    {
      inputs: {
        postmortem_policy: input.postmortem_policy,
        publish_target: input.publish_target,
      },
      context: {
        incident: incidentOutput.incident,
        outbox: outboxOutput.outbox,
      },
    },
    stateDir,
  );
  let delivery = null;
  if (compose.publishable) {
    delivery = await runStep(
      "steps/deliver.mjs",
      {
        inputs: { publish_target: input.publish_target },
        context: {
          send_plan: compose.send_plan,
          message: compose.message,
          idempotency_key: compose.idempotency_key,
          expected_version: compose.expected_version,
          content_digest: compose.content_digest,
          postmortem_digest: compose.postmortem_digest,
        },
      },
      stateDir,
    );
  }
  const verified = await runStep(
    "steps/verify.mjs",
    {
      inputs: { publish_target: input.publish_target },
      context: {
        incident: incidentOutput.incident,
        postmortem: compose.postmortem,
        unknowns: compose.unknowns,
        action_items: compose.action_items,
        publishable: compose.publishable,
        send_plan: compose.send_plan,
        content_digest: compose.content_digest,
        postmortem_digest: compose.postmortem_digest,
        idempotency_key: compose.idempotency_key,
        expected_version: compose.expected_version,
        delivery_result: delivery?.delivery_result ?? null,
      },
    },
    stateDir,
  );
  assertCitations(incidentOutput.incident, compose.postmortem);
  return { input, incident: incidentOutput.incident, outbox: outboxOutput.outbox, compose, delivery, verified };
}

function assertExpected(result, expected) {
  assert.equal(result.compose.publishable, expected.publishable);
  assert.equal(result.compose.root_cause_status, expected.root_cause_status);
  if (expected.timeline_count !== undefined) assert.equal(result.compose.timeline_count, expected.timeline_count);
  if (expected.unknowns_at_least !== undefined) assert.ok(result.compose.unknowns.length >= expected.unknowns_at_least);
  if (expected.publish_result_status) assert.equal(result.verified.publish_result.status, expected.publish_result_status);
  if (expected.publish_result === null) assert.equal(result.verified.publish_result, null);
  for (const [key, value] of Object.entries(expected.readback ?? {})) assert.equal(result.verified.readback[key], value, `readback.${key}`);
}

function assertContractFiles() {
  const skill = fs.readFileSync(path.join(packageRoot, "SKILL.md"), "utf8");
  const profile = fs.readFileSync(path.join(packageRoot, "X.yaml"), "utf8");
  assert.match(skill, /^---\nname: postmortem-maker\n/m);
  assert.match(skill, /allow_publish/);
  assert.match(profile, /^skill: postmortem-maker\nversion: "0\.1\.0"/m);
  assert.match(profile, /name: consistent-incident-published/);
  assert.match(profile, /name: conflicting-evidence-withheld/);
  assert.match(profile, /type: graph/);
  assert.match(profile, /type: cli-tool/);
}

async function assertHostileFixtureRefused(stateDir) {
  await assert.rejects(
    () => runStep("steps/read_incident.mjs", { inputs: { incident_source: { kind: "fixture", ref: "fixtures/../SKILL.md" } } }, stateDir),
    /cannot contain \.\./,
    "fixture traversal must be refused",
  );
}

async function assertSourceAllowlistRefused(stateDir) {
  await assert.rejects(
    () => runStep("steps/read_incident.mjs", { inputs: { incident_source: { kind: "thread_url", url: "http://127.0.0.1:8080/internal" } } }, stateDir),
    /must use HTTPS/,
    "non-HTTPS sources must be refused",
  );
  await assert.rejects(
    () => runStep("steps/read_incident.mjs", { inputs: { incident_source: { kind: "thread_url", url: "https://example.com/incident/1" } } }, stateDir),
    /public GitHub issue URLs only/,
    "non-GitHub sources must be refused before fetch",
  );
}

async function assertLongEventBodyAccepted(stateDir) {
  const longText = "x".repeat(2_500);
  const result = await runStep(
    "steps/read_incident.mjs",
    {
      inputs: {
        incident_source: {
          kind: "inline",
          ref: "inline://long-event-body",
          thread: {
            title: "Long event body",
            events: [{ id: "event-1", author: "tester", at: "2026-01-01T00:00:00Z", text: longText }],
          },
        },
      },
    },
    stateDir,
  );
  assert.equal(result.incident.events.length, 1);
  assert.equal(result.incident.events[0].text, longText, "event bodies above the field-validation limit remain readable");
}

const stateDir = fs.mkdtempSync(path.join(os.tmpdir(), "postmortem-maker-smoke-"));
try {
  await (async () => {
  assertContractFiles();
  const consistentDirectory = path.join(packageRoot, "fixtures", "consistent-incident");
  const conflictingDirectory = path.join(packageRoot, "fixtures", "conflicting-incident");
  const consistent = await executeCase(consistentDirectory, stateDir);
  const consistentExpected = readJson(path.join(consistentDirectory, "expected.json")).expected;
  assertExpected(consistent, consistentExpected);
  assert.equal(consistent.delivery.delivery_result.replayed, false);

  const replay = await executeCase(consistentDirectory, stateDir);
  assert.equal(replay.delivery.delivery_result.replayed, true, "same content is idempotent");
  assert.equal(replay.delivery.delivery_result.before_version, 1, "replay keeps outbox version");
  assert.equal(replay.delivery.delivery_result.after_version, 1, "replay does not append twice");

  const conflicting = await executeCase(conflictingDirectory, stateDir);
  const conflictingExpected = readJson(path.join(conflictingDirectory, "expected.json")).expected;
  assertExpected(conflicting, conflictingExpected);
  await assertHostileFixtureRefused(stateDir);
  await assertSourceAllowlistRefused(stateDir);
  await assertLongEventBodyAccepted(stateDir);

  process.stdout.write(`${JSON.stringify({
    status: "passed",
    cases: [
      { name: "consistent-incident-published", status: "sealed", publishable: consistent.compose.publishable, delivery: consistent.verified.publish_result?.status ?? null },
      { name: "conflicting-evidence-withheld", status: "sealed", publishable: conflicting.compose.publishable, unknowns: conflicting.compose.unknowns.length, delivery: null },
      { name: "idempotent-replay", status: "passed", replayed: replay.delivery.delivery_result.replayed },
      { name: "fixture-traversal-refused", status: "refused" },
      { name: "source-allowlist-refused", status: "refused" },
      { name: "long-event-body-accepted", status: "passed" },
    ],
  })}\n`);
  })();
} finally {
  fs.rmSync(stateDir, { recursive: true, force: true });
}
