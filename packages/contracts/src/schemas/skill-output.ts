import { generatedSchema, validateContractSchema } from "../internal.js";
import type { SkillOutputContract } from "../generated/contract-types.js";

export const skillOutputV1Schema = generatedSchema<SkillOutputContract>("skill-output.schema.json");

export function validateSkillOutputContract(value: unknown, label = "skill output"): SkillOutputContract {
  return validateContractSchema(skillOutputV1Schema, value, label);
}
