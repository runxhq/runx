import { generatedSchema, validateContractSchema } from "../internal.js";
import type { AgentContextEnvelopeContract } from "../generated/contract-types.js";

export const agentContextEnvelopeSchema = generatedSchema<AgentContextEnvelopeContract>(
  "agent-context-envelope.schema.json",
);

export function validateAgentContextEnvelopeContract(
  value: unknown,
  label = "agent_context_envelope",
): AgentContextEnvelopeContract {
  return validateContractSchema(agentContextEnvelopeSchema, value, label);
}
