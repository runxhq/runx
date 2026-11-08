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

function decide({ scenario, turnIndex, maxTurns, requestedTool }) {
  if (turnIndex > maxTurns) {
    return {
      decision: "refused",
      reason: "turn_budget_exhausted",
      nextTurnAllowed: false,
      nextTurnReason: "max_turns has already been exhausted",
    };
  }

  if (!ALLOWED_TOOLS.includes(requestedTool)) {
    return {
      decision: "refused",
      reason: "tool_not_allowed",
      nextTurnAllowed: false,
      nextTurnReason: `${requestedTool} is not in allowed_tools`,
    };
  }

  if (scenario === "pause") {
    return {
      decision: "needs_human",
      reason: "side_effect_review_required",
      nextTurnAllowed: false,
      nextTurnReason: "human review is required before the next turn",
    };
  }

  if (turnIndex >= Math.min(maxTurns, 2)) {
    return {
      decision: "done",
      reason: "fixture_objective_complete",
      nextTurnAllowed: false,
      nextTurnReason: "review accepted the bounded fixture result",
    };
  }

  return {
    decision: "continue",
    reason: "bounded_next_turn_available",
    nextTurnAllowed: true,
    nextTurnReason: "one more governed turn is allowed by policy",
  };
}

function buildTurn(inputs) {
  const loopId = stringInput(inputs, "loop_id", "loop-demo");
  const turnIndex = integerInput(inputs, "turn_index", 1);
  const maxTurns = integerInput(inputs, "max_turns", 3);
  const objective = stringInput(inputs, "objective", "Demonstrate a governed loop turn.");
  const previousSummary = stringInput(inputs, "previous_summary", "No prior turn.");
  const requestedTool = stringInput(inputs, "requested_tool", "fs.read");
  const scenario = stringInput(inputs, "scenario", "success");
  const outcome = decide({ scenario, turnIndex, maxTurns, requestedTool });

  if (!DECISIONS.includes(outcome.decision)) {
    throw new Error(`invalid fixture decision ${outcome.decision}`);
  }

  return {
    schema: "runx.loop_turn.v1",
    loop_id: loopId,
    turn_id: `${loopId}:turn-${turnIndex}`,
    turn_index: turnIndex,
    max_turns: maxTurns,
    objective,
    previous_summary: previousSummary,
    requested_tool: requestedTool,
    allowed_tools: [...ALLOWED_TOOLS],
    decision: outcome.decision,
    reason_code: outcome.reason,
    next_turn_allowed: outcome.nextTurnAllowed,
    next_turn_reason: outcome.nextTurnReason,
    projection_update: {
      latest_summary:
        outcome.decision === "continue"
          ? `Turn ${turnIndex} read context and proposed a bounded follow-up.`
          : `Turn ${turnIndex} ended with ${outcome.decision}: ${outcome.reason}.`,
      consumed_authority: {
        tools: [requestedTool],
        mutations: 0,
        spend_minor: 0,
      },
    },
    handoff: outcome.nextTurnAllowed
      ? {
          target: "loop-host",
          next_turn_index: turnIndex + 1,
          reason: outcome.nextTurnReason,
        }
      : null,
    receipt_notes: {
      loop_state_owner: "outer-loop-host",
      runx_turn_owner: "runx",
      context_boundary: "advisory-untrusted",
    },
  };
}

try {
  const output = buildTurn(readInputs());
  process.stdout.write(`${JSON.stringify(output, null, 2)}\n`);
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`loop turn failed: ${message}\n`);
  process.exit(1);
}
