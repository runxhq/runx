import { Ajv2020, type ErrorObject } from "ajv/dist/2020.js";
import {
  runxSchemaArtifacts,
  type RunxSchemaArtifactName,
} from "./schema-artifacts.js";

export type UnknownRecord = Readonly<Record<string, unknown>>;
export type DeepReadonly<T> =
  T extends (...args: never[]) => unknown ? T
    : T extends readonly (infer TValue)[] ? readonly DeepReadonly<TValue>[]
      : T extends (infer TValue)[] ? readonly DeepReadonly<TValue>[]
        : T extends object ? { readonly [TKey in keyof T]: DeepReadonly<T[TKey]> }
          : T;

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
