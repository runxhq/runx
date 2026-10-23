import { generatedSchema, validateContractSchema } from "../internal.js";
import type { SkillInspectionContract } from "../generated/contract-types.js";

export const skillInspectionSchema = generatedSchema<SkillInspectionContract>(
  "skill-inspection.schema.json",
);

export function validateSkillInspectionContract(
  value: unknown,
  label = "skill_inspection",
): SkillInspectionContract {
  return validateContractSchema(skillInspectionSchema, value, label);
}
