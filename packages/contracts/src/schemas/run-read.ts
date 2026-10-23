import { generatedSchema, validateContractSchema } from "../internal.js";
import type { RunEventRecordContract, RunReadContract } from "../generated/contract-types.js";

export const runReadSchema = generatedSchema<RunReadContract>("run-read.schema.json");
export const runEventSchema = generatedSchema<RunEventRecordContract>("run-event.schema.json");

export function validateRunReadContract(value: unknown, label = "run_read"): RunReadContract {
  return validateContractSchema(runReadSchema, value, label);
}

export function validateRunEventContract(value: unknown, label = "run_event"): RunEventRecordContract {
  return validateContractSchema(runEventSchema, value, label);
}
