import {
  type DeepReadonly,
  generatedSchema,
  validateContractSchema,
} from "../internal.js";
import type { AgentActInvocationContract } from "./agent-act.js";
import type { ResolutionRequestContract } from "./resolution.js";
import type { ProjectArtifactRefContract } from "./skill-output.js";

/** The exact package and execution closure a load bound for one runner. */
export type BoundClosureContract = DeepReadonly<{
  package_digest: string;
  closure_digest: string;
}>;

/**
 * What a turn call names: one entry of the turn's allowance. A skill is one
 * the allowance names, run by its default runner, or one runner of a skill a
 * load bound. A load's input is `{skill, runner?}` or `{connector}`.
 */
export type TurnTargetContract = DeepReadonly<
  | { kind: "tool"; tool: string }
  | { kind: "operation"; requirement: string; operation: string }
  | { kind: "skill"; skill: string; runner?: string; bound?: BoundClosureContract }
  | { kind: "ask" }
  | { kind: "load" }
>;

/** The package a load bound; a project's own has no tier and no publisher. */
export type LoadedPackageContract = DeepReadonly<
  | {
      kind: "skill";
      skill: string;
      trust_tier?: "first_party" | "verified" | "community";
      publisher?: string;
      package_digest: string;
    }
  | {
      kind: "connector";
      connector: string;
      trust_tier?: "first_party" | "verified" | "community";
      publisher?: string;
      manifest_digest: string;
    }
>;

/**
 * A skill's manual: its `SKILL.md` exactly as its package holds it, and the
 * digest of those bytes. It is the skill's operating guidance, which a host
 * gives its model as guidance below its own instructions and runx's
 * governance, never as a tool result's data.
 */
export type SkillManualContract = DeepReadonly<{
  sha256: string;
  content: string;
}>;

/** One runner of a loaded skill, as a model chooses it, and whether the load bound it. */
export type LoadedRunnerContract = DeepReadonly<{
  name: string;
  purpose?: string;
  inputs: readonly { name: string; type: string; required: boolean }[];
  bound: boolean;
}>;

/**
 * A completed load's output: what it bound, a skill's manual and runners,
 * the calls it adds to the turn, and each connector it reaches at the
 * manifest digest it binds. A skill's load binds the runner it names or the
 * skill's only runner; one that binds none is a reading, which asks no trust.
 */
export type TurnLoadContract = DeepReadonly<{
  package: LoadedPackageContract;
  manual?: SkillManualContract;
  runners?: readonly LoadedRunnerContract[];
  tools: readonly TurnToolContract[];
  connectors?: Record<string, string>;
}>;

/**
 * How a call ended. A completed call's `artifacts` hold each packet output as
 * a project artifact of the run, by output name: a later call passes one in
 * place of the packet. A pending call lists what it waits on, each request by
 * its address: its own ask or approval, its retry while its effect is
 * unconfirmed, or what the child run it started waits on.
 */
export type TurnCallOutcomeContract = DeepReadonly<
  | {
      status: "completed";
      output: Record<string, unknown>;
      receipt_id: string;
      artifacts?: Record<string, ProjectArtifactRefContract>;
    }
  | { status: "failed"; reason: string; receipt_id?: string }
  | { status: "refused"; reason: string }
  | { status: "pending"; waiting: readonly ResolutionRequestContract[] }
>;

/**
 * One call an agent made in its turn, and how it ended. `input_digest` is
 * `sha256:` over the canonical JSON of the call's input, so a host that
 * recorded a call before making it can prove which call sits at an index.
 * `call_id` is the id its host pinned on it, unique within the turn: a call
 * made again with it returns this call rather than running twice.
 */
export type TurnCallContract = DeepReadonly<{
  index: number;
  target: TurnTargetContract;
  input_digest: string;
  call_id?: string;
  outcome: TurnCallOutcomeContract;
}>;

/** One call an open turn offers its agent, with the input it takes. */
export type TurnToolContract = DeepReadonly<{
  name: string;
  description: string;
  target: TurnTargetContract;
  input_schema: Record<string, unknown>;
}>;

/**
 * One context entry of an act as a model reads it in an index: what it is,
 * whom it may be shared with, how large it is, and the ref `turn_read` reads
 * its data by.
 */
export type TurnContextLineContract = DeepReadonly<{
  ref: string;
  type?: string;
  summary?: string;
  scope: {
    purpose:
      | "procedure"
      | "mandate"
      | "confirmed_memory"
      | "observation"
      | "work_continuity"
      | "retrieved_history"
      | "hypothesis";
    trust: "operator_confirmed" | "skill_instructions" | "untrusted_evidence";
    sharing: "local_only" | "configured_model";
    goal_id: string | null;
    source_ref: string | null;
  };
  created_at: string;
  size_bytes: number;
}>;

type AgentContextEnvelope = AgentActInvocationContract["envelope"];

/**
 * An act's envelope as its turn's agent reads it: whole, except that each
 * context entry is a line of an index, its data read by `turn_read`.
 */
export type TurnEnvelopeContract = DeepReadonly<
  Omit<AgentContextEnvelope, "current_context" | "historical_context"> & {
    current_context: readonly TurnContextLineContract[];
    historical_context: readonly TurnContextLineContract[];
  }
>;

/** An agent act as its turn's agent reads it; its id is the turn's address. */
export type TurnInvocationContract = DeepReadonly<
  Omit<AgentActInvocationContract, "envelope"> & { envelope: TurnEnvelopeContract }
>;

/**
 * An open turn as its agent reads it: the act it answers, with its context as
 * an index, the calls it may make, and how each call it made so far ended.
 */
export type TurnStatusContract = DeepReadonly<{
  invocation: TurnInvocationContract;
  tools: readonly TurnToolContract[];
  calls: readonly TurnCallContract[];
}>;

/**
 * A page of what a turn's ref names (`current:<n>`, `historical:<n>`,
 * `input:<name>` or `call:<index>`), as text: a string as it is, any other
 * value as pretty JSON. Offsets count characters; `next_offset` reads on from
 * what the page delivered and is absent at the end.
 */
export type TurnReadPageContract = DeepReadonly<{
  ref: string;
  offset: number;
  text: string;
  next_offset?: number;
  total: number;
}>;

export const turnCallSchema = generatedSchema<TurnCallContract>("turn-call.schema.json");
export const turnStatusSchema = generatedSchema<TurnStatusContract>("turn-status.schema.json");
export const turnReadSchema = generatedSchema<TurnReadPageContract>("turn-read.schema.json");

export function validateTurnCallContract(value: unknown, label = "turn_call"): TurnCallContract {
  return validateContractSchema(turnCallSchema, value, label);
}

export function validateTurnStatusContract(value: unknown, label = "turn_status"): TurnStatusContract {
  return validateContractSchema(turnStatusSchema, value, label);
}

export function validateTurnReadContract(value: unknown, label = "turn_read"): TurnReadPageContract {
  return validateContractSchema(turnReadSchema, value, label);
}
