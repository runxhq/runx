import { Ajv2020, type ErrorObject } from "ajv/dist/2020.js";
import {
  runxSchemaArtifacts,
  type RunxSchemaArtifactName,
} from "./schema-artifacts.js";

export const JSON_SCHEMA_DRAFT_2020_12 = "https://json-schema.org/draft/2020-12/schema" as const;

export const RUNX_SCHEMA_BASE_URL = "https://schemas.runx.ai" as const;

export const RUNX_CONTRACT_IDS = {
  doctor: `${RUNX_SCHEMA_BASE_URL}/runx/doctor/v1.json`,
  dev: `${RUNX_SCHEMA_BASE_URL}/runx/dev/v1.json`,
  list: `${RUNX_SCHEMA_BASE_URL}/runx/list/v1.json`,
  runSummary: `${RUNX_SCHEMA_BASE_URL}/runx/run-summary/v1.json`,
  receipt: `${RUNX_SCHEMA_BASE_URL}/runx/receipt/v1.json`,
  effectFinalityReceipt: `${RUNX_SCHEMA_BASE_URL}/runx/effect-finality-receipt/v1.json`,
  fixture: `${RUNX_SCHEMA_BASE_URL}/runx/fixture/v1.json`,
  toolManifest: `${RUNX_SCHEMA_BASE_URL}/runx/tool/manifest/v1.json`,
  packetIndex: `${RUNX_SCHEMA_BASE_URL}/runx/packet/index/v1.json`,
  actAssignment: `${RUNX_SCHEMA_BASE_URL}/runx/act-assignment/v1.json`,
  externalAdapterManifest: `${RUNX_SCHEMA_BASE_URL}/runx/external-adapter/manifest/v1.json`,
  externalAdapterInvocation: `${RUNX_SCHEMA_BASE_URL}/runx/external-adapter/invocation/v1.json`,
  externalAdapterResponse: `${RUNX_SCHEMA_BASE_URL}/runx/external-adapter/response/v1.json`,
  externalAdapterHostResolution: `${RUNX_SCHEMA_BASE_URL}/runx/external-adapter/host-resolution/v1.json`,
  externalAdapterCancellation: `${RUNX_SCHEMA_BASE_URL}/runx/external-adapter/cancellation/v1.json`,
  externalAdapterCredentialRequest: `${RUNX_SCHEMA_BASE_URL}/runx/external-adapter/credential-request/v1.json`,
  credentialDeliveryProfile: `${RUNX_SCHEMA_BASE_URL}/runx/credential-delivery/profile/v1.json`,
  credentialDeliveryRequest: `${RUNX_SCHEMA_BASE_URL}/runx/credential-delivery/request/v1.json`,
  credentialDeliveryResponse: `${RUNX_SCHEMA_BASE_URL}/runx/credential-delivery/response/v1.json`,
  credentialDeliveryObservation: `${RUNX_SCHEMA_BASE_URL}/runx/credential-delivery/observation/v1.json`,
  threadOutboxProviderManifest: `${RUNX_SCHEMA_BASE_URL}/runx/thread-outbox-provider/manifest/v1.json`,
  threadOutboxProviderPush: `${RUNX_SCHEMA_BASE_URL}/runx/thread-outbox-provider/push/v1.json`,
  threadOutboxProviderFetch: `${RUNX_SCHEMA_BASE_URL}/runx/thread-outbox-provider/fetch/v1.json`,
  threadOutboxProviderObservation: `${RUNX_SCHEMA_BASE_URL}/runx/thread-outbox-provider/observation/v1.json`,
  dataOperationResult: `${RUNX_SCHEMA_BASE_URL}/runx/data/operation-result/v1.json`,
  reference: `${RUNX_SCHEMA_BASE_URL}/runx/reference/v1.json`,
  authority: `${RUNX_SCHEMA_BASE_URL}/runx/authority/v1.json`,
  authoritySubsetProof: `${RUNX_SCHEMA_BASE_URL}/runx/authority/subset-proof/v1.json`,
  signal: `${RUNX_SCHEMA_BASE_URL}/runx/signal/v1.json`,
  decision: `${RUNX_SCHEMA_BASE_URL}/runx/decision/v1.json`,
  act: `${RUNX_SCHEMA_BASE_URL}/runx/act/v1.json`,
  verification: `${RUNX_SCHEMA_BASE_URL}/runx/verification/v1.json`,
  artifact: `${RUNX_SCHEMA_BASE_URL}/runx/artifact/v1.json`,
  redaction: `${RUNX_SCHEMA_BASE_URL}/runx/redaction/v1.json`,
  ledgerEntry: `${RUNX_SCHEMA_BASE_URL}/runx/ledger-entry/v1.json`,
  handoffSignal: `${RUNX_SCHEMA_BASE_URL}/runx/handoff-signal/v1.json`,
  handoffState: `${RUNX_SCHEMA_BASE_URL}/runx/handoff-state/v1.json`,
  suppressionRecord: `${RUNX_SCHEMA_BASE_URL}/runx/suppression-record/v1.json`,
  operationalPolicy: `${RUNX_SCHEMA_BASE_URL}/runx/operational-policy/v1.json`,
  operationalProposal: `${RUNX_SCHEMA_BASE_URL}/runx/operational-proposal/v1.json`,
  externalJobContinuation: `${RUNX_SCHEMA_BASE_URL}/runx/external-job-continuation/v1.json`,
  externalJobSchedule: `${RUNX_SCHEMA_BASE_URL}/runx/external-job-schedule/v1.json`,
  externalJobScheduleIntent: `${RUNX_SCHEMA_BASE_URL}/runx/external-job-schedule-intent/v1.json`,
  externalJobStageRequest: `${RUNX_SCHEMA_BASE_URL}/runx/external-job-stage/request/v1.json`,
  externalJobStageResult: `${RUNX_SCHEMA_BASE_URL}/runx/external-job-stage/result/v1.json`,
  paidSkillListing: `${RUNX_SCHEMA_BASE_URL}/runx/marketplace/paid-skill-listing/v1.json`,
  paidInvocation: `${RUNX_SCHEMA_BASE_URL}/runx/payment/paid-invocation/v1.json`,
  offerRevisionRef: `${RUNX_SCHEMA_BASE_URL}/runx/payment/offer-revision-ref/v1.json`,
  parentInvocationBinding: `${RUNX_SCHEMA_BASE_URL}/runx/payment/parent-invocation-binding/v1.json`,
  quotePaidInvocationRequest: `${RUNX_SCHEMA_BASE_URL}/runx/payment/quote-paid-invocation/request/v1.json`,
  quotePaidInvocationResult: `${RUNX_SCHEMA_BASE_URL}/runx/payment/quote-paid-invocation/result/v1.json`,
  executePaidInvocationRequest: `${RUNX_SCHEMA_BASE_URL}/runx/payment/execute-paid-invocation/request/v1.json`,
  executePaidInvocationResult: `${RUNX_SCHEMA_BASE_URL}/runx/payment/execute-paid-invocation/result/v1.json`,
  getPaidInvocationRequest: `${RUNX_SCHEMA_BASE_URL}/runx/payment/get-paid-invocation/request/v1.json`,
  getPaidInvocationResult: `${RUNX_SCHEMA_BASE_URL}/runx/payment/get-paid-invocation/result/v1.json`,
  cancelPaidInvocationRequest: `${RUNX_SCHEMA_BASE_URL}/runx/payment/cancel-paid-invocation/request/v1.json`,
  cancelPaidInvocationResult: `${RUNX_SCHEMA_BASE_URL}/runx/payment/cancel-paid-invocation/result/v1.json`,
} as const;

