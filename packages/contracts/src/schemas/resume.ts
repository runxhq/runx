import {
  type DeepReadonly,
  generatedSchema,
  validateContractSchema,
} from "../internal.js";

/**
 * What a host brings a paused run (`runx resume <run-id> -`), keyed by the
 * address of the request each entry answers. Every entry carries the
 * `request_digest` the run reported for that request, so a decision never
 * reaches a request that changed after the person saw it. A close comes
 * alone, and leaves an unconfirmed effect unconfirmed.
 */
export type ResumeDocumentContract = DeepReadonly<{
  answers?: Record<string, { request_digest: string; value: unknown }>;
  approvals?: Record<string, { request_digest: string; approved: boolean; reason?: string }>;
  close?: { disposition: "declined" | "superseded" };
}>;

export const resumeDocumentSchema = generatedSchema<ResumeDocumentContract>("resume.schema.json");

export function validateResumeDocumentContract(value: unknown, label = "resume"): ResumeDocumentContract {
  return validateContractSchema(resumeDocumentSchema, value, label);
}
