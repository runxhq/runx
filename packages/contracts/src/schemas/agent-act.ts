import { generatedSchema, validateContractSchema } from "../internal.js";
import type {
  AgentActInvocationContract,
  ApprovalGateContract,
  QuestionContract,
} from "../generated/contract-types.js";

export const agentActInvocationSchema = generatedSchema<AgentActInvocationContract>(
  "agent-act-invocation.schema.json",
);

export const questionSchema = generatedSchema<QuestionContract>("question.schema.json");

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
