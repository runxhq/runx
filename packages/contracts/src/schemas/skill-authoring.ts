import { generatedSchema, validateContractSchema } from "../internal.js";
import type {
  SkillApplyResultContract,
  SkillArchitectureDecisionContract,
  SkillArchitecturePlanContract,
  SkillChangeBundleContract,
  SkillChangeDraftContract,
  SkillValidationResultContract,
} from "../generated/contract-types.js";

export const skillArchitectureDecisionV1Schema =
  generatedSchema<SkillArchitectureDecisionContract>("skill-architecture-decision.schema.json");
export const skillArchitecturePlanV1Schema =
  generatedSchema<SkillArchitecturePlanContract>("skill-architecture-plan.schema.json");
export const skillChangeDraftV1Schema =
  generatedSchema<SkillChangeDraftContract>("skill-change-draft.schema.json");
export const skillChangeBundleV1Schema =
  generatedSchema<SkillChangeBundleContract>("skill-change-bundle.schema.json");
export const skillValidationResultV1Schema =
  generatedSchema<SkillValidationResultContract>("skill-validation-result.schema.json");
export const skillApplyResultV1Schema =
  generatedSchema<SkillApplyResultContract>("skill-apply-result.schema.json");

export function validateSkillArchitectureDecisionContract(
  value: unknown,
  label = "skill_architecture_decision",
): SkillArchitectureDecisionContract {
  return validateContractSchema(skillArchitectureDecisionV1Schema, value, label);
}

export function validateSkillArchitecturePlanContract(
  value: unknown,
  label = "skill_architecture_plan",
): SkillArchitecturePlanContract {
  return validateContractSchema(skillArchitecturePlanV1Schema, value, label);
}

export function validateSkillChangeDraftContract(
  value: unknown,
  label = "skill_change_draft",
): SkillChangeDraftContract {
  return validateContractSchema(skillChangeDraftV1Schema, value, label);
}

export function validateSkillChangeBundleContract(
  value: unknown,
  label = "skill_change_bundle",
): SkillChangeBundleContract {
  return validateContractSchema(skillChangeBundleV1Schema, value, label);
}

export function validateSkillValidationResultContract(
  value: unknown,
  label = "skill_validation_result",
): SkillValidationResultContract {
  return validateContractSchema(skillValidationResultV1Schema, value, label);
}

export function validateSkillApplyResultContract(
  value: unknown,
  label = "skill_apply_result",
): SkillApplyResultContract {
  return validateContractSchema(skillApplyResultV1Schema, value, label);
}
