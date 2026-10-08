import { Type, type Static } from "../internal.js";
import {
  JSON_SCHEMA_DRAFT_2020_12,
  RUNX_CONTROL_SCHEMA_REFS,
  type DeepReadonly,
  generatedSchema,
  unknownRecordSchema,
  validateContractSchema,
} from "../internal.js";
import { agentContextEnvelopeSchema } from "./context.js";

const agentActInvocationTypeSchema = Type.Object(
  {
    id: Type.String({ minLength: 1 }),
    role: Type.String({ minLength: 1 }),
    task: Type.String({ minLength: 1 }),
    envelope: agentContextEnvelopeSchema,
  },
  {
    $schema: JSON_SCHEMA_DRAFT_2020_12,
    $id: RUNX_CONTROL_SCHEMA_REFS.agent_act_invocation,
    additionalProperties: false,
  },
);

export type AgentActInvocationContract = DeepReadonly<Static<typeof agentActInvocationTypeSchema>>;

export const agentActInvocationSchema = generatedSchema<AgentActInvocationContract>(
  "agent-act-invocation.schema.json",
);

const questionTypeSchema = Type.Object(
  {
    id: Type.String({ minLength: 1 }),
    prompt: Type.String({ minLength: 1 }),
    description: Type.Optional(Type.String()),
    required: Type.Boolean(),
    type: Type.String({ minLength: 1 }),
  },
  {
    $schema: JSON_SCHEMA_DRAFT_2020_12,
    $id: RUNX_CONTROL_SCHEMA_REFS.question,
    additionalProperties: false,
  },
);

export type QuestionContract = DeepReadonly<Static<typeof questionTypeSchema>>;

export const questionSchema = generatedSchema<QuestionContract>("question.schema.json");

const approvalGateTypeSchema = Type.Object(
  {
    id: Type.String({ minLength: 1 }),
    reason: Type.String({ minLength: 1 }),
    type: Type.Optional(Type.String()),
    summary: Type.Optional(unknownRecordSchema()),
  },
  {
    $schema: JSON_SCHEMA_DRAFT_2020_12,
    $id: RUNX_CONTROL_SCHEMA_REFS.approval_gate,
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
