import { generatedSchema, validateContractSchema } from "../internal.js";
import type { ProjectArtifactContract, SkillOutputContract } from "../generated/contract-types.js";

export const skillOutputV1Schema = generatedSchema<SkillOutputContract>("skill-output.schema.json");
export const projectArtifactV1Schema = generatedSchema<ProjectArtifactContract>("project-artifact.schema.json");

export function validateSkillOutputContract(value: unknown, label = "skill output"): SkillOutputContract {
  return validateContractSchema(skillOutputV1Schema, value, label);
}

export function validateProjectArtifactContract(value: unknown, label = "project artifact"): ProjectArtifactContract {
  return validateContractSchema(projectArtifactV1Schema, value, label);
}
