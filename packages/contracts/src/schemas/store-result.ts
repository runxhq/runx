import { generatedSchema, validateContractSchema } from "../internal.js";
import type { StoreResultContract } from "../generated/contract-types.js";

export const storeResultV1Schema = generatedSchema<StoreResultContract>(
  "store-result.schema.json",
);

export function validateStoreResultContract(
  value: unknown,
  label = "store result",
): StoreResultContract {
  return validateContractSchema(storeResultV1Schema, value, label);
}
