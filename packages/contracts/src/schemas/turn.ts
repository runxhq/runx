import {
  type DeepReadonly,
  generatedSchema,
  validateContractSchema,
} from "../internal.js";
import type { AgentActInvocationContract } from "./agent-act.js";
import type { ResolutionRequestContract } from "./resolution.js";

/** What a turn call names: one entry of the turn's allowance. */
export type TurnTargetContract = DeepReadonly<
  | { kind: "tool"; tool: string }
  | { kind: "operation"; requirement: string; operation: string }
  | { kind: "skill"; skill: string }
  | { kind: "ask" }
>;

/**
 * How a call ended. A pending call lists what it waits on, each request by
 * its address: its own ask or approval, its retry while its effect is
 * unconfirmed, or what the child run it started waits on.
 */
export type TurnCallOutcomeContract = DeepReadonly<
  | { status: "completed"; output: Record<string, unknown>; receipt_id: string }
  | { status: "failed"; reason: string; receipt_id?: string }
  | { status: "refused"; reason: string }
  | { status: "pending"; waiting: readonly ResolutionRequestContract[] }
>;

/** One call an agent made in its turn, and how it ended. */
export type TurnCallContract = DeepReadonly<{
  index: number;
  target: TurnTargetContract;
  outcome: TurnCallOutcomeContract;
}>;

/** One call an open turn offers its agent, with the input it takes. */
export type TurnToolContract = DeepReadonly<{
  name: string;
  description: string;
  target: TurnTargetContract;
  input_schema: Record<string, unknown>;
}>;

/** An open turn as its agent reads it; the invocation's id is its address. */
export type TurnStatusContract = DeepReadonly<{
  invocation: AgentActInvocationContract;
  tools: readonly TurnToolContract[];
  calls: readonly TurnCallContract[];
}>;

export const turnCallSchema = generatedSchema<TurnCallContract>("turn-call.schema.json");
export const turnStatusSchema = generatedSchema<TurnStatusContract>("turn-status.schema.json");

export function validateTurnCallContract(value: unknown, label = "turn_call"): TurnCallContract {
  return validateContractSchema(turnCallSchema, value, label);
}

export function validateTurnStatusContract(value: unknown, label = "turn_status"): TurnStatusContract {
  return validateContractSchema(turnStatusSchema, value, label);
}
