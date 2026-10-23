import { generatedSchema, validateContractSchema } from "../internal.js";
import type {
  ProcessListContract,
  ProcessOutputContract,
  RunxProcessContract,
} from "../generated/contract-types.js";

export const processSchema = generatedSchema<RunxProcessContract>("process.schema.json");
export const processOutputSchema = generatedSchema<ProcessOutputContract>("process-output.schema.json");
export const processListSchema = generatedSchema<ProcessListContract>("process-list.schema.json");

export function validateProcessContract(value: unknown, label = "process"): RunxProcessContract {
  return validateContractSchema(processSchema, value, label);
}

export function validateProcessOutputContract(value: unknown, label = "process_output"): ProcessOutputContract {
  return validateContractSchema(processOutputSchema, value, label);
}

export function validateProcessListContract(value: unknown, label = "process_list"): ProcessListContract {
  return validateContractSchema(processListSchema, value, label);
}
