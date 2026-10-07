import { Type, type Static } from "../internal.js";
import {
  JSON_SCHEMA_DRAFT_2020_12,
  RUNX_CONTRACT_IDS,
  RUNX_LOGICAL_SCHEMAS,
  type DeepReadonly,
  dateTimeStringSchema,
  generatedSchema,
  stringEnum,
  unknownRecordSchema,
  validateContractSchema,
} from "../internal.js";

const actAssignmentHostKinds = ["cli", "api", "comment", "system"] as const;

export const actAssignmentActorSchema = Type.Object(
  {
    actor_id: Type.Optional(Type.String()),
    display_name: Type.Optional(Type.String()),
    role: Type.Optional(Type.String()),
    provider_identity: Type.Optional(Type.String()),
  },
  {
    additionalProperties: false,
  },
);

export type ActAssignmentActorContract = DeepReadonly<Static<typeof actAssignmentActorSchema>>;

export const actAssignmentHostSchema = Type.Object(
  {
    kind: stringEnum(actAssignmentHostKinds),
    trigger_ref: Type.Optional(Type.String()),
    scope_set: Type.Optional(Type.Array(Type.String())),
    actor: Type.Optional(actAssignmentActorSchema),
  },
  {
    additionalProperties: false,
  },
);

export type ActAssignmentHostContract = DeepReadonly<Static<typeof actAssignmentHostSchema>>;

export const actAssignmentIdempotencySchema = Type.Object(
  {
    algorithm: Type.Literal("sha256"),
    intent_key: Type.String({ minLength: 1 }),
    trigger_key: Type.Optional(Type.String({ minLength: 1 })),
    content_hash: Type.String({ minLength: 1 }),
  },
  {
    additionalProperties: false,
  },
);

export type ActAssignmentIdempotencyContract = DeepReadonly<Static<typeof actAssignmentIdempotencySchema>>;

const actAssignmentV1TypeSchema = Type.Object(
  {
    schema: Type.Literal(RUNX_LOGICAL_SCHEMAS.actAssignment),
    skill_ref: Type.String({ minLength: 1 }),
    runner: Type.String({ minLength: 1 }),
    source_ref: Type.Optional(Type.String({ minLength: 1 })),
    requested_at: dateTimeStringSchema(),
    host: actAssignmentHostSchema,
    input_overrides: Type.Optional(unknownRecordSchema()),
    idempotency: actAssignmentIdempotencySchema,
  },
  {
    $schema: JSON_SCHEMA_DRAFT_2020_12,
    $id: RUNX_CONTRACT_IDS.actAssignment,
    "x-runx-schema": RUNX_LOGICAL_SCHEMAS.actAssignment,
    additionalProperties: false,
  },
);

export type ActAssignmentContract = DeepReadonly<Static<typeof actAssignmentV1TypeSchema>>;

export const actAssignmentV1Schema = generatedSchema<ActAssignmentContract>(
  "act-assignment.schema.json",
);

export function validateActAssignmentContract(
  value: unknown,
  label = "act_assignment",
): ActAssignmentContract {
  return validateContractSchema(actAssignmentV1Schema, value, label);
}
