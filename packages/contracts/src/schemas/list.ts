import { generatedSchema, generatedDefinition, validateContractSchema } from "../internal.js";
import type {
  RunxListItemContract,
  RunxListItemKindContract,
  RunxListReportContract,
  RunxListRequestedKindContract,
  RunxListSourceContract,
} from "../generated/contract-types.js";

export const listV1Schema = generatedSchema<RunxListReportContract>("list.schema.json");
export const runxListItemSchema = generatedDefinition<RunxListItemContract>("RunxListItem");
export const runxListRequestedKindSchema = generatedDefinition<RunxListRequestedKindContract>("RunxListRequestedKind");
export const runxListItemKindSchema = generatedDefinition<RunxListItemKindContract>("RunxListItemKind");
export const runxListSourceSchema = generatedDefinition<RunxListSourceContract>("RunxListSource");

export function validateRunxListReportContract(value: unknown, label = "list_report"): RunxListReportContract {
  return validateContractSchema(listV1Schema, value, label);
}
