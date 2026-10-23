// Collects every packet the workspace publishes and holds the packet gates.
//
// A package packet's source is its file in the owning package's `packets/`;
// a core packet's is its Rust type, published as `schemas/<file>`. This
// writes `dist/packets/<publisher>/<package>/<name>.v<N>.schema.json`, exact
// copies the site serves at each `$id`, and `packets.lock.json`, the digest
// each published package packet id was frozen at. With `--check` it writes
// nothing and fails on any drift.
//
// Gates: the parser admits each package's packets (an id names its owner and
// its file); every declared packet resolves; every package packet is
// declared by its owner; a `wrap_as` step declares the fields its packet
// requires; no object in a package packet is left without a shape; a
// published id never changes bytes; and no SKILL.md restates a packet in an
// output-schema section.

import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  listToolPacketDeclarations,
  resolveNativeRunxBinary,
  validateRunnerManifestYamlBatch,
} from "./lib/native-parser.mjs";

type JsonObject = Record<string, unknown>;

interface PacketFile {
  readonly id: string;
  readonly path: string;
  readonly source: string;
}

interface StepDeclaration {
  readonly id: string;
  readonly outputs?: JsonObject;
  readonly artifacts?: {
    readonly packet?: string;
    readonly packets?: Readonly<Record<string, string>>;
    readonly wrap_as?: string;
  };
}

interface RunnerDeclaration {
  readonly inputs?: Readonly<Record<string, { readonly packet?: string }>>;
  readonly graph: { readonly steps: readonly StepDeclaration[] };
}

interface RunnerManifest {
  readonly runners: Readonly<Record<string, RunnerDeclaration>>;
}

interface LockEntry {
  readonly id: string;
  readonly digest: string;
}

const PUBLISHER = "runx";
const LOCK_SCHEMA = "runx.packet.lock.v1";
const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const skillsRoot = path.join(workspaceRoot, "skills");
const distRoot = path.join(workspaceRoot, "dist", "packets");
const lockPath = path.join(workspaceRoot, "packets.lock.json");
const check = process.argv.includes("--check");
const findings: string[] = [];

ensureCurrentNativeRunx();
const corePackets = await readCorePackets();
const packagePackets = await readPackagePackets();
const declarations = await readDeclarations();

for (const packet of packagePackets.values()) {
  const owner = ownerOf(packet.id);
  if (!owner || owner.publisher !== PUBLISHER || packet.path.split("/")[1] !== owner.package) {
    findings.push(`${packet.path}: ${packet.id} must be owned by ${PUBLISHER}/${packet.path.split("/")[1]}`);
  }
  if (!declarations.get(owner?.package ?? "")?.ids.has(packet.id)) {
    findings.push(`${packet.path}: ${packet.id} is never declared by its own package; bind it or remove it`);
  }
  for (const schemaPath of shapelessObjects(JSON.parse(packet.source) as JsonObject, "$")) {
    findings.push(
      `${packet.path}: an object at ${schemaPath} has no shape; define its fields or set additionalProperties`,
    );
  }
}
for (const [packageName, declared] of declarations) {
  for (const id of declared.ids) {
    if (!corePackets.has(id) && !packagePackets.has(id)) {
      findings.push(`skills/${packageName} declares ${id}, which is neither a core packet nor any workspace package's`);
    }
  }
  for (const wrapped of declared.wrapped) {
    const packet = packagePackets.get(wrapped.packet);
    if (!packet) continue;
    findings.push(...wrappedFieldFindings(JSON.parse(packet.source) as JsonObject, wrapped));
  }
}
for (const declaration of listToolPacketDeclarations()) {
  if (!corePackets.has(declaration.packetId)) {
    findings.push(`native tool ${declaration.source} emits ${declaration.packetId}, which is not a core packet`);
  }
}
findings.push(...(await skillManualFindings()));

// Only package packets are copied into bundles, so only theirs are frozen; a
// core packet's one source is the runx that carries it.
const lock = nextLock([...packagePackets.values()].map((packet) => [packet.id, packet.source] as const));
const lockDocument = `${JSON.stringify({ schema: LOCK_SCHEMA, packets: lock }, null, 2)}\n`;

if (findings.length > 0) {
  throw new Error(`packet gates failed:\n${findings.join("\n")}`);
}
const drift = [
  ...(await reconcileDist([...packagePackets.values()])),
  ...(await reconcileFile(lockPath, lockDocument)),
];
if (drift.length > 0) {
  throw new Error(`packet artifacts are stale; run pnpm packets:collect:\n${drift.join("\n")}`);
}
console.log(
  `${check ? "checked" : "collected"} ${packagePackets.size} package packets and ${corePackets.size} core packets`,
);

/** `<publisher>` and `<package>` of a package packet id; undefined for a core one. */
function ownerOf(id: string): { publisher: string; package: string; file: string } | undefined {
  const match = /^([a-z0-9-]+)\/([a-z0-9-]+)\.(.+)\.v([1-9][0-9]*)$/u.exec(id);
  if (!match) return undefined;
  const [, publisher, packageName, name, version] = match;
  return { publisher: publisher!, package: packageName!, file: `${name}.v${version}.schema.json` };
}

