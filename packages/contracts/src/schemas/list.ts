import {
  RUNX_LOGICAL_SCHEMAS,
  type DeepReadonly,
  generatedSchema,
  generatedSchemaAt,
  validateContractSchema,
} from "../internal.js";

export type RunxListRequestedKindContract =
  | "all"
  | "tools"
  | "skills"
  | "graphs"
  | "packets"
  | "overlays";
export type RunxListItemKindContract = "tool" | "skill" | "graph" | "packet" | "overlay";
export type RunxListSourceContract = "local" | "workspace" | "dependencies" | "built-in";
export type RunxListStatusContract = "ok" | "invalid";
/** Whether a host may offer a skill; an internal one, such as a graph stage, runs only within its owner. */
export type RunxListVisibilityContract = "public" | "internal";

export type RunxListEmitContract = DeepReadonly<{
  name: string;
  packet?: string;
}>;

export type RunxListItemContract = DeepReadonly<{
  kind: RunxListItemKindContract;
  name: string;
  source: RunxListSourceContract;
  path: string;
  status: RunxListStatusContract;
  visibility?: RunxListVisibilityContract;
  description?: string;
  diagnostics?: readonly string[];
  scopes?: readonly string[];
  emits?: readonly RunxListEmitContract[];
  fixtures?: number;
  harness_cases?: number;
  steps?: number;
  wraps?: string;
}>;

export type RunxListReportContract = DeepReadonly<{
  schema: typeof RUNX_LOGICAL_SCHEMAS.list;
  root: string;
  requested_kind: RunxListRequestedKindContract;
  items: readonly RunxListItemContract[];
}>;

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
