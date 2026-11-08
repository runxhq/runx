import {
  generatedSchema,
  validateContractSchema,
} from "../internal.js";
import type { OperationalPolicyContract } from "../generated/contract-types.js";

export const operationalPolicySchema =
  generatedSchema<OperationalPolicyContract>("operational-policy.schema.json");

export function validateOperationalPolicyContract(
  value: unknown,
  label = "operational_policy",
): OperationalPolicyContract {
  return validateContractSchema(operationalPolicySchema, value, label);
}
