import { generatedSchema, validateContractSchema } from "../internal.js";
import type {
  ResolutionRequestContract,
  ResolutionResponseContract,
} from "../generated/contract-types.js";

export const resolutionRequestSchema = generatedSchema<ResolutionRequestContract>(
  "resolution-request.schema.json",
);

export const resolutionResponseSchema = generatedSchema<ResolutionResponseContract>(
  "resolution-response.schema.json",
);

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
