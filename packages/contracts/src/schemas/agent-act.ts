import { Type, type Static } from "../internal.js";
import {
  JSON_SCHEMA_DRAFT_2020_12,
  RUNX_CONTRACT_IDS,
  type DeepReadonly,
  generatedSchema,
  unknownRecordSchema,
  validateContractSchema,
} from "../internal.js";
import { agentContextEnvelopeSchema } from "./context.js";

const allowedOperationSchema = Type.Object(
  {
    requirement: Type.String({ minLength: 1 }),
    operation: Type.String({ minLength: 1 }),
  },
  { additionalProperties: false },
);

/** The lowest registry tier a catalog rule admits on its own. */
const trustTierSchema = Type.Union([
  Type.Literal("first_party"),
  Type.Literal("verified"),
  Type.Literal("community"),
]);

/**
 * What a load admits: exactly one of `catalog`, the registry's packages at a
 * tier or above, and `project: true`, the packages the project authored.
 */
const catalogRuleSchema = Type.Object(
  {
    catalog: Type.Optional(
      Type.Object({ trust: trustTierSchema }, { additionalProperties: false }),
    ),
    project: Type.Optional(Type.Boolean()),
  },
  { additionalProperties: false },
);

/** The calls an open turn admits: the skill's, or its runner's, declared limits. */
export const turnAllowanceSchema = Type.Object(
  {
    operations: Type.Optional(Type.Array(allowedOperationSchema)),
    tools: Type.Optional(Type.Array(Type.String({ minLength: 1 }))),
    /** Child skills by ref, and the catalog rules a load is admitted under. */
    skills: Type.Optional(
      Type.Array(Type.Union([Type.String({ minLength: 1 }), catalogRuleSchema])),
    ),
    asks: Type.Optional(Type.Boolean()),
  },
  { additionalProperties: false },
);

export type TurnAllowanceContract = DeepReadonly<Static<typeof turnAllowanceSchema>>;

const agentActInvocationTypeSchema = Type.Object(
  {
    id: Type.String({ minLength: 1 }),
    role: Type.String({ minLength: 1 }),
    task: Type.String({ minLength: 1 }),
    envelope: agentContextEnvelopeSchema,
    allowance: turnAllowanceSchema,
  },
  {
    $schema: JSON_SCHEMA_DRAFT_2020_12,
    $id: RUNX_CONTRACT_IDS.agentActInvocation,
    additionalProperties: false,
  },
);

export type AgentActInvocationContract = DeepReadonly<Static<typeof agentActInvocationTypeSchema>>;

export const agentActInvocationSchema = generatedSchema<AgentActInvocationContract>(
  "agent-act-invocation.schema.json",
);

/**
 * A question put to the person: `type` is `choice` when it offers options
 * (the recommended first) and `text` otherwise; `free_text` lets the person
 * answer in their own words. The person answers `{<id>: {option?, text?}}`.
 */
const questionTypeSchema = Type.Object(
  {
    id: Type.String({ minLength: 1 }),
    prompt: Type.String({ minLength: 1 }),
    description: Type.Optional(Type.String()),
    required: Type.Boolean(),
    type: Type.String({ minLength: 1 }),
    options: Type.Optional(
      Type.Array(
        Type.Object(
          {
            label: Type.String({ minLength: 1 }),
            detail: Type.Optional(Type.String()),
          },
          { additionalProperties: false },
        ),
      ),
    ),
    free_text: Type.Optional(Type.Boolean()),
  },
  {
    $schema: JSON_SCHEMA_DRAFT_2020_12,
    $id: RUNX_CONTRACT_IDS.question,
    additionalProperties: false,
  },
);

export type QuestionContract = DeepReadonly<Static<typeof questionTypeSchema>>;

export const questionSchema = generatedSchema<QuestionContract>("question.schema.json");

/** A project artifact holding a value too large to carry inline. */
const projectArtifactRefTypeSchema = Type.Object(
  {
    schema: Type.Literal("runx.project_artifact_ref.v1"),
    ref: Type.String(),
    digest: Type.String(),
    bytes: Type.Integer({ minimum: 0 }),
    media_type: Type.String(),
    path: Type.String(),
  },
  { additionalProperties: false },
);

/**
 * What a person approves. `summary.payload_digest` is the stable digest of
 * what executes; `payload` carries it inline up to 64 KiB, `payload_ref` as
 * a project artifact beyond, and a host recomputes the digest before it
 * offers Approve. Gate type `tool` is a local effect the floor holds.
 */
const approvalGateTypeSchema = Type.Object(
  {
    id: Type.String({ minLength: 1 }),
    reason: Type.String({ minLength: 1 }),
    type: Type.Optional(Type.String()),
    summary: Type.Optional(unknownRecordSchema()),
    payload: Type.Optional(Type.Unknown()),
    payload_ref: Type.Optional(projectArtifactRefTypeSchema),
  },
  {
    $schema: JSON_SCHEMA_DRAFT_2020_12,
    $id: RUNX_CONTRACT_IDS.approvalGate,
    additionalProperties: false,
  },
);

export type ApprovalGateContract = DeepReadonly<Static<typeof approvalGateTypeSchema>>;

export const approvalGateSchema = generatedSchema<ApprovalGateContract>("approval-gate.schema.json");

export function validateAgentActInvocationContract(
  value: unknown,
  label = "agent_act_invocation",
): AgentActInvocationContract {
  return validateContractSchema(agentActInvocationSchema, value, label);
}

export function validateQuestionContract(value: unknown, label = "question"): QuestionContract {
  return validateContractSchema(questionSchema, value, label);
}

export function validateApprovalGateContract(value: unknown, label = "approval_gate"): ApprovalGateContract {
  return validateContractSchema(approvalGateSchema, value, label);
}
