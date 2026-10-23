import { generatedSchema, validateContractSchema } from "../internal.js";
import type {
  ActResultEnvelopeContract,
  ResolutionRequestContract,
  ResolutionResponseContract,
} from "../generated/contract-types.js";

export const resolutionRequestSchema = generatedSchema<ResolutionRequestContract>(
  "resolution-request.schema.json",
);

/** What an answer's `choice` names to pay from the account's runx balance. */
export const PURCHASE_BALANCE_CHOICE = "runx:balance";

export const resolutionResponseSchema = generatedSchema<ResolutionResponseContract>(
  "resolution-response.schema.json",
);

export const actResultEnvelopeSchema = generatedSchema<ActResultEnvelopeContract>("act-result.schema.json");

export function validateResolutionRequestContract(
  value: unknown,
  label = "resolution_request",
): ResolutionRequestContract {
  return validateContractSchema(resolutionRequestSchema, value, label) as ResolutionRequestContract;
}

export function validateResolutionResponseContract(
  value: unknown,
  label = "resolution_response",
): ResolutionResponseContract {
  return validateContractSchema(resolutionResponseSchema, value, label) as ResolutionResponseContract;
}

export function validateActResultEnvelopeContract(
  value: unknown,
  label = "act_result",
): ActResultEnvelopeContract {
  return validateContractSchema(actResultEnvelopeSchema, value, label) as ActResultEnvelopeContract;
}
