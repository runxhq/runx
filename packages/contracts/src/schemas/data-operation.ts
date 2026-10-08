import { Type, type Static } from "../internal.js";
import {
  JSON_SCHEMA_DRAFT_2020_12,
  RUNX_CONTRACT_IDS,
  RUNX_LOGICAL_SCHEMAS,
  type DeepReadonly,
  stringEnum,
  unknownRecordSchema,
  validateContractSchema,
} from "../internal.js";

export const dataOperationResultStatuses = [
  "committed",
  "idempotent_replay",
  "read",
  "conflict",
] as const;

export const dataOperationResultStatusSchema = stringEnum(dataOperationResultStatuses);

export const dataOperationStopConditionSchema = Type.Object(
  {
    code: Type.String({ minLength: 1 }),
    message: Type.String({ minLength: 1 }),
  },
  { additionalProperties: false },
);

export type DataOperationStopConditionContract =
  DeepReadonly<Static<typeof dataOperationStopConditionSchema>>;

export const dataOperationResultV1Schema = Type.Object(
  {
    schema: Type.Literal(RUNX_LOGICAL_SCHEMAS.dataOperationResult),
    data_source_ref: Type.String({ minLength: 1 }),
    provider: Type.String({ minLength: 1 }),
    operation: Type.String({ minLength: 1 }),
    resource: Type.String({ minLength: 1 }),
    aggregate_id: Type.String({ minLength: 1 }),
    status: dataOperationResultStatusSchema,
    before_version: Type.Integer({ minimum: 0 }),
    after_version: Type.Integer({ minimum: 0 }),
    idempotency_key: Type.Union([Type.String({ minLength: 1 }), Type.Null()]),
    event_ref: Type.Union([Type.String({ minLength: 1 }), Type.Null()]),
    event_digest: Type.Union([Type.String({ minLength: 1 }), Type.Null()]),
    result_digest: Type.String({ minLength: 1 }),
    projection_digest: Type.String({ minLength: 1 }),
    projection: Type.Union([unknownRecordSchema(), Type.Null()]),
    events: Type.Array(Type.Unknown()),
    rows: Type.Array(Type.Unknown()),
    redactions: Type.Array(Type.Unknown()),
    stop_conditions: Type.Array(dataOperationStopConditionSchema),
    provider_evidence: unknownRecordSchema(),
  },
  {
    $schema: JSON_SCHEMA_DRAFT_2020_12,
    $id: RUNX_CONTRACT_IDS.dataOperationResult,
    "x-runx-schema": RUNX_LOGICAL_SCHEMAS.dataOperationResult,
    additionalProperties: false,
  },
);

export type DataOperationResultContract =
  DeepReadonly<Static<typeof dataOperationResultV1Schema>>;

export function validateDataOperationResultContract(
  value: unknown,
  label = "data operation result",
): DataOperationResultContract {
  return validateContractSchema(dataOperationResultV1Schema, value, label) as DataOperationResultContract;
}
