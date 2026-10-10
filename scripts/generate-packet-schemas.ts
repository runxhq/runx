import { spawnSync } from "node:child_process";
import { readFile, readdir, unlink, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  listToolPacketDeclarations,
  parsePacketSchemaDocumentsBatch,
  validateRunnerManifestYamlBatch,
} from "./lib/native-parser.mjs";

type JsonObject = Record<string, unknown>;

interface PacketContract {
  readonly packetId: string;
  readonly source: string;
  readonly schema: JsonObject;
}

interface ExistingPacketSchema {
  readonly path: string;
  readonly generated: boolean;
  readonly schema: JsonObject;
}

interface ArtifactContract {
  readonly packet?: string;
  readonly packets?: Readonly<Record<string, string>>;
  readonly wrap_as?: string;
}

interface ExecutionGraph {
  readonly steps: readonly GraphStep[];
}

interface GraphStep {
  readonly id: string;
  readonly kind: Readonly<Record<string, unknown>>;
  readonly outputs?: JsonObject;
  readonly artifacts?: ArtifactContract;
}

interface RunnerDefinition {
  readonly name: string;
  readonly inputs?: Readonly<Record<string, InputDefinition>>;
  readonly graph: ExecutionGraph;
}

interface InputDefinition {
  readonly packet?: string;
}

interface RunnerManifest {
  readonly runners: Readonly<Record<string, RunnerDefinition>>;
}

interface ParsedPacketSchema {
  readonly packetId: string;
  readonly value: JsonObject;
  readonly sha256: string;
}

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const skillsRoot = path.join(workspaceRoot, "skills");
const packetRoot = path.join(workspaceRoot, "dist", "packets");
const check = process.argv.includes("--check");
ensureCurrentNativeRunx();
const contracts = new Map<string, PacketContract>();
const ownedContracts = await ownedPacketContracts();
const manualContracts: PacketContract[] = [];
const declarations = new Map<string, string>();
const existingById = await existingSchemas();

const profilePaths = await findProfiles(skillsRoot);
const profileSources = await Promise.all(profilePaths.map((profilePath) => readFile(profilePath, "utf8")));
const profiles = validateRunnerManifestYamlBatch(profileSources) as RunnerManifest[];
for (const [index, profilePath] of profilePaths.entries()) {
  const profile = profiles[index];
  if (!profile) throw new Error(`native parser omitted ${path.relative(workspaceRoot, profilePath)}`);
  collectManifestContracts(profile, path.relative(workspaceRoot, profilePath));
}
for (const declaration of listToolPacketDeclarations()) {
  if (!declarations.has(declaration.packetId)) {
    declarations.set(declaration.packetId, declaration.source);
  }
}
// Public native boundary packets have no run.yaml producer to discover. The
// Rust artifact owner marks those explicitly; ordinary contract schemas are
// never promoted merely because they exist.
for (const contract of ownedContracts.values()) {
  if (contract.schema["x-runx-packet"] === true) contracts.set(contract.packetId, contract);
}
for (const packetId of declarations.keys()) {
  const owned = ownedContracts.get(packetId);
  if (owned) contracts.set(packetId, owned);
}

