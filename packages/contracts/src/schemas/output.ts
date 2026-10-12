import { Type, type Static } from "../internal.js";
import {
  JSON_SCHEMA_DRAFT_2020_12,
  RUNX_CONTRACT_IDS,
  type DeepReadonly,
  generatedSchema,
  stringEnum,
  validateContractSchema,
} from "../internal.js";

const outputScalarKinds = [
  "string",
  "number",
  "integer",
  "boolean",
  "array",
  "object",
  "null",
] as const;

export const outputScalarSchema = stringEnum(outputScalarKinds);

export type OutputScalarContract = DeepReadonly<Static<typeof outputScalarSchema>>;

export const outputObjectEntrySchema = Type.Object(
  {
    type: Type.Optional(outputScalarSchema),
    description: Type.Optional(Type.String()),
    required: Type.Optional(Type.Boolean()),
    wrap_as: Type.Optional(Type.String({ minLength: 1 })),
    enum: Type.Optional(Type.Array(Type.String())),
  },
  {
    additionalProperties: false,
    minProperties: 1,
  },
);

export type OutputObjectEntryContract = DeepReadonly<Static<typeof outputObjectEntrySchema>>;

export const outputEntrySchema = Type.Union([
  outputScalarSchema,
  outputObjectEntrySchema,
]);

export type OutputEntryContract = DeepReadonly<Static<typeof outputEntrySchema>>;

const outputTypeSchema = Type.Record(
  Type.String(),
  outputEntrySchema,
  {
    $schema: JSON_SCHEMA_DRAFT_2020_12,
    $id: RUNX_CONTRACT_IDS.output,
  },
);

export type OutputContract = DeepReadonly<Static<typeof outputTypeSchema>>;

export const outputSchema = generatedSchema<OutputContract>("output.schema.json");

export function validateOutputContract(
  value: unknown,
  label = "output",
): OutputContract {
  return validateContractSchema(outputSchema, value, label);
}
