import { describe, expect, it } from "vitest";

import {
  externalJobContinuationV1Schema,
  externalJobScheduleIntentFromOutput,
  validateExternalJobContinuationContract,
  validateExternalJobScheduleContract,
  validateExternalJobScheduleIntentContract,
  validateExternalJobStageRequestContract,
  validateExternalJobStageResultContract,
} from "./external-job.js";
import type { ExternalJobContinuationContract } from "../generated/contract-types.js";

const DIGEST_A = `sha256:${"a".repeat(64)}` as const;
const DIGEST_B = `sha256:${"b".repeat(64)}` as const;

function runnable(): ExternalJobContinuationContract {
  return {
    continuation_id: "job_ocr_1",
    principal_ref: { type: "principal", uri: "runx:principal:ausca" },
    seller_ref: { type: "principal", uri: "runx:principal:ausca" },
    purchase_ref: { type: "target", uri: "runx:purchase:ocr_1" },
    source_run_ref: { type: "act", uri: "runx:run:initial-run-1" },
    executor_binding: {
      skill: "ausca-document-ocr",
      runner: "async",
      package_digest: DIGEST_A,
      execution_closure_digest: DIGEST_B,
    },
    operation_identity: DIGEST_B,
    stage: "start",
    state: "runnable",
    attempts: 0,
    max_attempts: 6,
    next_attempt_at: "2026-08-27T00:00:00Z",
    deadline_at: "2026-08-27T01:00:00Z",
    created_at: "2026-08-27T00:00:00Z",
    updated_at: "2026-08-27T00:00:00Z",
  };
}