export const RUNX_LOGICAL_SCHEMAS = {
  doctor: "runx.doctor.v1",
  dev: "runx.dev.v1",
  list: "runx.list.v1",
  runSummary: "runx.run-summary.v1",
  receipt: "runx.receipt.v1",
  effectFinalityReceipt: "runx.effect_finality_receipt.v1",
  fixture: "runx.fixture.v1",
  toolManifest: "runx.tool.manifest.v1",
  packetIndex: "runx.packet.index.v1",
  actAssignment: "runx.act_assignment.v1",
  externalAdapterManifest: "runx.external_adapter.manifest.v1",
  externalAdapterInvocation: "runx.external_adapter.invocation.v1",
  externalAdapterResponse: "runx.external_adapter.response.v1",
  externalAdapterHostResolution: "runx.external_adapter.host_resolution.v1",
  externalAdapterCancellation: "runx.external_adapter.cancellation.v1",
  externalAdapterCredentialRequest: "runx.external_adapter.credential_request.v1",
  credentialDeliveryProfile: "runx.credential_delivery.profile.v1",
  credentialDeliveryRequest: "runx.credential_delivery.request.v1",
  credentialDeliveryResponse: "runx.credential_delivery.response.v1",
  credentialDeliveryObservation: "runx.credential_delivery.observation.v1",
  threadOutboxProviderManifest: "runx.thread_outbox_provider.manifest.v1",
  threadOutboxProviderPush: "runx.thread_outbox_provider.push.v1",
  threadOutboxProviderFetch: "runx.thread_outbox_provider.fetch.v1",
  threadOutboxProviderObservation: "runx.thread_outbox_provider.observation.v1",
  dataOperationResult: "runx.data.operation_result.v1",
  reference: "runx.reference.v1",
  authority: "runx.authority.v1",
  authoritySubsetProof: "runx.authority_subset_proof.v1",
  signal: "runx.signal.v1",
  decision: "runx.decision.v1",
  act: "runx.act.v1",
  verification: "runx.verification.v1",
  artifact: "runx.artifact.v1",
  redaction: "runx.redaction.v1",
  ledgerEntry: "runx.ledger.entry.v1",
  handoffSignal: "runx.handoff_signal.v1",
  handoffState: "runx.handoff_state.v1",
  suppressionRecord: "runx.suppression_record.v1",
  operationalPolicy: "runx.operational_policy.v1",
  operationalProposal: "runx.operational_proposal.v1",
  externalJobContinuation: "runx.external_job_continuation.v1",
  externalJobSchedule: "runx.external_job_schedule.v1",
  externalJobScheduleIntent: "runx.external_job_schedule_intent.v1",
  externalJobStageRequest: "runx.external_job_stage.request.v1",
  externalJobStageResult: "runx.external_job_stage.result.v1",
  paidSkillListing: "runx.marketplace.paid_skill_listing.v1",
  paidInvocation: "runx.payment.paid_invocation.v1",
  offerRevisionRef: "runx.payment.offer_revision_ref.v1",
  parentInvocationBinding: "runx.payment.parent_invocation_binding.v1",
  quotePaidInvocationRequest: "runx.payment.quote_paid_invocation.request.v1",
  quotePaidInvocationResult: "runx.payment.quote_paid_invocation.result.v1",
  executePaidInvocationRequest: "runx.payment.execute_paid_invocation.request.v1",
  executePaidInvocationResult: "runx.payment.execute_paid_invocation.result.v1",
  getPaidInvocationRequest: "runx.payment.get_paid_invocation.request.v1",
  getPaidInvocationResult: "runx.payment.get_paid_invocation.result.v1",
  cancelPaidInvocationRequest: "runx.payment.cancel_paid_invocation.request.v1",
  cancelPaidInvocationResult: "runx.payment.cancel_paid_invocation.result.v1",
} as const;

