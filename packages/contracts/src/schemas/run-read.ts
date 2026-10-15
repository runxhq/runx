import {
  type DeepReadonly,
  generatedSchema,
  validateContractSchema,
} from "../internal.js";
import type { ResolutionRequestContract } from "./resolution.js";
import type { StandingApprovalContract } from "./resume.js";
import type { ProjectArtifactRefContract } from "./skill-output.js";
import type { ClosureDispositionContract } from "./spine.js";
import type { TurnTargetContract } from "./turn.js";

export type RunStateContract = "running" | "paused" | "sealed";

/** Who works a run's agent turns: the host it is attached to, or runx's managed agent. */
export type RunDriverContract = "attached" | "managed";

/** A request a run waits on: its address, the digest an answer binds to, and the request. */
export type WaitedRequestContract = DeepReadonly<{
  address: string;
  request_digest: string;
  request: ResolutionRequestContract;
}>;

export type RunHoldContract = DeepReadonly<
  | { kind: "request" | "retry" | "claimed"; address: string; request_digest: string }
  | { kind: "child"; run_id: string }
>;

/** A project artifact holding part of a call's output: a packet output by name, or the rest. */
export type RunCallOutputContract = DeepReadonly<{
  output?: string;
  artifact: ProjectArtifactRefContract;
}>;

export type RunCallOutcomeContract = DeepReadonly<
  | {
      status: "completed";
      succeeded: boolean;
      receipt_id: string;
      outputs?: readonly RunCallOutputContract[];
      /** Why a call that ran did not succeed. */
      reason?: string;
    }
  | { status: "refused" | "failed"; reason: string }
  | { status: "pending" | "claimed"; address: string; request_digest: string }
  | { status: "child"; run_id: string }
>;

export type RunEventFactContract = DeepReadonly<
  | { kind: "started"; skill: string; runner: string }
  | { kind: "child_started"; parent: string; step_id: string; skill: string }
  | { kind: "step"; step_id: string; attempt: number; succeeded: boolean; receipt_id: string }
  | { kind: "skipped"; step_ids: readonly string[] }
  | { kind: "suspended"; step_id: string; attempt: number; hold: RunHoldContract }
  | {
      kind: "call";
      step_id: string;
      attempt: number;
      turn: string;
      index: number;
      target: TurnTargetContract;
      input_digest: string;
      /** The id its host pinned on the call. */
      call_id?: string;
      /** The standing approval that admitted it on the approval floor. */
      standing?: StandingApprovalContract;
      outcome: RunCallOutcomeContract;
    }
  | { kind: "sealed"; disposition: ClosureDispositionContract; receipt_id: string }
  | { kind: "standing"; approvals: readonly StandingApprovalContract[] }
  | {
      /** A resume admitted this decision before it took the run further. */
      kind: "answered";
      address: string;
      request_digest: string;
      answer_digest: string;
      actor: "human" | "agent";
    }
>;

/** One event of a run's record: references and digests, never outputs or secrets. */
export type RunEventContract = DeepReadonly<{
  stream: string;
  version: number;
  /** For display only: it orders nothing. */
  committed_at: string;
  event: RunEventFactContract;
}>;

/**
 * `runx history <run-id> --json`: the run, what it waits on, and its events
 * after the cursor, stream by stream. Pass `cursor` back as `--after` to read
 * what follows; a read after it never skips or repeats an event.
 */
export type RunReadContract = DeepReadonly<{
  schema: "runx.run_read.v1";
  run: {
    run_id: string;
    skill: string;
    runner: string;
    state: RunStateContract;
    disposition?: ClosureDispositionContract;
    /**
     * The execution package digest of the package the run loaded, not the
     * registry's digest of the published package files a listing carries.
     */
    package_digest: string;
    execution_closure_digest: string;
    driver: RunDriverContract;
    receipt_id?: string;
    /** Whether a process holds the run's driver lock now. */
    driven: boolean;
    created_at: string;
    updated_at: string;
  };
  waiting: readonly WaitedRequestContract[];
  events: readonly RunEventContract[];
  cursor: string;
  more: boolean;
}>;

export const runReadSchema = generatedSchema<RunReadContract>("run-read.schema.json");
export const runEventSchema = generatedSchema<RunEventContract>("run-event.schema.json");

export function validateRunReadContract(value: unknown, label = "run_read"): RunReadContract {
  return validateContractSchema(runReadSchema, value, label);
}

export function validateRunEventContract(value: unknown, label = "run_event"): RunEventContract {
  return validateContractSchema(runEventSchema, value, label);
}
