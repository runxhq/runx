import { generatedSchema, generatedSchemaAt, validateContractSchema } from "../internal.js";
import type {
  RunxListItemContract,
  RunxListItemKindContract,
  RunxListReportContract,
  RunxListRequestedKindContract,
  RunxListSourceContract,
} from "../generated/contract-types.js";

export const listV1Schema = generatedSchema<RunxListReportContract>("list.schema.json");
export const runxListItemSchema = generatedSchemaAt<RunxListItemContract>(
  listV1Schema,
  ["properties", "items", "items"],
  "list.items[]",
);
export const runxListRequestedKindSchema = generatedSchemaAt<RunxListRequestedKindContract>(
  listV1Schema,
  ["properties", "requested_kind"],
  "list.requested_kind",
);
export const runxListItemKindSchema = generatedSchemaAt<RunxListItemKindContract>(
  runxListItemSchema,
  ["properties", "kind"],
  "list.items[].kind",
);
export const runxListSourceSchema = generatedSchemaAt<RunxListSourceContract>(
  runxListItemSchema,
  ["properties", "source"],
  "list.items[].source",
);

export function validateRunxListReportContract(value: unknown, label = "list_report"): RunxListReportContract {
  return validateContractSchema(listV1Schema, value, label);
}
