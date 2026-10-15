import {
  type DeepReadonly,
  generatedSchema,
  validateContractSchema,
} from "../internal.js";

/**
 * What a host brings a paused run (`runx resume <run-id> -`), keyed by the
 * address of the request each entry answers. Every entry carries the
 * `request_digest` the run reported for that request, so a decision never
 * reaches a request that changed after the person saw it. `answers` is the
 * agent's lane (inputs and acts); `approvals` is the person's: an approval
 * takes `approved` and `reason`, a claimed call's retry `value: {retry:
 * true}`. Runx records each decision it admits before it goes further. A
 * close comes alone, and leaves an unconfirmed effect unconfirmed.
 */
export type ResumeDocumentContract = DeepReadonly<{
  answers?: Record<string, { request_digest: string; value: unknown }>;
  approvals?: Record<
    string,
    { request_digest: string; approved?: boolean; reason?: string; value?: unknown }
  >;
  /** The person's new standing approvals, replacing the run's set. */
  standing_approvals?: readonly StandingApprovalContract[];
  close?: { disposition: "declined" | "superseded" };
}>;

/**
 * A person's bounded confirmation of the local effects a run worked by an
 * agent would otherwise hold for them: exactly one of `edits` (file changes
 * under these workspace-relative paths, `""` the whole workspace),
 * `commands` (an argv beginning with these tokens) and `local` (every local
 * effect). It never covers an operation's own declared approval.
 */
export type StandingApprovalContract = DeepReadonly<
  | { edits: { paths: string[] } }
  | { commands: { argv_prefix: string[] } }
  | { local: true }
>;

export const resumeDocumentSchema = generatedSchema<ResumeDocumentContract>("resume.schema.json");

export function validateResumeDocumentContract(value: unknown, label = "resume"): ResumeDocumentContract {
  return validateContractSchema(resumeDocumentSchema, value, label);
}
