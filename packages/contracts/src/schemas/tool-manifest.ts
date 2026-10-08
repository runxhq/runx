import type { DeepReadonly, JsonSchema, UnknownRecord } from "../internal.js";
import { runxSchemaArtifacts } from "../schema-artifacts.js";

export type ToolCommandInputModeContract = "args" | "stdin" | "none";

/** A tool's executable: a trusted host process. */
export type ToolManifestSourceContract = DeepReadonly<{
  command: string;
  args?: readonly string[];
  cwd?: string;
  input_mode?: ToolCommandInputModeContract;
  timeout_seconds?: number;
  environment?: {
    required?: readonly string[];
    optional?: readonly string[];
  };
}>;

export type ToolManifestInputContract = DeepReadonly<{
  type: "array" | "boolean" | "integer" | "json" | "number" | "object" | "string";
  required: boolean;
  description?: string;
  default?: unknown;
  artifact?: boolean;
  packet?: string;
  schema?: UnknownRecord;
}>;

export type ToolManifestArtifactContract = DeepReadonly<{
  emits?: readonly string[];
  named_emits?: Readonly<Record<string, string>>;
  packets?: Readonly<Record<string, string>>;
  packet?: string;
  wrap_as?: string;
}>;

export type ToolIdempotencyPolicyContract = DeepReadonly<{
  key?: string;
}>;

export type ToolManifestContract = DeepReadonly<{
  schema: "runx.tool.manifest.v1";
  name: string;
  version?: string;
  description?: string;
  source: ToolManifestSourceContract;
  inputs?: Readonly<Record<string, ToolManifestInputContract>>;
  scopes?: readonly string[];
  artifacts?: ToolManifestArtifactContract;
  idempotency?: ToolIdempotencyPolicyContract;
}>;

export const toolManifestV1Schema = runxSchemaArtifacts[
  "tool-manifest.schema.json"
] as JsonSchema<ToolManifestContract>;
