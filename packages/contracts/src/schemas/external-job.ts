import { generatedSchema, validateContractSchema } from "../internal.js";
import { canonicalJsonStringify } from "../canonical-json.js";
import type {
  ExternalJobCheckpointContract,
  ExternalJobContinuationContract,
  ExternalJobScheduleContract,
  ExternalJobScheduleIntentContract,
  ExternalJobStageRequestContract,
  ExternalJobStageResultContract,
  ReferenceContract,
} from "../generated/contract-types.js";

export const externalJobContinuationV1Schema =
  generatedSchema<ExternalJobContinuationContract>("external-job-continuation.schema.json");
export const externalJobScheduleV1Schema =
  generatedSchema<ExternalJobScheduleContract>("external-job-schedule.schema.json");
export const externalJobScheduleIntentV1Schema =
  generatedSchema<ExternalJobScheduleIntentContract>("external-job-schedule-intent.schema.json");
export const externalJobStageRequestV1Schema =
  generatedSchema<ExternalJobStageRequestContract>("external-job-stage-request.schema.json");
export const externalJobStageResultV1Schema =
  generatedSchema<ExternalJobStageResultContract>("external-job-stage-result.schema.json");

export function validateExternalJobContinuationContract(
  value: unknown,
  label = "external_job_continuation",
): ExternalJobContinuationContract {
  const continuation = validateContractSchema(externalJobContinuationV1Schema, value, label);
  validateSemantics(continuation, label);
  return continuation;
}

export function validateExternalJobScheduleContract(
  value: unknown,
  label = "external_job_schedule",
): ExternalJobScheduleContract {
  const schedule = validateContractSchema(externalJobScheduleV1Schema, value, label);
  validateIdentityReferences(schedule, label);
  checkpoint(schedule.checkpoint, `${label}.checkpoint`);
  const createdAt = timestamp(schedule.created_at, `${label}.created_at`);
  const nextAttemptAt = timestamp(schedule.next_attempt_at, `${label}.next_attempt_at`);
  const deadlineAt = timestamp(schedule.deadline_at, `${label}.deadline_at`);
  if (nextAttemptAt < createdAt || nextAttemptAt > deadlineAt || deadlineAt <= createdAt) {
    throw new Error(`${label} timestamps are not ordered.`);
  }
  return schedule;
}

export function validateExternalJobScheduleIntentContract(
  value: unknown,
  label = "external_job_schedule_intent",
): ExternalJobScheduleIntentContract {
  const intent = validateContractSchema(externalJobScheduleIntentV1Schema, value, label);
  runner(intent.stage_runner, `${label}.stage_runner`);
  checkpoint(intent.checkpoint, `${label}.checkpoint`);
  if (intent.initial_delay_ms > intent.deadline_ms) {
    throw new Error(`${label}.initial_delay_ms must not exceed deadline_ms.`);
  }
  return intent;
}

/**
 * Reads the exact native Runx output envelope used to request a durable
 * external job. A non-schedule output is not an error; an output that claims
 * to be a schedule but changes the closed envelope is.
 */
export function externalJobScheduleIntentFromOutput(
  value: unknown,
  label = "external_job_schedule_intent_output",
): ExternalJobScheduleIntentContract | undefined {
  if (!value || Array.isArray(value) || typeof value !== "object") return undefined;
  const output = value as Readonly<Record<string, unknown>>;
  if (!Object.prototype.hasOwnProperty.call(output, "schedule_intent")) return undefined;
  if (Object.keys(output).length !== 1) {
    throw new Error(`${label} must contain only schedule_intent.`);
  }
  const envelope = output.schedule_intent;
  if (!envelope || Array.isArray(envelope) || typeof envelope !== "object") {
    throw new Error(`${label}.schedule_intent must be a Runx value envelope.`);
  }
  const wrapped = envelope as Readonly<Record<string, unknown>>;
  if (Object.keys(wrapped).length !== 1 || !Object.prototype.hasOwnProperty.call(wrapped, "data")) {
    throw new Error(`${label}.schedule_intent envelope is invalid.`);
  }
  return validateExternalJobScheduleIntentContract(
    wrapped.data,
    `${label}.schedule_intent.data`,
  );
}

export function validateExternalJobStageRequestContract(
  value: unknown,
  label = "external_job_stage_request",
): ExternalJobStageRequestContract {
  const request = validateContractSchema(externalJobStageRequestV1Schema, value, label);
  validateExternalJobContinuationContract(request.continuation, `${label}.continuation`);
  checkpoint(request.checkpoint, `${label}.checkpoint`);
  return request;
}

export function validateExternalJobStageResultContract(
  value: unknown,
  label = "external_job_stage_result",
): ExternalJobStageResultContract {
  const result = validateContractSchema(externalJobStageResultV1Schema, value, label);
  referenceType(result.provider_job_ref, "target", `${label}.provider_job_ref`);
  if (result.status === "waiting" || result.status === "materialize") {
    checkpoint(result.checkpoint, `${label}.checkpoint`);
  } else {
    referenceType(result.evidence_ref, "verification", `${label}.evidence_ref`);
    if (result.status === "ready") {
      referenceType(result.result_artifact_ref, "artifact", `${label}.result_artifact_ref`);
    } else if (result.failure.retryable) {
      throw new Error(`${label}.provider_failed must be terminal.`);
    }
  }
  return result;
}

