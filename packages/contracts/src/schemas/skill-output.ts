import { generatedSchema, validateContractSchema } from "../internal.js";

export interface ProjectArtifactRefContract {
  readonly schema: "runx.project_artifact_ref.v1";
  readonly ref: string;
  readonly digest: string;
  readonly bytes: number;
  readonly media_type: string;
  readonly path: string;
}

/** Native transport envelope; additional execution fields pass through intact. */
export interface SkillOutputContract extends Readonly<Record<string, unknown>> {
  readonly schema?: "runx.skill_run.v1";
  readonly status: "sealed" | "needs_agent" | "needs_approval" | "payment_required" | "policy_denied" | "failure";
  readonly outcome?: "completed" | "deferred" | "blocked" | "failed";
  readonly run_id?: string;
  readonly receipt_id?: string;
  readonly result?: unknown;
  readonly requests?: readonly unknown[];
  readonly request_set_ref?: ProjectArtifactRefContract;
  readonly diagnostics_ref?: ProjectArtifactRefContract;
  readonly artifact_refs?: readonly ProjectArtifactRefContract[];
}

export const skillOutputV1Schema = generatedSchema("skill-output.schema.json");
export const projectArtifactRefV1Schema = generatedSchema("project-artifact-ref.schema.json");

export function validateSkillOutputContract(value: unknown, label = "skill output"): SkillOutputContract {
  return validateContractSchema(skillOutputV1Schema, value, label) as SkillOutputContract;
}

export function validateProjectArtifactRefContract(value: unknown, label = "project artifact"): ProjectArtifactRefContract {
  return validateContractSchema(projectArtifactRefV1Schema, value, label) as ProjectArtifactRefContract;
}
