import { generatedSchema, validateContractSchema } from "../internal.js";
import type { ActAssignmentContract } from "../generated/contract-types.js";

export const actAssignmentV1Schema = generatedSchema<ActAssignmentContract>(
  "act-assignment.schema.json",
);

export function validateActAssignmentContract(
  value: unknown,
  label = "act_assignment",
): ActAssignmentContract {
  return validateContractSchema(actAssignmentV1Schema, value, label);
}