export const RUNX_CONTROL_SCHEMA_REFS = {
  output: "https://runx.ai/spec/output.schema.json",
  agent_context_envelope: "https://runx.ai/spec/agent-context-envelope.schema.json",
  agent_act_invocation: "https://runx.ai/spec/agent-act-invocation.schema.json",
  question: "https://runx.ai/spec/question.schema.json",
  approval_gate: "https://runx.ai/spec/approval-gate.schema.json",
  resolution_request: "https://runx.ai/spec/resolution-request.schema.json",
  resolution_response: "https://runx.ai/spec/resolution-response.schema.json",
  act_result: "https://runx.ai/spec/act-result.schema.json",
  credential_envelope: "https://runx.ai/spec/credential-envelope.schema.json",
  scope_admission: "https://runx.ai/spec/scope-admission.schema.json",
  authority_proof: "https://runx.ai/spec/authority-proof.schema.json",
} as const;

export const RUNX_AUXILIARY_SCHEMA_IDS = {
  registryBinding: "https://runx.ai/schemas/registry-binding.schema.json",
  reviewReceiptOutput: "https://runx.ai/schemas/review-receipt-output.schema.json",
} as const;

export type UnknownRecord = Readonly<Record<string, unknown>>;
export type DeepReadonly<T> =
  T extends (...args: never[]) => unknown ? T
    : T extends readonly (infer TValue)[] ? readonly DeepReadonly<TValue>[]
      : T extends (infer TValue)[] ? readonly DeepReadonly<TValue>[]
        : T extends object ? { readonly [TKey in keyof T]: DeepReadonly<T[TKey]> }
          : T;

const optionalSchema = Symbol("runx.optional_schema");

