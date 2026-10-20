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

/** An open turn as its agent reads it; the invocation's id is its address. */
export type TurnStatusContract = DeepReadonly<{
  invocation: AgentActInvocationContract;
  tools: readonly TurnToolContract[];
  calls: readonly TurnCallContract[];
}>;

export const turnCallSchema = generatedSchema<TurnCallContract>("turn-call.schema.json");
export const turnStatusSchema = generatedSchema<TurnStatusContract>("turn-status.schema.json");

export function validateTurnCallContract(value: unknown, label = "turn_call"): TurnCallContract {
  return validateContractSchema(turnCallSchema, value, label);
}

export function validateTurnStatusContract(value: unknown, label = "turn_status"): TurnStatusContract {
  return validateContractSchema(turnStatusSchema, value, label);
}
