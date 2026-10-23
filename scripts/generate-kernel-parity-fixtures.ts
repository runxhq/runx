import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  evaluateRustKernelInputSync,
  type SequentialGraphEvent,
  type SequentialGraph,
  type SequentialGraphStepDefinition,
} from "./rust-kernel-eval.js";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const fixtureRoot = path.join(workspaceRoot, "fixtures", "kernel");
const schemaRoot = path.join(fixtureRoot, "schema");
const kernelFixtureSchemaRefs = {
  "../schema/policy.schema.json": "policy.schema.json",
  "../schema/state-machine.schema.json": "state-machine.schema.json",
} as const satisfies Record<KernelFixture["$schema"], string>;

export interface KernelFixture {
  readonly $schema: "../schema/state-machine.schema.json" | "../schema/policy.schema.json";
  readonly name: string;
  readonly description?: string;
  readonly input: Readonly<Record<string, unknown>> & { readonly kind: string };
  readonly expected:
    | {
        readonly kind: "output";
        readonly value: unknown;
      }
    | {
        readonly kind: "error";
        readonly code: string;
        readonly message?: string;
      };
}

export class KernelFixtureEvaluationError extends Error {
  readonly code = "kernel.fixture.evaluation_failed";
  readonly sourceErrorName?: string;
  readonly sourceErrorMessage?: string;

  constructor(error: unknown) {
    super("kernel fixture evaluation failed", { cause: error });
    this.name = "KernelFixtureEvaluationError";
    this.sourceErrorName = error instanceof Error ? error.name : undefined;
    this.sourceErrorMessage = error instanceof Error ? error.message : String(error);
  }
}

interface KernelFixtureCase {
  readonly name: string;
  readonly description?: string;
  readonly input: KernelFixture["input"];
  readonly expected?: KernelFixture["expected"];
}

interface ValidationResult {
  readonly valid: boolean;
  readonly errors: readonly string[];
}

type JsonSchema = Readonly<Record<string, unknown>>;

const supportedJsonSchemaKeywords = new Set([
  "$id",
  "$schema",
  "additionalProperties",
  "anyOf",
  "const",
  "items",
  "oneOf",
  "pattern",
  "properties",
  "required",
  "type",
]);

export function buildKernelParityFixtures(): readonly KernelFixture[] {
  return fixtureCases()
    .map(
      (fixtureCase) =>
        normalizeForFixture({
          $schema: fixtureCase.input.kind.startsWith("state-machine.")
            ? "../schema/state-machine.schema.json"
            : "../schema/policy.schema.json",
          name: fixtureCase.name,
          description: fixtureCase.description,
          input: fixtureCase.input,
          expected: fixtureCase.expected ?? {
            kind: "output",
            value: evaluateKernelFixtureInput(fixtureCase.input),
          },
        }) as KernelFixture,
    )
    .sort((left, right) => left.name.localeCompare(right.name));
}

export async function collectKernelFixtureFiles(root: string = fixtureRoot): Promise<readonly string[]> {
  const files: string[] = [];
  for (const directoryName of ["policy", "runner", "state-machine"]) {
    const directory = path.join(root, directoryName);
    let entries: readonly string[] = [];
    try {
      entries = await readdir(directory);
    } catch (error) {
      if (!isNodeError(error) || error.code !== "ENOENT") {
        throw error;
      }
      continue;
    }
    files.push(
      ...entries
        .filter((entry) => entry.endsWith(".json"))
        .map((entry) => path.join(directory, entry)),
    );
  }
  return files.sort();
}

export async function readKernelFixture(filePath: string): Promise<KernelFixture> {
  return JSON.parse(await readFile(filePath, "utf8")) as KernelFixture;
}

export async function validateKernelFixture(fixture: KernelFixture): Promise<ValidationResult> {
  const envelopeSchema = await readJsonSchema(path.join(schemaRoot, "fixture.schema.json"));
  const schemaFile = kernelFixtureSchemaFile(fixture.$schema);
  if (!schemaFile) {
    return {
      valid: false,
      errors: [`fixture.$schema: unsupported kernel fixture schema ref '${fixture.$schema}'`],
    };
  }
  const concreteSchema = await readJsonSchema(path.join(schemaRoot, schemaFile));
  const errors = [
    ...schemaErrors(envelopeSchema, fixture, "fixture.schema.json"),
    ...schemaErrors(concreteSchema, fixture, schemaFile),
    ...runnerFixtureErrors(fixture),
  ];
  return {
    valid: errors.length === 0,
    errors,
  };
}

export function isRunnerKernelFixture(fixture: KernelFixture): boolean {
  return fixture.expected.kind === "error" && fixture.expected.code === "kernel.fixture.evaluation_failed";
}

