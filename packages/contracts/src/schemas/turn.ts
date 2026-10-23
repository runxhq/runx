import { generatedSchema, validateContractSchema } from "../internal.js";
import type {
  TurnCallResultContract,
  TurnLoadContract,
  TurnReadPageContract,
  TurnStatusContract,
} from "../generated/contract-types.js";

export const turnCallSchema = generatedSchema<TurnCallResultContract>("turn-call.schema.json");
export const turnLoadSchema = generatedSchema<TurnLoadContract>("turn-load.schema.json");
export const turnStatusSchema = generatedSchema<TurnStatusContract>("turn-status.schema.json");
export const turnReadSchema = generatedSchema<TurnReadPageContract>("turn-read.schema.json");

export function validateTurnCallContract(value: unknown, label = "turn_call"): TurnCallResultContract {
  return validateContractSchema(turnCallSchema, value, label);
}

/** A completed load's output, as a host reads it from the load's call to give its model the skill. */
export function validateTurnLoadContract(value: unknown, label = "turn_load"): TurnLoadContract {
  return validateContractSchema(turnLoadSchema, value, label);
}

export function validateTurnStatusContract(value: unknown, label = "turn_status"): TurnStatusContract {
  return validateContractSchema(turnStatusSchema, value, label);
}

export function validateTurnReadContract(value: unknown, label = "turn_read"): TurnReadPageContract {
  return validateContractSchema(turnReadSchema, value, label);
}
