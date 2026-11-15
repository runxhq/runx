#!/usr/bin/env node

import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const ROOT = resolve(new URL("../..", import.meta.url).pathname);
// The package this host drives, beside it: the host is not part of it.
const PACKAGE = resolve(new URL("skill", import.meta.url).pathname);

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
    env: process.env,
    encoding: "utf8",
  });
  if (!options.allowExitCodes?.includes(result.status) && result.status !== 0) {
    process.stderr.write(result.stderr || result.stdout);
    throw new Error(`${command} ${args.join(" ")} exited ${result.status}`);
  }
  return result;
}

// One iteration of the loop is one governed run of the package's decide method.
function runIteration({ runx, receiptDir, loopId, iteration, maxIterations, objective, previousSummary, scenario, requestedTool }) {
  const result = runCommand(runx, [
    "skill",
    `${PACKAGE}#decide`,
    "--receipt-dir",
    receiptDir,
    "--json",
    "--input",
    `loop_id=${loopId}`,
    "--input",
    `iteration=${iteration}`,
    "--input",
    `max_iterations=${maxIterations}`,
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
      `${PACKAGE}#context-gate`,
      "--receipt-dir",
      receiptDir,
      "--json",
      "--input",
      "objective=Review whether the next iteration may continue.",
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

function printIteration(result) {
  const payload = resultFrom(result);
  console.log(
    [
      `iteration ${payload.iteration}`,
      `run=${result.run_id}`,
      `receipt=${result.receipt_id}`,
      `decision=${payload.decision}`,
      `reason=${payload.reason_code}`,
      `next=${payload.next_iteration_allowed ? "yes" : "no"}`,
    ].join(" | "),
  );
  console.log(`  ${payload.next_iteration_reason}`);
  return payload;
}

function runSuccessLoop(runx, receiptDir) {
  console.log("\n1) success path: a bounded loop over governed runs");
  let previousSummary = "No earlier iteration.";
  for (let iteration = 1; iteration <= 3; iteration += 1) {
    const result = runIteration({
      runx,
      receiptDir,
      loopId: "loop-demo-success",
      iteration,
      maxIterations: 3,
      objective: "Draft, review, and ratify one tiny safe change.",
      previousSummary,
      scenario: "success",
      requestedTool: "fs.read",
    });
    const payload = printIteration(result);
    previousSummary = payload.projection_update.latest_summary;
    if (!payload.next_iteration_allowed) return;
  }
}

function runRefusal(runx, receiptDir) {
  console.log("\n2) refusal path: loop policy stops an undeclared tool");
  const result = runIteration({
    runx,
    receiptDir,
    loopId: "loop-demo-refusal",
    iteration: 1,
    maxIterations: 3,
    objective: "Try to mutate files without explicit authority.",
    previousSummary: "No earlier iteration.",
    scenario: "refusal",
    requestedTool: "fs.write",
  });
  printIteration(result);
}

// A paused turn's request names its allowance, and its full view is the
// artifact the request points to: what a host or an agent harness is given.
function runContextGate(runx, receiptDir) {
  console.log("\n3) context gate: agent turn pauses with bounded advisory context");
  const preview = runContextPreview({ runx, receiptDir });
  const [request] = preview.requests ?? [];
  if (preview.status !== "needs_agent" || request?.kind !== "turn") {
    throw new Error(`expected a run waiting on one turn, got ${JSON.stringify(preview)}`);
  }
  const view = JSON.parse(readFileSync(request.artifact.path, "utf8"));
  const context = view.invocation.envelope.current_context;
  console.log(`needs_agent run=${preview.run_id} request=${request.id}`);
  console.log(`  tools=${JSON.stringify(request.allowance.tools)}`);
  console.log(`  context_entries=${context.length}`);
  for (const entry of context) {
    console.log(
      `  - ${entry.data.name} skill=${entry.data.skill} digest=${entry.origin.digest} trust=${entry.labels.trust} (${entry.data.security_boundary})`,
    );
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