function kernelFixtureSchemaFile(schemaRef: unknown): string | undefined {
  return typeof schemaRef === "string" && Object.hasOwn(kernelFixtureSchemaRefs, schemaRef)
    ? kernelFixtureSchemaRefs[schemaRef as keyof typeof kernelFixtureSchemaRefs]
    : undefined;
}

function runnerFixtureErrors(fixture: KernelFixture): readonly string[] {
  const isRunnerFixture = isRunnerKernelFixture(fixture);
  if (isRunnerFixture && !fixture.name.startsWith("runner-")) {
    return [`fixture.name: runner ingestion fixtures must use the 'runner-' prefix`];
  }
  if (!isRunnerFixture && fixture.name.startsWith("runner-")) {
    return [`fixture.name: only kernel.fixture.evaluation_failed error fixtures may use the 'runner-' prefix`];
  }
  return [];
}

export function evaluateKernelFixtureInput(input: KernelFixture["input"]): unknown {
  try {
    return evaluateKernelFixtureInputUnchecked(input);
  } catch (error) {
    throw new KernelFixtureEvaluationError(error);
  }
}

function evaluateKernelFixtureInputUnchecked(input: KernelFixture["input"]): unknown {
  return evaluateRustKernelInputSync(input);
}

function createSequentialGraph(
  graphId: string,
  steps: readonly SequentialGraphStepDefinition[],
): SequentialGraph {
  return evaluateRustKernelInputSync({
    kind: "state-machine.createSequentialGraph",
    graphId,
    steps,
  }) as SequentialGraph;
}

function transitionSequentialGraph(
  state: SequentialGraph,
  event: SequentialGraphEvent,
): SequentialGraph {
  return evaluateRustKernelInputSync({
    kind: "state-machine.transitionSequentialGraph",
    state,
    event,
  }) as SequentialGraph;
}

export function normalizeForFixture(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map((item) => normalizeForFixture(item));
  }
  if (!isPlainRecord(value)) {
    return value;
  }

  const output: Record<string, unknown> = {};
  for (const key of Object.keys(value).sort()) {
    const normalized = normalizeForFixture(value[key]);
    if (normalized !== undefined) {
      output[key] = normalized;
    }
  }
  return output;
}

export function stableFixtureJson(value: unknown): string {
  return `${JSON.stringify(normalizeForFixture(value), null, 2)}\n`;
}

async function main(): Promise<void> {
  const check = process.argv.includes("--check");
  const fixtures = buildKernelParityFixtures();
  const expectedFiles = new Set<string>();

  for (const fixture of fixtures) {
    const directory = fixtureDirectory(fixture);
    const filePath = path.join(directory, `${fixture.name}.json`);
    expectedFiles.add(filePath);
    const content = stableFixtureJson(fixture);
    if (check) {
      let existing = "";
      try {
        existing = await readFile(filePath, "utf8");
      } catch (error) {
        if (isNodeError(error) && error.code === "ENOENT") {
          throw new Error(`missing fixture ${path.relative(workspaceRoot, filePath)}`);
        }
        throw error;
      }
      if (existing !== content) {
        throw new Error(`fixture is stale: ${path.relative(workspaceRoot, filePath)}`);
      }
      continue;
    }
    await mkdir(directory, { recursive: true });
    await writeFile(filePath, content);
  }

  if (check) {
    for (const filePath of await collectKernelFixtureFiles()) {
      if (!expectedFiles.has(filePath)) {
        throw new Error(`stale fixture file: ${path.relative(workspaceRoot, filePath)}`);
      }
    }
  }

  console.log(`${check ? "checked" : "generated"} ${fixtures.length} kernel parity fixtures`);
}