const manualSchemaFindings: string[] = [];
for (const contract of manualContracts) {
  const ownedSchema = ownedContracts.get(contract.packetId)?.schema
    ?? existingById.get(contract.packetId)?.schema;
  if (!ownedSchema) throw new Error(`owned packet schema '${contract.packetId}' was not found`);
  manualSchemaFindings.push(
    ...structuralFloorFindings(ownedSchema, contract.schema, contract.packetId, contract.source),
  );
}
const manualContractsByPacket = new Map<string, PacketContract[]>();
for (const contract of manualContracts) {
  const packetContracts = manualContractsByPacket.get(contract.packetId) ?? [];
  packetContracts.push(contract);
  manualContractsByPacket.set(contract.packetId, packetContracts);
}
for (const [packetId, packetContracts] of manualContractsByPacket) {
  // A packet-bound `object` output is intentionally opaque: the referenced
  // manual packet schema validates its contents at the artifact boundary. It
  // must not be treated as an alternate envelope with no required fields.
  const shapedContracts = packetContracts.filter((contract) => {
    const view = schemaView(contract.schema, contract.schema, new Set());
    return view.required.size > 0 || view.properties.size > 0;
  });
  const requiredSets = shapedContracts.map((contract) => new Set(stringArray(contract.schema.required)));
  const shapes = new Set(requiredSets.map((required) => [...required].sort().join("\u0000")));
  if (shapes.size < 2) continue;
  const commonRequired = requiredSets.slice(1).reduce(
    (common, required) => new Set([...common].filter((field) => required.has(field))),
    new Set(requiredSets[0] ?? []),
  );
  const ownedSchema = ownedContracts.get(packetId)?.schema ?? existingById.get(packetId)?.schema;
  if (!ownedSchema) throw new Error(`owned packet schema '${packetId}' was not found`);
  for (const field of schemaView(ownedSchema, ownedSchema, new Set()).required) {
    if (!commonRequired.has(field)) {
      manualSchemaFindings.push(
        `manual packet schema '${packetId}' unconditionally requires root property '${field}', but its run.yaml bindings use incompatible envelope shapes`,
      );
    }
  }
}
const structuralContracts = new Map(contracts);
for (const [packetId, source] of declarations) {
  if (structuralContracts.has(packetId)) continue;
  const existing = existingById.get(packetId);
  if (existing && !existing.generated) {
    structuralContracts.set(packetId, { packetId, source, schema: existing.schema });
  }
}
for (const contract of structuralContracts.values()) {
  const effectiveSchema = existingById.get(contract.packetId)?.generated === false
    ? existingById.get(contract.packetId)?.schema ?? contract.schema
    : contract.schema;
  for (const schemaPath of ambiguousObjectSchemaPaths(effectiveSchema)) {
    manualSchemaFindings.push(
      `packet schema '${contract.packetId}' from ${contract.source} declares an object without a shape at ${schemaPath}; define its semantic fields or set additionalProperties explicitly for an intentional open payload`,
    );
  }
}
if (manualSchemaFindings.length > 0) {
  throw new Error(`manual packet schemas conflict with run.yaml output contracts:\n${manualSchemaFindings.join("\n")}`);
}

const staleGenerated = [...existingById.entries()]
  .filter(([packetId, existing]) => existing.generated && !contracts.has(packetId))
  .sort(([left], [right]) => left.localeCompare(right));
const orphanedManual = [...existingById.entries()]
  .filter(
    ([packetId, existing]) =>
      !existing.generated && !declarations.has(packetId) && !contracts.has(packetId),
  )
  .sort(([left], [right]) => left.localeCompare(right));
if (orphanedManual.length > 0) {
  throw new Error(
    `manual packet schemas have no active declaration or public native owner; remove or assign ownership explicitly:\n${orphanedManual
      .map(([, existing]) => path.relative(workspaceRoot, existing.path))
      .join("\n")}`,
  );
}
if (check && staleGenerated.length > 0) {
  throw new Error(
    `generated packet schemas have no active declaration or public native owner:\n${staleGenerated
      .map(([, existing]) => path.relative(workspaceRoot, existing.path))
      .join("\n")}`,
  );
}
if (!check) {
  await Promise.all(staleGenerated.map(([, existing]) => unlink(existing.path)));
}

for (const contract of [...contracts.values()].sort((left, right) => left.packetId.localeCompare(right.packetId))) {
  const existing = existingById.get(contract.packetId);
  if (existing && !existing.generated) continue;
  const filePath = existing?.path ?? path.join(packetRoot, `${packetFileName(contract.packetId)}.schema.json`);
  const document = `${JSON.stringify({
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: packetSchemaId(contract.packetId),
    "x-runx-packet-id": contract.packetId,
    "x-runx-generated-from": contract.source,
    ...contract.schema,
  }, null, 2)}\n`;
  if (check) {
    const current = await readFile(filePath, "utf8").catch(() => undefined);
    if (current !== document) {
      throw new Error(`packet schema is missing or stale: ${path.relative(workspaceRoot, filePath)}`);
    }
  } else {
    await writeFile(filePath, document, "utf8");
  }
}

