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

const DEFINITIONS_FILE = "definitions.schema.json";

/**
 * The one definition of the named contract type `typeName`: the document it
 * is published as, or its entry in the definitions document, which defines
 * every named type that has no document of its own.
 */
export function generatedDefinition<TStatic>(typeName: string): JsonSchema<TStatic> {
  const own = Object.entries(runxSchemaArtifacts).find(
    ([fileName, schema]) => fileName !== DEFINITIONS_FILE && (schema as JsonSchema).title === typeName,
  );
  if (own) return own[1] as JsonSchema<TStatic>;
  const definitions = (runxSchemaArtifacts[DEFINITIONS_FILE] as JsonSchema).$defs as
    | Readonly<Record<string, JsonSchema>>
    | undefined;
  const definition = definitions?.[typeName];
  if (!definition) throw new Error(`no generated contract type is named ${typeName}`);
  return definition as JsonSchema<TStatic>;
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
  const id = canonicalSchema.$id;
  const published = typeof id === "string" && generatedSchemaById.get(id) === canonicalSchema
    ? ajv.getSchema(id)
    : undefined;
  const compiled = published ?? ajv.compile(normalizeSchemaForAjv(canonicalSchema));
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
  // A generated schema refers to the definitions it shares with the rest of
  // the published set, so every generated document resolves.
  for (const schema of generatedSchemaById.values()) ajv.addSchema(normalizeSchemaForAjv(schema));
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