export type JsonSchema<TStatic = unknown> = Record<string, unknown> & { readonly __runxStatic?: TStatic };
export type Static<TSchemaValue> = TSchemaValue extends JsonSchema<infer TValue> ? TValue : unknown;

export function generatedSchema<TStatic>(fileName: RunxSchemaArtifactName): JsonSchema<TStatic> {
  return runxSchemaArtifacts[fileName] as JsonSchema<TStatic>;
}

export function generatedSchemaAt<TStatic>(
  schema: JsonSchema,
  path: readonly (string | number)[],
  label: string,
): JsonSchema<TStatic> {
  let current: unknown = schema;
  for (const segment of path) {
    if (
      current === null
      || typeof current !== "object"
      || !(segment in current)
    ) {
      throw new Error(`generated schema fragment not found: ${label}`);
    }
    current = (current as Record<string | number, unknown>)[segment];
  }
  if (current === null || typeof current !== "object") {
    throw new Error(`generated schema fragment is not an object: ${label}`);
  }
  return current as JsonSchema<TStatic>;
}

type AnySchema = JsonSchema<any>;
type SchemaWithOptional<TStatic = unknown> = JsonSchema<TStatic> & { readonly [optionalSchema]: true };
type OptionalKeys<TProperties extends Record<string, AnySchema>> = {
  [TKey in keyof TProperties]: TProperties[TKey] extends { readonly [optionalSchema]: true } ? TKey : never;
}[keyof TProperties];
type RequiredKeys<TProperties extends Record<string, AnySchema>> = Exclude<keyof TProperties, OptionalKeys<TProperties>>;
type ObjectStatic<TProperties extends Record<string, AnySchema>> = {
  [TKey in RequiredKeys<TProperties>]: Static<TProperties[TKey]>;
} & {
  [TKey in OptionalKeys<TProperties>]?: Static<TProperties[TKey]>;
};
type UnionStatic<TSchemas extends readonly AnySchema[]> = Static<TSchemas[number]>;
function schemaWith<TStatic>(options: Record<string, unknown>, base: JsonSchema): JsonSchema<TStatic> {
  return (Object.keys(options).length > 0 ? { ...base, ...options } : base) as JsonSchema<TStatic>;
}

function cloneSchema<TStatic>(schema: JsonSchema<TStatic>): JsonSchema<TStatic> {
  return { ...schema };
}

function jsonTypeForLiteral(value: unknown): string | undefined {
  switch (typeof value) {
    case "string":
      return "string";
    case "number":
      return Number.isInteger(value) ? "integer" : "number";
    case "boolean":
      return "boolean";
    default:
      return value === null ? "null" : undefined;
  }
}