const missing = [...declarations.keys()].filter(
  (packetId) => !existingById.has(packetId) && !contracts.has(packetId),
);
if (missing.length > 0) {
  throw new Error(`packet declarations have no schema contract: ${missing.join(", ")}`);
}
console.log(
  `${check ? "checked" : "generated"} ${contracts.size} packet contracts for ${declarations.size} manifest declarations${
    !check && staleGenerated.length > 0 ? `; removed ${staleGenerated.length} stale generated artifact(s)` : ""
  }`,
);

// Packet declarations come from the native typed tool catalog. Standalone
// generation must not inspect the workspace with a stale CLI; verify:fast
// supplies its unified prebuilt binary and skips this fallback build.
function ensureCurrentNativeRunx(): void {
  if (process.env.RUNX_RUST_CLI_BIN) return;
  const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
  const result = spawnSync(
    cargo,
    [
      "build",
      "--quiet",
      "--manifest-path",
      path.join(workspaceRoot, "crates", "Cargo.toml"),
      "-p",
      "runx-cli",
      "--bin",
      "runx",
    ],
    { cwd: workspaceRoot, env: process.env, stdio: "inherit" },
  );
  if (result.error) throw result.error;
  if ((result.status ?? 1) !== 0) {
    throw new Error(`failed to build the native Runx catalog owner (exit ${result.status ?? 1})`);
  }
}

function collectManifestContracts(manifest: RunnerManifest, profile: string): void {
  for (const [runnerName, runner] of Object.entries(manifest.runners)) {
    const location = `root.runners.${runnerName}`;
    collectInputPacketDeclarations(runner.inputs, profile, location);
    for (const [index, step] of runner.graph.steps.entries()) {
      collectStepContract(step, profile, `${location}.steps.${index}`);
    }
  }
}

function collectInputPacketDeclarations(
  inputs: Readonly<Record<string, InputDefinition>> | undefined,
  profile: string,
  location: string,
): void {
  for (const [inputName, input] of Object.entries(inputs ?? {})) {
    const packetId = nonEmptyString(input.packet);
    if (!packetId) continue;
    const source = `${profile}#${location}.inputs.${inputName}`;
    if (!declarations.has(packetId)) declarations.set(packetId, source);
  }
}

function collectStepContract(step: GraphStep, profile: string, location: string): void {
  const { outputs, artifacts } = step;
  if ("agent" in step.kind && (!outputs || Object.keys(outputs).length === 0)) {
    throw new Error(`${profile}#${location} agent step has no declared outputs`);
  }
  if (artifacts) {
    const source = `${profile}#${location}`;
    collectPacketDeclarations(artifacts, source);
    if (outputs && Object.keys(outputs).length > 0) {
      collectArtifactContracts(artifacts, outputs, source);
    }
  }
}

function collectPacketDeclarations(artifacts: ArtifactContract, source: string): void {
  const packetIds = [nonEmptyString(artifacts.packet)];
  if (artifacts.packets) {
    packetIds.push(...Object.values(artifacts.packets).map(nonEmptyString));
  }
  for (const packetId of packetIds) {
    if (!packetId) continue;
    const existing = declarations.get(packetId);
    if (!existing) declarations.set(packetId, source);
  }
}

