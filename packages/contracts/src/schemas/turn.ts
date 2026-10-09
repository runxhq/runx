import {
  type DeepReadonly,
  generatedSchema,
  validateContractSchema,
} from "../internal.js";
import type { TurnAllowanceContract } from "./agent-act.js";
import type { ResolutionRequestContract } from "./resolution.js";

/** What a turn call names: an allowance entry, or a question for the human. */
export type TurnTargetContract = DeepReadonly<
  | { kind: "tool"; tool: string }
  | { kind: "operation"; requirement: string; operation: string }
  | { kind: "skill"; skill: string }
  | { kind: "ask" }
>;

export type TurnCallOutcomeContract = DeepReadonly<
  | { status: "completed"; output: Record<string, unknown>; receipt_id: string }
  | { status: "failed"; reason: string; receipt_id?: string }
  | { status: "refused"; reason: string }
  | { status: "pending"; request: ResolutionRequestContract }
>;

/** One call an agent made in its turn, and how it ended. */
export type TurnCallContract = DeepReadonly<{
  index: number;
  target: TurnTargetContract;
  outcome: TurnCallOutcomeContract;
}>;

/** An open turn as its agent reads it. */
export type TurnStatusContract = DeepReadonly<{
  step_id: string;
  request_id: string;
  allowance: TurnAllowanceContract;
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
