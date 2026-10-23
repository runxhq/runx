import { generatedSchema, validateContractSchema } from "../internal.js";
import type { ResumeDocumentContract } from "../generated/contract-types.js";

export const resumeDocumentSchema = generatedSchema<ResumeDocumentContract>("resume.schema.json");

export function validateResumeDocumentContract(value: unknown, label = "resume"): ResumeDocumentContract {
  return validateContractSchema(resumeDocumentSchema, value, label);
}