function collectArtifactContracts(
  artifacts: ArtifactContract,
  outputs: JsonObject,
  source: string,
): void {
  const wrapAs = nonEmptyString(artifacts.wrap_as);
  const packet = nonEmptyString(artifacts.packet);
  if (packet) {
    if (!wrapAs) throw new Error(`${source} packet requires wrap_as`);
    // wrap_as names the step's whole declared result as the artifact, as the
    // runtime projects it.
    register({ packetId: packet, source, schema: objectSchema(outputs) });
  }
  if (!artifacts.packets) return;
  for (const [output, packetValue] of Object.entries(artifacts.packets)) {
    const packetId = nonEmptyString(packetValue);
    if (!packetId) throw new Error(`${source} packets.${output} must be a packet id`);
    if (!(output in outputs)) throw new Error(`${source} packets.${output} has no matching output declaration`);
    register({ packetId, source, schema: outputSchema(outputs[output]) });
  }
}

function register(contract: PacketContract): void {
  if (ownedContracts.has(contract.packetId)) {
    manualContracts.push(contract);
    return;
  }
  if (existingById.get(contract.packetId)?.generated === false) {
    manualContracts.push(contract);
    if (!contracts.has(contract.packetId)) contracts.set(contract.packetId, contract);
    return;
  }
  const existing = contracts.get(contract.packetId);
  if (existing && JSON.stringify(existing.schema) !== JSON.stringify(contract.schema)) {
    if (isBareObjectSchema(existing.schema) && !isBareObjectSchema(contract.schema)) {
      contracts.set(contract.packetId, contract);
      return;
    }
    if (!isBareObjectSchema(existing.schema) && isBareObjectSchema(contract.schema)) return;
    throw new Error(`packet '${contract.packetId}' has conflicting run.yaml output contracts`);
  }
  if (!existing) contracts.set(contract.packetId, contract);
}

async function ownedPacketContracts(): Promise<Map<string, PacketContract>> {
  const result = new Map<string, PacketContract>();
  const schemaRoot = path.join(workspaceRoot, "schemas");
  for (const entry of (await readdir(schemaRoot)).filter((name) => name.endsWith(".schema.json")).sort()) {
    const relativePath = path.posix.join("schemas", entry);
    const schema = JSON.parse(await readFile(path.join(workspaceRoot, relativePath), "utf8")) as JsonObject;
    const packetId = nonEmptyString(schema["x-runx-schema"]);
    if (!packetId) continue;
    if (!nonEmptyString(schema.$id)) {
      throw new Error(`${relativePath} must declare its canonical schema id for '${packetId}'`);
    }
    if (result.has(packetId)) {
      throw new Error(`duplicate owned packet schema id '${packetId}'`);
    }
    result.set(packetId, { packetId, source: relativePath, schema });
  }
  return result;
}

function structuralFloorFindings(
  actual: JsonObject,
  floor: JsonObject,
  packetId: string,
  source: string,
): readonly string[] {
  const findings: string[] = [];
  const actualView = schemaView(actual, actual, new Set());
  const floorType = nonEmptyString(floor.type);
  if (floorType && actualView.type !== floorType) {
    findings.push(
      `manual packet schema '${packetId}' for ${source} must constrain the root to type '${floorType}'`,
    );
  }
  const required = stringArray(floor.required);
  for (const field of required) {
    const expected = isRecord(floor.properties) ? floor.properties[field] : undefined;
    const declaredType = isRecord(expected) ? nonEmptyString(expected.type) : undefined;
    const actualProperty = actualView.properties.get(field);
    if (!actualProperty) {
      findings.push(
        `manual packet schema '${packetId}' for ${source} must declare root property '${field}'`,
      );
      continue;
    }
    const actualType = schemaView(actualProperty, actual, new Set()).type;
    if (declaredType && actualType !== declaredType) {
      findings.push(
        `manual packet schema '${packetId}' for ${source} must type root property '${field}' as '${declaredType}'`,
      );
    }
  }
  return findings;
}

