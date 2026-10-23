import { generatedSchema, validateContractSchema } from "../internal.js";
import type { OperationalProposalContract } from "../generated/contract-types.js";

export const operationalProposalSchemaVersion = "runx.operational_proposal.v1" as const;

export const operationalProposalSchema = generatedSchema<OperationalProposalContract>(
  "operational-proposal.schema.json",
);

export function validateOperationalProposalContract(
  value: unknown,
  label = "operational_proposal",
): OperationalProposalContract {
  return validateContractSchema(operationalProposalSchema, value, label);
}
