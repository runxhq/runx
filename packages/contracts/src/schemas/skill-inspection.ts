import {
  type DeepReadonly,
  generatedSchema,
  validateContractSchema,
} from "../internal.js";

/** One input of a runner, as a caller fills it. */
export type InspectedInputContract = DeepReadonly<{
  name: string;
  type: string;
  required: boolean;
  description?: string;
  schema: Record<string, unknown>;
}>;

/** One output of a runner's result. */
export type InspectedOutputContract = DeepReadonly<{
  name: string;
  type?: string;
  required?: boolean;
}>;

/** One runner as a caller reads it: what it takes, returns and needs. */
export type RunnerInspectionContract = DeepReadonly<{
  name: string;
  input_schema: Record<string, unknown>;
  inputs: InspectedInputContract[];
  outputs: InspectedOutputContract[];
  declared_result_packets: Record<string, string>;
  requirements: {
    scopes?: string[];
    environment?: { required?: string[]; optional?: string[] };
  };
  scopes?: string[];
  continuations?: string[];
  connectors?: Record<string, unknown>;
}>;

export type OperatorJourneyContract = DeepReadonly<{
  case: string;
  mode: string;
  request: string;
  expected_outcome: string;
  confusors: string[];
  prior_evidence: string[];
  must_not_repeat: string[];
  runner?: string;
  exercises_runner?: string;
}>;

/**
 * A skill package's inspection (`runx.skill.inspect.v1`): what `runx skill
 * inspect` and a turn's `registry.inspect` report of a package, by runner,
 * without running it. The execution closure keeps the shape its owner gives
 * it.
 */
export type SkillInspectionContract = DeepReadonly<{
  schema: "runx.skill.inspect.v1";
  status: "ok";
  name: string;
  skill_path: string;
  manual_digest: string;
  package_digest: string;
  description?: string;
  version?: string;
  capabilities?: { execution: string; completion: string; approval: string };
  catalog?: {
    kind: string;
    audience: string;
    visibility: string;
    role: string;
    canonical_skill?: string;
    provider?: string;
    part_of?: string[];
    execution?: string;
    completion?: string;
    approval?: string;
  };
  semantic_report?: unknown;
  runners: string[];
  operator_journeys: OperatorJourneyContract[];
  runner_inspections?: { runner: RunnerInspectionContract; execution_closure: Record<string, unknown> }[];
  runner?: RunnerInspectionContract;
  execution_closure?: Record<string, unknown>;
  readiness?: { status: "ready" | "needs_connector_setup" | "needs_environment" };
  connector_readiness?: Record<string, unknown>[];
  environment?: {
    status: "ready" | "needs_environment";
    variables: { name: string; required: boolean; available: boolean }[];
    missing?: string[];
  };
  examples?: string[];
  resume?: { may_pause: boolean; command: string };
  entry_provenance?: Record<string, unknown>;
}>;

export const skillInspectionSchema = generatedSchema<SkillInspectionContract>(
  "skill-inspection.schema.json",
);

export function validateSkillInspectionContract(
  value: unknown,
  label = "skill_inspection",
): SkillInspectionContract {
  return validateContractSchema(skillInspectionSchema, value, label);
}