function schemaView(
  schema: JsonObject,
  root: JsonObject,
  visitedRefs: Set<string>,
): {
  readonly type?: string;
  readonly required: Set<string>;
  readonly properties: Map<string, JsonObject>;
  readonly closed: boolean;
} {
  let type = nonEmptyString(schema.type);
  let closed = schema.additionalProperties === false;
  const required = new Set(stringArray(schema.required));
  const properties = new Map<string, JsonObject>();
  if (isRecord(schema.properties)) {
    for (const [name, value] of Object.entries(schema.properties)) {
      if (isRecord(value)) properties.set(name, value);
    }
  }
  const branches: JsonObject[] = [];
  const ref = nonEmptyString(schema.$ref);
  if (ref?.startsWith("#/") && !visitedRefs.has(ref)) {
    const resolved = resolveLocalRef(root, ref);
    if (resolved) {
      visitedRefs.add(ref);
      branches.push(resolved);
    }
  }
  if (Array.isArray(schema.allOf)) {
    branches.push(...schema.allOf.filter(isRecord));
  }
  for (const branch of branches) {
    const view = schemaView(branch, root, new Set(visitedRefs));
    type ??= view.type;
    closed ||= view.closed;
    for (const field of view.required) required.add(field);
    for (const [name, value] of view.properties) properties.set(name, value);
  }
  for (const keyword of ["anyOf", "oneOf"] as const) {
    const alternatives = Array.isArray(schema[keyword])
      ? schema[keyword].filter(isRecord)
      : [];
    if (alternatives.length === 0) continue;
    const views = alternatives.map((alternative) =>
      schemaView(alternative, root, new Set(visitedRefs))
    );
    const alternativeType = views[0]?.type;
    if (alternativeType && views.every((view) => view.type === alternativeType)) {
      type ??= alternativeType;
    }
    closed ||= views.every((view) => view.closed);
    const commonRequired = views.slice(1).reduce(
      (common, view) => new Set([...common].filter((field) => view.required.has(field))),
      new Set(views[0]?.required ?? []),
    );
    for (const field of commonRequired) required.add(field);
    const commonProperties = views.slice(1).reduce(
      (common, view) => new Set([...common].filter((field) => view.properties.has(field))),
      new Set(views[0]?.properties.keys() ?? []),
    );
    for (const field of commonProperties) {
      properties.set(field, {
        [keyword]: views.map((view) => view.properties.get(field)),
      });
    }
  }
  return { type, required, properties, closed };
}

function resolveLocalRef(root: JsonObject, ref: string): JsonObject | undefined {
  let value: unknown = root;
  for (const encoded of ref.slice(2).split("/")) {
    if (!isRecord(value)) return undefined;
    const segment = encoded.replace(/~1/gu, "/").replace(/~0/gu, "~");
    value = value[segment];
  }
  return isRecord(value) ? value : undefined;
}

function stringArray(value: unknown): readonly string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
}

function objectSchema(outputs: JsonObject): JsonObject {
  const required = Object.entries(outputs)
    .filter(([, declaration]) => outputIsRequired(declaration))
    .map(([name]) => name)
    .sort();
  return {
    type: "object",
    required,
    properties: Object.fromEntries(
      Object.entries(outputs)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([name, declaration]) => [name, outputSchema(declaration)]),
    ),
    additionalProperties: false,
  };
}

function outputIsRequired(declaration: unknown): boolean {
  return !isRecord(declaration) || declaration.required !== false;
}

function outputSchema(declaration: unknown): JsonObject {
  const type = typeof declaration === "string"
    ? declaration
    : isRecord(declaration) && typeof declaration.type === "string"
      ? declaration.type
      : "json";
  if (!new Set(["string", "number", "integer", "boolean", "array", "object", "json", "null"]).has(type)) {
    throw new Error(`unsupported agent output type '${type}'`);
  }
  const schema = isRecord(declaration) && isRecord(declaration.schema)
    ? structuredClone(declaration.schema)
    : {};
  if (type !== "json") schema.type = type;
  if (isRecord(declaration) && Array.isArray(declaration.enum)) schema.enum = declaration.enum;
  if (isRecord(declaration) && typeof declaration.description === "string") {
    schema.description = declaration.description;
  }
  return schema;
}