/** Core packets as published: `schemas/` documents marked as packets, by id. */
async function readCorePackets(): Promise<Map<string, string>> {
  const packets = new Map<string, string>();
  const schemaRoot = path.join(workspaceRoot, "schemas");
  for (const entry of (await readdir(schemaRoot)).filter((name) => name.endsWith(".schema.json")).sort()) {
    const source = await readFile(path.join(schemaRoot, entry), "utf8");
    const schema = JSON.parse(source) as JsonObject;
    if (schema["x-runx-packet"] !== true) continue;
    const id = schema["x-runx-schema"];
    if (typeof id !== "string") throw new Error(`schemas/${entry} is a packet without x-runx-schema`);
    packets.set(id, source);
  }
  return packets;
}

/** Every workspace package's packets, as the parser admitted them with each package. */
async function readPackagePackets(): Promise<Map<string, PacketFile>> {
  const result = spawnSync(resolveNativeRunxBinary(), ["list", "packets", "--json"], {
    cwd: workspaceRoot,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
  });
  if (result.status !== 0) throw new Error(`runx list packets failed:\n${result.stderr}`);
  const report = JSON.parse(result.stdout) as {
    items: { name: string; path: string; status: string; diagnostics?: string[] }[];
  };
  const packets = new Map<string, PacketFile>();
  for (const item of report.items) {
    if (item.status !== "ok") {
      findings.push(`${item.path}: ${(item.diagnostics ?? []).join(", ")}`);
      continue;
    }
    packets.set(item.name, {
      id: item.name,
      path: item.path,
      source: await readFile(path.join(workspaceRoot, item.path), "utf8"),
    });
  }
  return packets;
}

/** What each package's profiles declare: every packet id, and each `wrap_as` binding. */
async function readDeclarations(): Promise<
  Map<string, { ids: Set<string>; wrapped: { packet: string; outputs: string[]; at: string }[] }>
> {
  const profilePaths = await findProfiles(skillsRoot);
  const manifests = validateRunnerManifestYamlBatch(
    await Promise.all(profilePaths.map((profilePath) => readFile(profilePath, "utf8"))),
  ) as RunnerManifest[];
  const declarations = new Map<
    string,
    { ids: Set<string>; wrapped: { packet: string; outputs: string[]; at: string }[] }
  >();
  profilePaths.forEach((profilePath, index) => {
    const relative = path.relative(workspaceRoot, profilePath);
    const packageName = relative.split(path.sep)[1]!;
    const declared = declarations.get(packageName) ?? { ids: new Set<string>(), wrapped: [] };
    declarations.set(packageName, declared);
    for (const [runnerName, runner] of Object.entries(manifests[index]!.runners)) {
      for (const input of Object.values(runner.inputs ?? {})) {
        if (input.packet) declared.ids.add(input.packet);
      }
      for (const step of runner.graph.steps) {
        const artifacts = step.artifacts;
        if (!artifacts) continue;
        for (const packet of Object.values(artifacts.packets ?? {})) declared.ids.add(packet);
        if (artifacts.packet) {
          declared.ids.add(artifacts.packet);
          declared.wrapped.push({
            packet: artifacts.packet,
            outputs: Object.keys(step.outputs ?? {}),
            at: `${relative}#${runnerName}.${step.id}`,
          });
        }
      }
    }
  });
  return declarations;
}

/** A `wrap_as` step's outputs are its packet's fields: every required one, and none it closes out. */
function wrappedFieldFindings(
  packet: JsonObject,
  wrapped: { packet: string; outputs: string[]; at: string },
): string[] {
  // A step that declares no outputs adopts its whole result as the packet.
  if (wrapped.outputs.length === 0) return [];
  const properties = isRecord(packet.properties) ? packet.properties : {};
  const findings = stringArray(packet.required)
    .filter((field) => !wrapped.outputs.includes(field))
    .map((field) => `${wrapped.at}: ${wrapped.packet} requires ${field}, which the step does not declare`);
  if (packet.additionalProperties === false) {
    findings.push(
      ...wrapped.outputs
        .filter((output) => !(output in properties))
        .map((output) => `${wrapped.at}: ${wrapped.packet} has no field ${output}, which the step declares`),
    );
  }
  return findings;
}

/** The lock with this tree's packets: an existing id keeps its digest, and a new one is added. */
function nextLock(packets: readonly (readonly [string, string])[]): LockEntry[] {
  const frozen = new Map<string, string>();
  if (existsSync(lockPath)) {
    try {
      const current = JSON.parse(readFileSync(lockPath, "utf8")) as { packets?: LockEntry[] };
      for (const entry of current.packets ?? []) frozen.set(entry.id, entry.digest);
    } catch {
      findings.push("packets.lock.json is not valid JSON");
    }
  }
  const next = new Map(frozen);
  for (const [id, source] of packets) {
    const digest = `sha256:${createHash("sha256").update(source).digest("hex")}`;
    const was = frozen.get(id);
    if (was && was !== digest) {
      findings.push(`${id} changed bytes since it was published; add a new version beside it instead`);
    }
    if (!was) next.set(id, digest);
  }
  return [...next]
    .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
    .map(([id, digest]) => ({ id, digest }));
}