function fixtureCases(): readonly KernelFixtureCase[] {
  const linearSteps: readonly SequentialGraphStepDefinition[] = [
    { id: "first" },
    { id: "second", contextFrom: ["first"] },
  ];
  const pendingGraph = createSequentialGraph("gx_fixture", linearSteps);
  const startedGraph = transitionSequentialGraph(pendingGraph, {
    type: "start_step",
    stepId: "first",
    at: "2026-04-10T00:00:00.000Z",
  });
  const failedOnceGraph = transitionSequentialGraph(startedGraph, {
    type: "step_failed",
    stepId: "first",
    at: "2026-04-10T00:00:01.000Z",
    error: "boom",
  });
  return [
    {
      name: "scope-grant-exact-only-allows-exact",
      input: {
        kind: "policy.scopeGrantAllows",
        grantedScope: "admin:*",
        requestedScope: "admin:*",
        policy: "exact_only",
      },
    },
    {
      name: "scope-grant-exact-only-denies-wildcard-expansion",
      input: {
        kind: "policy.scopeGrantAllows",
        grantedScope: "admin:*",
        requestedScope: "admin:write",
        policy: "exact_only",
      },
    },
    {
      name: "scope-grant-delegated-allows-one-segment",
      input: {
        kind: "policy.scopeGrantAllows",
        grantedScope: "repo:*",
        requestedScope: "repo:read",
        policy: "delegated",
      },
    },
    {
      name: "scope-grant-delegated-denies-empty-namespace",
      input: {
        kind: "policy.scopeGrantAllows",
        grantedScope: ":*",
        requestedScope: ":read",
        policy: "delegated",
      },
      expected: { kind: "output", value: false },
    },
    {
      name: "scope-grant-delegated-denies-nested-segment",
      input: {
        kind: "policy.scopeGrantAllows",
        grantedScope: "repo:*",
        requestedScope: "repo:admin:keys",
        policy: "delegated",
      },
    },
    {
      name: "scope-grant-delegated-denies-universal",
      input: {
        kind: "policy.scopeGrantAllows",
        grantedScope: "*",
        requestedScope: "repo:read",
        policy: "delegated",
      },
    },
    {
      name: "scope-grant-trusted-allows-universal",
      input: {
        kind: "policy.scopeGrantAllows",
        grantedScope: "*",
        requestedScope: "repo:admin:keys",
        policy: "trusted",
      },
    },
    {
      name: "sequential-create-graph",
      input: {
        kind: "state-machine.createSequentialGraph",
        graphId: "gx_fixture",
        steps: linearSteps,
      },
    },
    {
      name: "sequential-plan-first-step",
      input: {
        kind: "state-machine.planSequentialGraphTransition",
        state: pendingGraph,
        steps: linearSteps,
      },
    },
    {
      name: "sequential-transition-step-succeeded",
      input: {
        kind: "state-machine.transitionSequentialGraph",
        state: startedGraph,
        event: {
          type: "step_succeeded",
          stepId: "first",
          at: "2026-04-10T00:00:01.000Z",
          receiptId: "rx_first",
          admissionWitness: {
            stepId: "first",
            receiptId: "rx_first",
          },
          outputs: {
            z: "last",
            a: "first",
          },
        },
      },
    },
    {
      name: "sequential-plan-fails-after-a-failed-step",
      input: {
        kind: "state-machine.planSequentialGraphTransition",
        state: failedOnceGraph,
        steps: [{ id: "first" }],
      },
    },
    {
      name: "runner-rejects-malformed-state",
      description:
        "Pins the fixture-runner ingestion error envelope for invalid but schema-shaped input; this is not a state-machine decision fixture.",
      input: {
        kind: "state-machine.planSequentialGraphTransition",
        state: {},
        steps: [{ id: "first" }],
      },
      expected: {
        kind: "error",
        code: "kernel.fixture.evaluation_failed",
        message: "kernel fixture evaluation failed",
      },
    },
  ];
}

function fixtureDirectory(fixture: KernelFixture): string {
  if (isRunnerKernelFixture(fixture)) {
    return path.join(fixtureRoot, "runner");
  }
  return path.join(fixtureRoot, fixture.input.kind.startsWith("state-machine.") ? "state-machine" : "policy");
}

async function readJsonSchema(filePath: string): Promise<JsonSchema> {
  return JSON.parse(await readFile(filePath, "utf8")) as JsonSchema;
}

function schemaErrors(schema: JsonSchema, value: unknown, schemaName: string): readonly string[] {
  return validateJsonSchemaValue(schema, value, "").map((error) => `${schemaName}${error.path}: ${error.message}`);
}