function validateSemantics(
  continuation: ExternalJobContinuationContract,
  label: string,
): void {
  validateIdentityReferences(continuation, label);
  if (
    !Number.isSafeInteger(continuation.attempts)
    || continuation.attempts < 0
    || continuation.attempts > continuation.max_attempts
  ) {
    throw new Error(`${label}.attempts must be a safe integer between zero and max_attempts.`);
  }
  const createdAt = timestamp(continuation.created_at, `${label}.created_at`);
  const updatedAt = timestamp(continuation.updated_at, `${label}.updated_at`);
  const deadlineAt = timestamp(continuation.deadline_at, `${label}.deadline_at`);
  if (updatedAt < createdAt || deadlineAt <= createdAt) {
    throw new Error(`${label} timestamps are not ordered.`);
  }
  if (continuation.next_attempt_at) {
    const nextAttemptAt = timestamp(continuation.next_attempt_at, `${label}.next_attempt_at`);
    if (nextAttemptAt < createdAt || nextAttemptAt > deadlineAt) {
      throw new Error(`${label}.next_attempt_at must fall within the continuation deadline.`);
    }
  }

  const terminal = ["succeeded", "failed", "superseded", "dead_letter"]
    .includes(continuation.state);
  if (terminal === Boolean(continuation.next_attempt_at)) {
    throw new Error(`${label} must schedule exactly non-terminal state.`);
  }
  if (continuation.state === "waiting_external") {
    if (continuation.stage !== "inspect" || !continuation.provider_job_ref) {
      throw new Error(`${label} waiting_external state requires inspect and a provider job reference.`);
    }
  } else if (continuation.state === "runnable") {
    if (continuation.stage === "inspect") {
      throw new Error(`${label} inspect state must be waiting_external.`);
    }
    if (continuation.stage !== "start" && !continuation.provider_job_ref) {
      throw new Error(`${label} post-start state requires a provider job reference.`);
    }
  }

  if (!terminal) {
    if (
      continuation.result_artifact_ref
      || continuation.terminal_receipt_ref
      || continuation.terminal_evidence_ref
      || continuation.terminal_evidence_digest
      || continuation.failure
    ) {
      throw new Error(`${label} active state cannot expose terminal fields.`);
    }
    return;
  }
  if (continuation.state === "succeeded") {
    if (
      continuation.stage !== "finalize"
      || !continuation.result_artifact_ref
      || !continuation.terminal_receipt_ref
      || !continuation.terminal_evidence_ref
      || !continuation.terminal_evidence_digest
      || continuation.failure
    ) {
      throw new Error(`${label} succeeded state requires finalized artifact and evidence.`);
    }
    referenceType(
      continuation.terminal_receipt_ref,
      "receipt",
      `${label}.terminal_receipt_ref`,
    );
    return;
  }
  if (continuation.terminal_receipt_ref) {
    throw new Error(`${label} non-success terminal state cannot expose an execution receipt.`);
  }
  if (continuation.result_artifact_ref) {
    throw new Error(`${label} non-success terminal state cannot expose a result artifact.`);
  }
  if (continuation.state === "dead_letter") {
    if (!continuation.failure || continuation.failure.retryable) {
      throw new Error(`${label} dead_letter state requires a non-retryable bounded failure.`);
    }
    return;
  }
  if (!continuation.terminal_evidence_ref || !continuation.terminal_evidence_digest) {
    throw new Error(`${label} terminal provider outcome requires exact evidence.`);
  }
  if (continuation.state === "failed" && (!continuation.failure || continuation.failure.retryable)) {
    throw new Error(`${label} failed state requires a terminal failure.`);
  }
  if (continuation.state === "superseded" && continuation.failure) {
    throw new Error(`${label} superseded state cannot claim provider failure.`);
  }
}

function validateIdentityReferences(
  value: Pick<
    ExternalJobContinuationContract,
    "purchase_ref" | "source_run_ref"
  >,
  label: string,
): void {
  referenceType(value.purchase_ref, "target", `${label}.purchase_ref`);
  if (!value.purchase_ref.uri.startsWith("runx:purchase:")) {
    throw new Error(`${label}.purchase_ref must identify a purchase.`);
  }
  referenceType(value.source_run_ref, "act", `${label}.source_run_ref`);
  if (!value.source_run_ref.uri.startsWith("runx:run:")) {
    throw new Error(`${label}.source_run_ref must identify a hosted run.`);
  }
}

function referenceType(
  reference: ReferenceContract,
  type: ReferenceContract["type"],
  label: string,
): void {
  if (reference.type !== type) throw new Error(`${label}.type must be ${type}.`);
}

function timestamp(value: string, label: string): number {
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) throw new Error(`${label} must be a valid timestamp.`);
  return parsed;
}

function checkpoint(value: ExternalJobCheckpointContract, label: string): void {
  const bytes = new TextEncoder().encode(canonicalJsonStringify(value)).byteLength;
  if (bytes > 64 * 1024) throw new Error(`${label} exceeds 65536 bytes.`);
}

function runner(value: string, label: string): void {
  if (
    value !== value.trim()
    || !/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u.test(value)
  ) {
    throw new Error(`${label} is invalid.`);
  }
}