function isBareObjectSchema(schema: JsonObject): boolean {
  const view = schemaView(schema, schema, new Set());
  return view.type === "object"
    && view.required.size === 0
    && view.properties.size === 0
    && !view.closed;
}

function ambiguousObjectSchemaPaths(schema: JsonObject): readonly string[] {
  const findings: string[] = [];
  visitSchema(schema, "$", findings);
  return findings;
}

function visitSchema(schema: JsonObject, schemaPath: string, findings: string[]): void {
  const declaredTypes = typeof schema.type === "string"
    ? [schema.type]
    : Array.isArray(schema.type)
      ? schema.type.filter((value): value is string => typeof value === "string")
      : [];
  const declaresObject = declaredTypes.includes("object");
  const objectContractIsExplicit = [
    "$ref",
    "additionalProperties",
    "allOf",
    "anyOf",
    "const",
    "enum",
    "if",
    "maxProperties",
    "minProperties",
    "not",
    "oneOf",
    "patternProperties",
    "properties",
    "propertyNames",
    "unevaluatedProperties",
  ].some((keyword) => Object.hasOwn(schema, keyword));
  if (declaresObject && !objectContractIsExplicit) findings.push(schemaPath);

  for (const keyword of [
    "additionalProperties",
    "contains",
    "else",
    "if",
    "items",
    "not",
    "propertyNames",
    "then",
    "unevaluatedProperties",
  ]) {
    const child = schema[keyword];
    if (isRecord(child)) visitSchema(child, `${schemaPath}.${keyword}`, findings);
  }
  for (const keyword of ["allOf", "anyOf", "oneOf", "prefixItems"]) {
    const children = schema[keyword];
    if (!Array.isArray(children)) continue;
    children.forEach((child, index) => {
      if (isRecord(child)) visitSchema(child, `${schemaPath}.${keyword}[${index}]`, findings);
    });
  }
  for (const keyword of [
    "$defs",
    "definitions",
    "dependentSchemas",
    "patternProperties",
    "properties",
  ]) {
    const children = schema[keyword];
    if (!isRecord(children)) continue;
    for (const [name, child] of Object.entries(children)) {
      if (isRecord(child)) visitSchema(child, `${schemaPath}.${keyword}.${name}`, findings);
    }
  }
}

async function existingSchemas(): Promise<Map<string, ExistingPacketSchema>> {
  const schemas = new Map<string, ExistingPacketSchema>();
  const entries = (await readdir(packetRoot)).filter((name) => name.endsWith(".json")).sort();
  const documents = await Promise.all(entries.map(async (entry) => ({
    path: path.posix.join("dist", "packets", entry),
    source: await readFile(path.join(packetRoot, entry), "utf8"),
  })));
  const parsed = parsePacketSchemaDocumentsBatch(documents) as Array<ParsedPacketSchema | undefined>;
  for (const [index, schema] of parsed.entries()) {
    if (!schema) continue;
    const filePath = path.join(packetRoot, entries[index]!);
    if (schemas.has(schema.packetId)) throw new Error(`duplicate packet schema id '${schema.packetId}'`);
    schemas.set(schema.packetId, {
      path: filePath,
      generated: typeof schema.value["x-runx-generated-from"] === "string",
      schema: schema.value,
    });
  }
  return schemas;
}

async function findProfiles(directory: string): Promise<readonly string[]> {
  const profiles: string[] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) profiles.push(...await findProfiles(entryPath));
    else if (entry.isFile() && entry.name === "run.yaml") profiles.push(entryPath);
  }
  return profiles.sort();
}

function packetFileName(packetId: string): string {
  return packetId.replace(/[^a-zA-Z0-9]+/g, ".").replace(/^\.+|\.+$/g, "");
}

function packetSchemaId(packetId: string): string {
  const segments = packetId.split(".").filter(Boolean);
  if (segments[0] === "runx") segments.shift();
  return `https://schemas.runx.ai/runx/${segments.join("/")}.json`;
}

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function isRecord(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