export const Type = {
  Array<TItems extends AnySchema>(items: TItems, options: Record<string, unknown> = {}): JsonSchema<Static<TItems>[]> {
    return schemaWith<Static<TItems>[]>(options, { type: "array", items });
  },

  Boolean(options: Record<string, unknown> = {}): JsonSchema<boolean> {
    return schemaWith<boolean>(options, { type: "boolean" });
  },

  Integer(options: Record<string, unknown> = {}): JsonSchema<number> {
    return schemaWith<number>(options, { type: "integer" });
  },

  Literal<const TValue>(value: TValue, options: Record<string, unknown> = {}): JsonSchema<TValue> {
    const literalType = jsonTypeForLiteral(value);
    const schema = literalType ? { const: value, type: literalType } : { const: value };
    return schemaWith<TValue>(options, schema);
  },

  Null(options: Record<string, unknown> = {}): JsonSchema<null> {
    return schemaWith<null>(options, { type: "null" });
  },

  Number(options: Record<string, unknown> = {}): JsonSchema<number> {
    return schemaWith<number>(options, { type: "number" });
  },

  Object<TProperties extends Record<string, AnySchema>>(
    properties: TProperties,
    options: Record<string, unknown> = {},
  ): JsonSchema<ObjectStatic<TProperties>> {
    const normalizedProperties: Record<string, JsonSchema> = {};
    const required: string[] = [];
    for (const [key, propertySchema] of Object.entries(properties)) {
      normalizedProperties[key] = cloneSchema(propertySchema);
      if (!(propertySchema as SchemaWithOptional)[optionalSchema]) {
        required.push(key);
      }
    }

    return schemaWith<ObjectStatic<TProperties>>(options, {
      type: "object",
      properties: normalizedProperties,
      ...(required.length > 0 ? { required } : {}),
    });
  },

  Optional<TSchema extends AnySchema>(schema: TSchema): SchemaWithOptional<Static<TSchema>> {
    return { ...schema, [optionalSchema]: true } as SchemaWithOptional<Static<TSchema>>;
  },

  Record<TValues extends AnySchema>(
    _keys: JsonSchema,
    values: TValues,
    options: Record<string, unknown> = {},
  ): JsonSchema<Record<string, Static<TValues>>> {
    return schemaWith<Record<string, Static<TValues>>>(options, {
      type: "object",
      additionalProperties: values,
    });
  },

  Ref<TSchema extends AnySchema>(schema: TSchema, options: Record<string, unknown> = {}): JsonSchema<Static<TSchema>> {
    const id = schema.$id;
    if (typeof id !== "string" || id.length === 0) {
      throw new Error("Referenced schema must have a non-empty $id.");
    }
    return schemaWith<Static<TSchema>>(options, { $ref: id });
  },

  String(options: Record<string, unknown> = {}): JsonSchema<string> {
    return schemaWith<string>(options, { type: "string" });
  },

  Union<TSchemas extends readonly AnySchema[]>(
    schemas: TSchemas,
    options: Record<string, unknown> = {},
  ): JsonSchema<UnionStatic<TSchemas>> {
    return schemaWith<UnionStatic<TSchemas>>(options, { anyOf: [...schemas] });
  },

  Unknown(options: Record<string, unknown> = {}): JsonSchema<unknown> {
    return schemaWith<unknown>(options, {});
  },

  KeyOf<const TSchema extends JsonSchema>(
    schema: TSchema,
    options: Record<string, unknown> = {},
  ): JsonSchema<string> {
    const properties = schema.properties;
    if (!properties || typeof properties !== "object" || Array.isArray(properties)) {
      throw new Error("KeyOf requires an object schema with properties.");
    }
    return schemaWith(options, {
      type: "string",
      enum: Object.keys(properties),
    });
  },
} as const;

export function stringEnum<const TValue extends readonly string[]>(
  values: TValue,
  options: Record<string, unknown> = {},
) {
  const properties = Object.fromEntries(
    values.map((value) => [value, Type.Null()]),
  ) as Record<TValue[number], JsonSchema>;
  return Type.KeyOf(
    Type.Object(properties, { additionalProperties: false }),
    options,
  );
}

export function unknownRecordSchema(options: Record<string, unknown> = {}) {
  return Type.Record(Type.String(), Type.Unknown(), options);
}

export function dateTimeStringSchema(options: Record<string, unknown> = {}) {
  return Type.String({
    minLength: 1,
    pattern: "^\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}(?:\\.\\d+)?Z$",
    ...options,
  });
}

export function validateContractSchema<TSchemaValue extends JsonSchema>(
  schema: TSchemaValue,
  value: unknown,
  label: string,
  references: readonly JsonSchema[] = [],
): Static<TSchemaValue> {
  const canonicalSchema = schemaWithGeneratedArtifact(schema);
  const validate = contractValidator(canonicalSchema, references);
  if (validate(value)) {
    return value as Static<TSchemaValue>;
  }
  const firstError = validate.errors?.[0];
  const schemaRef = typeof canonicalSchema.$id === "string" ? canonicalSchema.$id : "contract schema";
  const path = firstError ? formatAjvErrorPath(label, firstError) : label;
  throw new Error(`${path} must match ${schemaRef}.`);
}

export interface PrecompiledContractValidator {
  (value: unknown): boolean;
  readonly errors?: readonly ErrorObject[] | null;
}

export function validatePrecompiledContract<TSchemaValue extends JsonSchema>(
  schema: TSchemaValue,
  value: unknown,
  label: string,
  validate: PrecompiledContractValidator,
): Static<TSchemaValue> {
  if (validate(value)) {
    return value as Static<TSchemaValue>;
  }
  const canonicalSchema = schemaWithGeneratedArtifact(schema);
  const firstError = validate.errors?.[0];
  const schemaRef = typeof canonicalSchema.$id === "string" ? canonicalSchema.$id : "contract schema";
  const errorPath = firstError ? formatAjvErrorPath(label, firstError) : label;
  throw new Error(`${errorPath} must match ${schemaRef}.`);
}

export function contractSchemaMatches(
  schema: JsonSchema,
  value: unknown,
  references: readonly JsonSchema[] = [],
): boolean {
  return contractValidator(schemaWithGeneratedArtifact(schema), references)(value) === true;
}

