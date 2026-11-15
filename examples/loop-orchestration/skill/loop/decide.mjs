#!/usr/bin/env node

import { readFileSync } from "node:fs";

const ALLOWED_TOOLS = Object.freeze(["fs.read"]);
const DECISIONS = Object.freeze(["continue", "done", "needs_human", "refused"]);

function readInputs() {
  if (process.env.RUNX_INPUTS_PATH) {
    return JSON.parse(readFileSync(process.env.RUNX_INPUTS_PATH, "utf8"));
  }
  return JSON.parse(process.env.RUNX_INPUTS_JSON || "{}");
}

function integerInput(inputs, key, fallback) {
  const value = inputs[key] ?? fallback;
  const number = Number(value);
  if (!Number.isInteger(number) || number < 1) {
    throw new Error(`${key} must be a positive integer`);
  }
  return number;
}

function stringInput(inputs, key, fallback = "") {
  const value = inputs[key] ?? fallback;
  if (typeof value !== "string") {
    return String(value);
  }
  return value;
}

function decide({ scenario, iteration, maxIterations, requestedTool }) {
  if (iteration > maxIterations) {
    return {
      decision: "refused",
      reason: "iteration_budget_exhausted",
      nextIterationAllowed: false,
      nextIterationReason: "max_iterations has already been exhausted",
    };
  }

  if (!ALLOWED_TOOLS.includes(requestedTool)) {
    return {
      decision: "refused",
      reason: "tool_not_allowed",
      nextIterationAllowed: false,
      nextIterationReason: `${requestedTool} is not in allowed_tools`,
    };
  }

  if (scenario === "pause") {
    return {
      decision: "needs_human",
      reason: "side_effect_review_required",
      nextIterationAllowed: false,
      nextIterationReason: "human review is required before the next iteration",
    };
  }

  if (iteration >= Math.min(maxIterations, 2)) {
    return {
      decision: "done",
      reason: "fixture_objective_complete",
      nextIterationAllowed: false,
      nextIterationReason: "review accepted the bounded fixture result",
    };
  }

  return {
    decision: "continue",
    reason: "bounded_next_iteration_available",
    nextIterationAllowed: true,
    nextIterationReason: "one more governed run is allowed by policy",
  };
}

function buildDecision(inputs) {
  const loopId = stringInput(inputs, "loop_id", "loop-demo");
  const iteration = integerInput(inputs, "iteration", 1);
  const maxIterations = integerInput(inputs, "max_iterations", 3);
  const objective = stringInput(inputs, "objective", "Demonstrate one governed iteration of a loop.");
  const previousSummary = stringInput(inputs, "previous_summary", "No earlier iteration.");
  const requestedTool = stringInput(inputs, "requested_tool", "fs.read");
  const scenario = stringInput(inputs, "scenario", "success");
  const outcome = decide({ scenario, iteration, maxIterations, requestedTool });

  if (!DECISIONS.includes(outcome.decision)) {
    throw new Error(`invalid fixture decision ${outcome.decision}`);
  }

  return {
    schema: "loop_orchestration.decision.v1",
    loop_id: loopId,
    iteration_id: `${loopId}:iteration-${iteration}`,
    iteration,
    max_iterations: maxIterations,
    objective,
    previous_summary: previousSummary,
    requested_tool: requestedTool,
    allowed_tools: [...ALLOWED_TOOLS],
    decision: outcome.decision,
    reason_code: outcome.reason,
    next_iteration_allowed: outcome.nextIterationAllowed,
    next_iteration_reason: outcome.nextIterationReason,
    projection_update: {
      latest_summary:
        outcome.decision === "continue"
          ? `Iteration ${iteration} read context and proposed a bounded follow-up.`
          : `Iteration ${iteration} ended with ${outcome.decision}: ${outcome.reason}.`,
      consumed_authority: {
        tools: [requestedTool],
        mutations: 0,
        spend_minor: 0,
      },
    },
    handoff: outcome.nextIterationAllowed
      ? {
          target: "loop-host",
          next_iteration: iteration + 1,
          reason: outcome.nextIterationReason,
        }
      : null,
    receipt_notes: {
      loop_state_owner: "outer-loop-host",
      run_owner: "runx",
      context_boundary: "advisory-untrusted",
    },
  };
}

try {
  const output = buildDecision(readInputs());
  process.stdout.write(`${JSON.stringify(output, null, 2)}\n`);
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`loop decision failed: ${message}\n`);
  process.exit(1);
}