describe("external job continuation contract", () => {
  it("exposes the generated current-V1 schema and accepts runnable state", () => {
    expect(externalJobContinuationV1Schema).toMatchObject({
      $id: "https://schemas.runx.ai/runx/external-job-continuation/v1.json",
      "x-runx-schema": "runx.external_job_continuation.v1",
      "x-runx-packet": true,
    });
    expect(validateExternalJobContinuationContract(runnable())).toEqual(runnable());
  });

  it("requires normalized provider identity while waiting", () => {
    expect(() => validateExternalJobContinuationContract({
      ...runnable(),
      stage: "inspect",
      state: "waiting_external",
    })).toThrow("requires inspect and a provider job reference");
  });

  it("does not expose an artifact when a refund supersedes completion", () => {
    expect(() => validateExternalJobContinuationContract({
      ...runnable(),
      stage: "finalize",
      state: "superseded",
      next_attempt_at: undefined,
      provider_job_ref: { type: "target", uri: "runx:aws:transcribe:job-1" },
      result_artifact_ref: { type: "artifact", uri: "runx:artifact:result-1" },
      terminal_evidence_ref: { type: "verification", uri: "runx:verification:job-1" },
      terminal_evidence_digest: DIGEST_A,
    })).toThrow("cannot expose a result artifact");
  });

  it("binds successful continuation state to the exact sealed stage receipt", () => {
    const succeeded = {
      ...runnable(),
      stage: "finalize" as const,
      state: "succeeded" as const,
      next_attempt_at: undefined,
      provider_job_ref: { type: "target" as const, uri: "runx:aws:textract-analysis:job-1" },
      result_artifact_ref: { type: "artifact" as const, uri: "runx:artifact:result-1" },
      terminal_evidence_ref: { type: "verification" as const, uri: "runx:verification:job-1" },
      terminal_evidence_digest: DIGEST_A,
    };
    expect(() => validateExternalJobContinuationContract(succeeded))
      .toThrow("requires finalized artifact and evidence");
    expect(validateExternalJobContinuationContract({
      ...succeeded,
      terminal_receipt_ref: {
        type: "receipt",
        uri: `runx:receipt:sha256:${"c".repeat(64)}`,
      },
    }).state).toBe("succeeded");
  });

  it("bounds attempts and dead-letter failures", () => {
    expect(() => validateExternalJobContinuationContract({
      ...runnable(),
      attempts: 7,
    })).toThrow("between zero and max_attempts");
    expect(validateExternalJobContinuationContract({
      ...runnable(),
      state: "dead_letter",
      next_attempt_at: undefined,
      failure: { code: "poison_input", message: "Input cannot be resumed.", retryable: false },
    }).state).toBe("dead_letter");
  });

  it("validates the durable schedule and exact package stage boundary", () => {
    const schedule = {
      continuation_id: "job_ocr_1",
      principal_ref: runnable().principal_ref,
      seller_ref: runnable().seller_ref,
      purchase_ref: runnable().purchase_ref,
      source_run_ref: runnable().source_run_ref,
      executor_binding: runnable().executor_binding,
      operation_identity: DIGEST_B,
      checkpoint: { input_artifact_ref: "runx:artifact:input-1" },
      max_attempts: 6,
      next_attempt_at: "2026-08-27T00:00:00Z",
      deadline_at: "2026-08-27T01:00:00Z",
      created_at: "2026-08-27T00:00:00Z",
    } as const;
    expect(validateExternalJobScheduleContract(schedule)).toEqual(schedule);
    const intent = {
      schema: "runx.external_job_schedule_intent.v1",
      stage_runner: "continue",
      checkpoint: schedule.checkpoint,
      max_attempts: 6,
      initial_delay_ms: 0,
      deadline_ms: 60_000,
    } as const;
    expect(validateExternalJobScheduleIntentContract(intent)).toMatchObject({ max_attempts: 6 });
    expect(externalJobScheduleIntentFromOutput({
      schedule_intent: { data: intent },
    })).toEqual(intent);
    expect(externalJobScheduleIntentFromOutput({ result: "ordinary" })).toBeUndefined();
    expect(() => externalJobScheduleIntentFromOutput({
      schedule_intent: { data: intent },
      result: "smuggled",
    })).toThrow("must contain only schedule_intent");
    expect(() => validateExternalJobScheduleIntentContract({
      schema: "runx.external_job_schedule_intent.v1",
      stage_runner: " continue ",
      checkpoint: schedule.checkpoint,
      max_attempts: 6,
      initial_delay_ms: 0,
      deadline_ms: 60_000,
    })).toThrow("stage_runner is invalid");
    expect(() => validateExternalJobScheduleIntentContract({
      ...intent,
      initial_delay_ms: 60_001,
      deadline_ms: 60_000,
    })).toThrow("initial_delay_ms must not exceed deadline_ms");
    expect(() => externalJobScheduleIntentFromOutput({
      schedule_intent: { data: {
        ...intent,
        initial_delay_ms: 60_001,
        deadline_ms: 60_000,
      } },
    })).toThrow("initial_delay_ms must not exceed deadline_ms");
    expect(validateExternalJobStageRequestContract({
      continuation: runnable(),
      checkpoint: schedule.checkpoint,
      operation_key: DIGEST_A,
    })).toMatchObject({ operation_key: DIGEST_A });
    expect(validateExternalJobStageResultContract({
      status: "waiting",
      provider_job_ref: { type: "target", uri: "runx:aws:textract-analysis:job-1" },
      checkpoint: { pages: [] },
      retry_after_ms: 10_000,
    })).toMatchObject({ status: "waiting" });
  });

  it("bounds package checkpoint state and rejects retryable terminal provider failure", () => {
    expect(() => validateExternalJobScheduleContract({
      continuation_id: "job_ocr_1",
      principal_ref: runnable().principal_ref,
      seller_ref: runnable().seller_ref,
      purchase_ref: runnable().purchase_ref,
      source_run_ref: runnable().source_run_ref,
      executor_binding: runnable().executor_binding,
      operation_identity: DIGEST_B,
      checkpoint: { payload: "x".repeat(70_000) },
      max_attempts: 6,
      next_attempt_at: "2026-08-27T00:00:00Z",
      deadline_at: "2026-08-27T01:00:00Z",
      created_at: "2026-08-27T00:00:00Z",
    })).toThrow("exceeds 65536 bytes");
    expect(() => validateExternalJobStageResultContract({
      status: "provider_failed",
      provider_job_ref: { type: "target", uri: "runx:aws:textract-analysis:job-1" },
      evidence_ref: { type: "verification", uri: "runx:verification:job-1" },
      evidence_digest: DIGEST_A,
      failure: { code: "provider_failed", message: "retry", retryable: true },
    })).toThrow("must be terminal");
  });
});