export function validateContractSchemaForDiagnostics(
  schema: JsonSchema,
  value: unknown,
  references: readonly JsonSchema[] = [],
): readonly string[] {
  const validate = contractValidator(schemaWithGeneratedArtifact(schema), references);
  if (validate(value)) {
    return [];
  }
  return (validate.errors ?? []).map((error) => error.instancePath || error.message || error.keyword);
}

const contractValidators = new WeakMap<
  JsonSchema,
  Map<string, PrecompiledContractValidator>
>();
const contractSchemaIdentities = new WeakMap<JsonSchema, number>();
let nextContractSchemaIdentity = 1;

function contractValidator(
  canonicalSchema: JsonSchema,
  references: readonly JsonSchema[],
): PrecompiledContractValidator {
  const canonicalReferences = references.map(schemaWithGeneratedArtifact);
  const referenceKey = canonicalReferences
    .map((reference) => contractSchemaIdentity(reference))
    .join(":");
  let byReferenceSet = contractValidators.get(canonicalSchema);
  if (!byReferenceSet) {
    byReferenceSet = new Map();
    contractValidators.set(canonicalSchema, byReferenceSet);
  }
  const cached = byReferenceSet.get(referenceKey);
  if (cached) return cached;

  const ajv = createContractAjv(canonicalReferences);
  const compiled = ajv.compile(normalizeSchemaForAjv(canonicalSchema));
  byReferenceSet.set(referenceKey, compiled);
  return compiled;
}

function contractSchemaIdentity(schema: JsonSchema): number {
  const existing = contractSchemaIdentities.get(schema);
  if (existing !== undefined) return existing;
  const identity = nextContractSchemaIdentity;
  nextContractSchemaIdentity += 1;
  contractSchemaIdentities.set(schema, identity);
  return identity;
}

function createContractAjv(references: readonly JsonSchema[] = []) {
  const ajv = new Ajv2020({
    allErrors: false,
    strict: false,
    validateSchema: false,
  });
  for (const reference of references.map(schemaWithGeneratedArtifact)) {
    const id = reference.$id;
    if (typeof id === "string" && id.length > 0 && !ajv.getSchema(id)) {
      ajv.addSchema(normalizeSchemaForAjv(reference), id);
    }
  }
  return ajv;
}

const generatedSchemaById = new Map(
  Object.values(runxSchemaArtifacts).flatMap((schema) => {
    const id = schema.$id;
    return typeof id === "string" && id.length > 0 ? [[id, schema as JsonSchema]] : [];
  }),
);

function schemaWithGeneratedArtifact<TSchemaValue extends JsonSchema>(schema: TSchemaValue): TSchemaValue {
  const id = schema.$id;
  if (typeof id !== "string" || id.length === 0) {
    return schema;
  }
  return (generatedSchemaById.get(id) ?? schema) as TSchemaValue;
}

function normalizeSchemaForAjv(schema: JsonSchema): JsonSchema {
  return stripNestedSchemaIdentities(schema, true) as JsonSchema;
}

function stripNestedSchemaIdentities(value: unknown, isRoot: boolean): unknown {
  if (Array.isArray(value)) {
    return value.map((entry) => stripNestedSchemaIdentities(entry, false));
  }
  if (!value || typeof value !== "object") {
    return value;
  }
  const output: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (!isRoot && (key === "$id" || key === "$schema")) {
      continue;
    }
    output[key] = stripNestedSchemaIdentities(entry, false);
  }
  return output;
}

function formatAjvErrorPath(label: string, error: ErrorObject): string {
  const path = error.instancePath ? `${label}${formatSchemaErrorPath(error.instancePath)}` : label;
  if (
    error.keyword === "required"
    && typeof error.params === "object"
    && error.params
    && "missingProperty" in error.params
    && typeof error.params.missingProperty === "string"
  ) {
    return `${path}.${error.params.missingProperty}`;
  }
  return path;
}

export function formatSchemaErrorPath(path: string): string {
  const segments = path.split("/").filter((segment) => segment.length > 0);
  return segments.map((segment) => {
    const decoded = segment.replace(/~1/g, "/").replace(/~0/g, "~");
    return /^\d+$/u.test(decoded) ? `[${decoded}]` : `.${decoded}`;
  }).join("");
}

export function asUnknownRecord(value: unknown): UnknownRecord | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? value as UnknownRecord : undefined;
}
