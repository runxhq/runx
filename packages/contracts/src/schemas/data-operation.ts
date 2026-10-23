import { generatedSchema, validateContractSchema } from "../internal.js";
import type { DataOperationResultContract } from "../generated/contract-types.js";

export const dataOperationResultV1Schema = generatedSchema<DataOperationResultContract>(
  "data-operation-result.schema.json",
);

export function validateDataOperationResultContract(
  value: unknown,
  label = "data operation result",
): DataOperationResultContract {
  return validateContractSchema(dataOperationResultV1Schema, value, label);
}