export function validateJsonSchemaValue(schema: unknown, value: unknown, pathPrefix: string): readonly { readonly path: string; readonly message: string }[] {
  if (!isPlainRecord(schema)) {
    return [{ path: pathPrefix || "/", message: "schema must be an object" }];
  }

  const keywordErrors = unsupportedKeywordErrors(schema, pathPrefix);
  const anyOf = Array.isArray(schema.anyOf) ? schema.anyOf : undefined;
  if (anyOf) {
    if (!anyOf.some((branch) => validateJsonSchemaValue(branch, value, pathPrefix).length === 0)) {
      return [...keywordErrors, { path: pathPrefix || "/", message: "value did not match any allowed schema branch" }];
    }
  }

  const oneOf = Array.isArray(schema.oneOf) ? schema.oneOf : undefined;
  if (oneOf) {
    const branchResults = oneOf.map((branch, index) => ({
      index,
      errors: validateJsonSchemaValue(branch, value, pathPrefix),
    }));
    const matchCount = branchResults.filter((result) => result.errors.length === 0).length;
    if (matchCount !== 1) {
      const branchSummary = branchResults
        .map((result) => {
          if (result.errors.length === 0) {
            return `branch ${result.index}: matched`;
          }
          const firstError = result.errors[0];
          return `branch ${result.index}: ${firstError?.path ?? (pathPrefix || "/")} ${firstError?.message ?? "did not match"}`;
        })
        .join("; ");
      return [
        ...keywordErrors,
        {
          path: pathPrefix || "/",
          message: `value matched ${matchCount} schema branches; expected exactly one (${branchSummary})`,
        },
      ];
    }
  }

  if ("const" in schema && !deepEqual(value, schema.const)) {
    return [...keywordErrors, { path: pathPrefix || "/", message: `value must equal ${JSON.stringify(schema.const)}` }];
  }

  if (typeof schema.pattern === "string" && typeof value === "string" && !(new RegExp(schema.pattern, "u")).test(value)) {
    return [...keywordErrors, { path: pathPrefix || "/", message: `string must match ${schema.pattern}` }];
  }

  const typeErrors = validateJsonSchemaType(schema.type, value, pathPrefix);
  if (typeErrors.length > 0) {
    return [...keywordErrors, ...typeErrors];
  }

  const errors: { path: string; message: string }[] = [...keywordErrors];
  if ((schema.type === "object" || isPlainRecord(schema.properties)) && isPlainRecord(value)) {
    const properties = isPlainRecord(schema.properties) ? schema.properties : {};
    const required = Array.isArray(schema.required) ? schema.required.filter((entry): entry is string => typeof entry === "string") : [];
    for (const requiredKey of required) {
      if (!Object.hasOwn(value, requiredKey)) {
        errors.push({ path: `${pathPrefix}/${requiredKey}`, message: "required property is missing" });
      }
    }
    for (const [key, entry] of Object.entries(value)) {
      const propertySchema = Object.hasOwn(properties, key) ? properties[key] : undefined;
      if (propertySchema) {
        errors.push(...validateJsonSchemaValue(propertySchema, entry, `${pathPrefix}/${key}`));
      } else if (schema.additionalProperties === false) {
        errors.push({ path: `${pathPrefix}/${key}`, message: "additional property is not allowed" });
      }
    }
  }

  if ((schema.type === "array" || schema.items !== undefined) && Array.isArray(value) && schema.items !== undefined) {
    value.forEach((item, index) => {
      errors.push(...validateJsonSchemaValue(schema.items, item, `${pathPrefix}/${index}`));
    });
  }

  return errors;
}

function unsupportedKeywordErrors(schema: JsonSchema, pathPrefix: string): readonly { readonly path: string; readonly message: string }[] {
  return Object.keys(schema)
    .filter((key) => !supportedJsonSchemaKeywords.has(key))
    .map((key) => ({
      path: `${pathPrefix}/${key}`,
      message: `unsupported JSON Schema keyword '${key}'`,
    }));
}

function validateJsonSchemaType(
  type: unknown,
  value: unknown,
  pathPrefix: string,
): readonly { readonly path: string; readonly message: string }[] {
  if (type === undefined) {
    return [];
  }
  const types = Array.isArray(type) ? type : [type];
  if (types.some((entry) => jsonSchemaTypeMatches(entry, value))) {
    return [];
  }
  return [{ path: pathPrefix || "/", message: `value must be ${types.join(" or ")}` }];
}

function jsonSchemaTypeMatches(type: unknown, value: unknown): boolean {
  switch (type) {
    case "array":
      return Array.isArray(value);
    case "boolean":
      return typeof value === "boolean";
    case "integer":
      return typeof value === "number" && Number.isInteger(value);
    case "null":
      return value === null;
    case "number":
      return typeof value === "number" && Number.isFinite(value);
    case "object":
      return isPlainRecord(value);
    case "string":
      return typeof value === "string";
    default:
      return false;
  }
}

function deepEqual(left: unknown, right: unknown): boolean {
  return JSON.stringify(normalizeForFixture(left)) === JSON.stringify(normalizeForFixture(right));
}

function expectRecord(value: unknown, field: string): Readonly<Record<string, unknown>> {
  if (!isPlainRecord(value)) {
    throw new Error(`${field} must be an object`);
  }
  return value;
}

function expectArray(value: unknown, field: string): readonly unknown[] {
  if (!Array.isArray(value)) {
    throw new Error(`${field} must be an array`);
  }
  return value;
}

function expectString(value: unknown, field: string): string {
  if (typeof value !== "string") {
    throw new Error(`${field} must be a string`);
  }
  return value;
}

function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error;
}

if (process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href === import.meta.url) {
  await main();
}