/** SKILL.md names a packet by its id; it never restates the packet's shape in an output-schema section. */
async function skillManualFindings(): Promise<string[]> {
  const owners = new Set([...packagePackets.values()].map((packet) => ownerOf(packet.id)?.package));
  const findings: string[] = [];
  for (const owner of owners) {
    if (!owner) continue;
    const manualPath = path.join(skillsRoot, owner, "SKILL.md");
    const manual = await readFile(manualPath, "utf8");
    for (const heading of manual.matchAll(/^#{2,4}\s+(?:output|outputs)\s+(?:schema|shape)s?\b.*$/gimu)) {
      findings.push(
        `skills/${owner}/SKILL.md: "${heading[0].trim()}" restates a packet; name the packet id instead`,
      );
    }
  }
  return findings;
}

/** Write, or with `--check` compare, `dist/packets/` as exact copies of the package packets. */
async function reconcileDist(packets: readonly PacketFile[]): Promise<string[]> {
  const expected = new Map(
    packets.map((packet) => {
      const owner = ownerOf(packet.id)!;
      return [path.join(distRoot, owner.publisher, owner.package, owner.file), packet.source] as const;
    }),
  );
  const drift: string[] = [];
  for (const existing of await listFiles(distRoot)) {
    if (expected.has(existing)) continue;
    drift.push(`stale ${path.relative(workspaceRoot, existing)}`);
    if (!check) await rm(existing);
  }
  for (const [filePath, source] of expected) {
    drift.push(...(await reconcileFile(filePath, source)));
  }
  return check ? drift : [];
}

async function reconcileFile(filePath: string, source: string): Promise<string[]> {
  const current = await readFile(filePath, "utf8").catch(() => undefined);
  if (current === source) return [];
  if (check) return [`${path.relative(workspaceRoot, filePath)} is missing or stale`];
  await mkdir(path.dirname(filePath), { recursive: true });
  await writeFile(filePath, source, "utf8");
  return [];
}

async function listFiles(directory: string): Promise<string[]> {
  const entries = await readdir(directory, { withFileTypes: true }).catch(() => []);
  const files: string[] = [];
  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...(await listFiles(entryPath)));
    else files.push(entryPath);
  }
  return files;
}

async function findProfiles(directory: string): Promise<string[]> {
  const profiles: string[] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) profiles.push(...(await findProfiles(entryPath)));
    else if (entry.isFile() && entry.name === "run.yaml") profiles.push(entryPath);
  }
  return profiles.sort();
}

/** Every object a schema declares that names no fields and no explicit openness. */
function shapelessObjects(schema: JsonObject, schemaPath: string): string[] {
  const types = typeof schema.type === "string"
    ? [schema.type]
    : Array.isArray(schema.type)
      ? schema.type.filter((value): value is string => typeof value === "string")
      : [];
  const explicit = [
    "$ref", "additionalProperties", "allOf", "anyOf", "const", "enum", "if", "maxProperties",
    "minProperties", "not", "oneOf", "patternProperties", "properties", "propertyNames",
    "unevaluatedProperties",
  ].some((keyword) => Object.hasOwn(schema, keyword));
  const found = types.includes("object") && !explicit ? [schemaPath] : [];
  for (const keyword of ["additionalProperties", "contains", "else", "if", "items", "not", "propertyNames", "then", "unevaluatedProperties"]) {
    const child = schema[keyword];
    if (isRecord(child)) found.push(...shapelessObjects(child, `${schemaPath}.${keyword}`));
  }
  for (const keyword of ["allOf", "anyOf", "oneOf", "prefixItems"]) {
    const children = schema[keyword];
    if (!Array.isArray(children)) continue;
    children.forEach((child, index) => {
      if (isRecord(child)) found.push(...shapelessObjects(child, `${schemaPath}.${keyword}[${index}]`));
    });
  }
  for (const keyword of ["$defs", "definitions", "dependentSchemas", "patternProperties", "properties"]) {
    const children = schema[keyword];
    if (!isRecord(children)) continue;
    for (const [name, child] of Object.entries(children)) {
      if (isRecord(child)) found.push(...shapelessObjects(child, `${schemaPath}.${keyword}.${name}`));
    }
  }
  return found;
}

// The packet listing comes from the native parser; without a prebuilt binary
// this builds the current one rather than reading the workspace with a stale CLI.
function ensureCurrentNativeRunx(): void {
  if (process.env.RUNX_RUST_CLI_BIN) return;
  const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
  const result = spawnSync(
    cargo,
    ["build", "--quiet", "--manifest-path", path.join(workspaceRoot, "crates", "Cargo.toml"), "-p", "runx-cli", "--bin", "runx"],
    { cwd: workspaceRoot, env: process.env, stdio: "inherit" },
  );
  if (result.error) throw result.error;
  if ((result.status ?? 1) !== 0) throw new Error(`failed to build the native runx (exit ${result.status ?? 1})`);
}

function stringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
}

function isRecord(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
