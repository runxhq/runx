import { generatedSchema, validateContractSchema } from "../internal.js";
import type { OutputContract } from "../generated/contract-types.js";

export const outputSchema = generatedSchema<OutputContract>("output.schema.json");

export function validateOutputContract(
  value: unknown,
  label = "output",
): OutputContract {
  return validateContractSchema(outputSchema, value, label);
}
