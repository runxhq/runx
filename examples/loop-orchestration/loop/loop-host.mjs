#!/usr/bin/env node

import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const ROOT = resolve(new URL("../../..", import.meta.url).pathname);
const EXAMPLE = resolve(new URL("..", import.meta.url).pathname);
const DEFAULT_SEED = "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=";

function findRunx() {
  const candidate = process.env.RUNX_BIN || join(ROOT, "crates/target/debug/runx");
  const probe = spawnSync(candidate, ["--version"], { stdio: "ignore" });
  if (probe.status === 0) return candidate;

  const pathProbe = spawnSync("runx", ["--version"], { stdio: "ignore" });
  if (pathProbe.status === 0) return "runx";

  throw new Error("runx binary not found; build crates/runx-cli or set RUNX_BIN");
}

function runCommand(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: ROOT,
    env: {
      ...process.env,
      RUNX_RECEIPT_SIGN_KID: process.env.RUNX_RECEIPT_SIGN_KID || "hosted-receipt-3097e2dee2cb4a34",
      RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64:
        process.env.RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64 || DEFAULT_SEED,
      RUNX_RECEIPT_SIGN_ISSUER_TYPE: process.env.RUNX_RECEIPT_SIGN_ISSUER_TYPE || "hosted",
    },
    encoding: "utf8",
  });
  if (!options.allowExitCodes?.includes(result.status) && result.status !== 0) {
    process.stderr.write(result.stderr || result.stdout);
    throw new Error(`${command} ${args.join(" ")} exited ${result.status}`);
  }
  return result;
}

function runTurn({ runx, receiptDir, loopId, turnIndex, maxTurns, objective, previousSummary, scenario, requestedTool }) {
  const result = runCommand(runx, [
    "skill",
    EXAMPLE,
    "turn",
    "--receipt-dir",
    receiptDir,
    "--json",
    "--input",
    `loop_id=${loopId}`,
    "--input",
    `turn_index=${turnIndex}`,
    "--input",
    `max_turns=${maxTurns}`,
    "--input",
    `objective=${objective}`,
    "--input",
    `previous_summary=${previousSummary}`,
    "--input",
    `scenario=${scenario}`,
    "--input",
    `requested_tool=${requestedTool}`,
  ]);
  return JSON.parse(result.stdout);
}

function runContextPreview({ runx, receiptDir }) {
  const result = runCommand(
    runx,
    [
      "skill",
      EXAMPLE,
      "context-gate",
      "--receipt-dir",
      receiptDir,
      "--json",
      "--input",
      "objective=Review whether the next turn may continue.",
    ],
    { allowExitCodes: [2] },
  );
  return JSON.parse(result.stdout);
}

function resultFrom(envelope) {
  if (!envelope || envelope.status !== "sealed" || !envelope.result) {
    throw new Error(`expected sealed runx skill result, got ${JSON.stringify(envelope)}`);
  }
  return envelope.result;
}

function printTurn(result) {
  const payload = resultFrom(result);
  console.log(
    [
      `turn ${payload.turn_index}`,
      `run=${result.run_id}`,
      `receipt=${result.receipt_id}`,
      `decision=${payload.decision}`,
      `reason=${payload.reason_code}`,
      `next=${payload.next_turn_allowed ? "yes" : "no"}`,
    ].join(" | "),
  );
  console.log(`  ${payload.next_turn_reason}`);
  return payload;
}

function runSuccessLoop(runx, receiptDir) {
  console.log("\n1) success path: bounded loop over governed turns");
  let previousSummary = "No prior turn.";
  for (let turnIndex = 1; turnIndex <= 3; turnIndex += 1) {
    const result = runTurn({
      runx,
      receiptDir,
      loopId: "loop-demo-success",
      turnIndex,
      maxTurns: 3,
      objective: "Draft, review, and ratify one tiny safe change.",
      previousSummary,
      scenario: "success",
      requestedTool: "fs.read",
    });
    const payload = printTurn(result);
    previousSummary = payload.projection_update.latest_summary;
    if (!payload.next_turn_allowed) return;
  }
}

function runRefusal(runx, receiptDir) {
  console.log("\n2) refusal path: loop policy stops an undeclared tool");
  const result = runTurn({
    runx,
    receiptDir,
    loopId: "loop-demo-refusal",
    turnIndex: 1,
    maxTurns: 3,
    objective: "Try to mutate files without explicit authority.",
    previousSummary: "No prior turn.",
    scenario: "refusal",
    requestedTool: "fs.write",
  });
  printTurn(result);
}

function runContextGate(runx, receiptDir) {
  console.log("\n3) context gate: agent turn pauses with bounded advisory context");
  const preview = runContextPreview({ runx, receiptDir });
  const request = preview.requests?.[0];
  const invocation = request?.invocation;
  const context = invocation?.envelope?.current_context || invocation?.current_context || [];
  const allowedTools = invocation?.envelope?.allowed_tools || invocation?.allowed_tools || [];
  console.log(`needs_agent run=${preview.run_id} request=${request?.id}`);
  console.log(`  allowed_tools=${JSON.stringify(allowedTools)}`);
  console.log(`  context_artifacts=${context.length}`);
  for (const entry of context) {
    const data = entry.data || {};
    const name = data.name || entry.name || "context";
    const ref = data.ref || entry.ref || entry.source_ref || entry.id || "context";
    const digest = data.sha256 || entry.sha256 || entry.meta?.hash || "sha256:unknown";
    const boundary =
      data.security_boundary ||
      entry.security_boundary ||
      entry.boundary ||
      "untrusted-agent-context";
    console.log(`  - ${name} ref=${ref} digest=${digest} (${boundary})`);
  }
}

function main() {
  const runx = findRunx();
  const receiptDir = process.env.RUNX_LOOP_RECEIPT_DIR || mkdtempSync(join(tmpdir(), "runx-loop-"));
  console.log("runx loop orchestration example");
  console.log(`binary: ${runx}`);
  console.log(`receipts: ${receiptDir}`);

  runSuccessLoop(runx, receiptDir);
  runRefusal(runx, receiptDir);
  runContextGate(runx, receiptDir);

  console.log("\nDone. Inspect the receipt store or rerun with:");
  console.log(`  RUNX_LOOP_RECEIPT_DIR=${receiptDir} sh examples/loop-orchestration/run.sh`);
}

main();
