import { generatedSchema, validateContractSchema } from "../internal.js";
import type {
  TurnInvocationContract,
  ApprovalGateContract,
  QuestionContract,
} from "../generated/contract-types.js";

export const turnInvocationSchema = generatedSchema<TurnInvocationContract>(
  "agent-act-invocation.schema.json",
);

export const questionSchema = generatedSchema<QuestionContract>("question.schema.json");

export const approvalGateSchema = generatedSchema<ApprovalGateContract>("approval-gate.schema.json");

export function validateTurnInvocationContract(
  value: unknown,
  label = "turn_invocation",
): TurnInvocationContract {
  return validateContractSchema(turnInvocationSchema, value, label);
}

export function validateQuestionContract(value: unknown, label = "question"): QuestionContract {
  return validateContractSchema(questionSchema, value, label);
}

export function validateApprovalGateContract(value: unknown, label = "approval_gate"): ApprovalGateContract {
  return validateContractSchema(approvalGateSchema, value, label);
}
