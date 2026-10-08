// Connector manifests as runx Connect reads them, and their declared requests
// rendered exactly as the kernel's http driver renders them
// (crates/runx-contracts/src/connector/template.rs and
// crates/runx-runtime/src/drivers/http.rs). fixtures/contracts/connector-request
// holds both renderers to the same answers. Manifests are validated in Rust;
// this module reads them, it never validates them.

import { Ajv2020, type ValidateFunction } from "ajv/dist/2020.js";

import { officialConnectors } from "./generated/official-connectors.js";

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | readonly JsonValue[]
  | { readonly [key: string]: JsonValue };

export type JsonObject = { readonly [key: string]: JsonValue };

export type Template = JsonValue;

export type OperationVerb =
  | "read"
  | "write"
  | "comment"
  | "review"
  | "merge"
  | "create"
  | "update"
  | "delete"
  | "execute"
  | "revoke";

export type HttpMethod = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

export interface HttpRequest {
  readonly id: string;
  readonly each?: Template;
  readonly method: HttpMethod;
  readonly url: Template;
  readonly query?: { readonly [name: string]: Template };
  readonly headers?: { readonly [name: string]: Template };
  readonly body?: Template;
  /** A local read answered in pages; runx Connect never pages. */
  readonly pages?: {
    readonly param: string;
    readonly cursor: string;
    readonly items: string;
    readonly limit?: Template;
  };
}

export interface HttpReadback {
  readonly request: HttpRequest;
  readonly matches: readonly JsonValue[];
  readonly expected_status?: number;
  readonly decode?: readonly DecodeStep[];
}

export type DecodeStep =
  | { readonly op: "auto_json" | "text" | "status" | "parse_json" }
  | { readonly op: "field" | "reject_true" | "reject_present"; readonly path: string };

export interface HttpImplementation {
  readonly allowed_hosts: readonly string[];
  /** How each credential authenticates a request, by credential name. */
  readonly auth?: {
    readonly [credential: string]:
      | { readonly type: "bearer"; readonly field?: string }
      | { readonly type: "oauth1" };
  };
  readonly requests: readonly HttpRequest[];
  readonly preconditions?: readonly HttpReadback[];
  readonly readbacks?: readonly HttpReadback[];
  readonly decode: readonly DecodeStep[];
  readonly operation_id_path?: string;
  readonly result_paths: { readonly [field: string]: string };
  readonly rate_limit?: { readonly reset_header: string };
}

export interface ConnectorOperation {
  readonly description: string;
  readonly verb: OperationVerb;
  readonly scopes: readonly string[];
  readonly credentials?: readonly string[];
  readonly target: { readonly template: Template; readonly pattern?: string };
  readonly readback?: string;
  readonly approval?: "none" | "required";
  readonly standing?: JsonObject;
  readonly idempotency_key_format?: "runx" | "uuid";
  readonly guarantees?: JsonObject;
  readonly amount?: "none" | "required";
  readonly input_schema?: JsonObject;
  readonly result_fields?: readonly string[];
  /** Present when the hosted placement offers the operation. */
  readonly hosted?: { readonly provider_scopes?: readonly string[] };
  readonly implementation?: {
    readonly http?: HttpImplementation;
    readonly mcp?: JsonObject;
    readonly process?: JsonObject;
    readonly web?: JsonObject;
    readonly store?: JsonObject;
  };
}

export interface ConnectorManifest {
  readonly schema: "runx.connector.v1";
  readonly id: string;
  readonly name: string;
  readonly description: string;
  readonly auth: JsonObject;
  /** The JSON Schema of the parameters an instance pins. */
  readonly parameters?: JsonObject;
  readonly placements: {
    readonly hosted?: {
      readonly auth: "delegated" | "credential";
      readonly credential?: string;
      /** Absent means public. */
      readonly callers?: "public" | "runtime";
    };
    readonly local?: JsonObject;
  };
  readonly operations: { readonly [name: string]: ConnectorOperation };
  readonly messages?: JsonObject;
  readonly references?: readonly ReferenceDeclaration[];
}

/** The external reference kinds a connector's locators may name. */
export type ReferenceKind =
  | "thread"
  | "comment"
  | "event"
  | "tracking_item"
  | "change_request"
  | "repository"
  | "external_url";

/** One kind of thing a connector's locators name. */
export interface ReferenceDeclaration {
  readonly kind: ReferenceKind;
  /** A regular expression the whole locator matches. */
  readonly locator: string;
}

/** An official connector with the digests Rust computed for it. */
export interface OfficialConnector {
  readonly manifest_digest: string;
  readonly digests: {
    readonly [operation: string]: {
      readonly operation: string;
      readonly implementation?: string;
    };
  };
  readonly manifest: ConnectorManifest;
}

let referenceDeclarations: readonly (readonly [string, ReferenceKind, RegExp])[] | undefined;

/**
 * What `locator` names, from the reference kinds official connectors declare,
 * as the kernel classifies it: the first declaration of `provider`'s
 * connector it matches, or of any official connector when no provider is
 * given, and an external URL when none does.
 */
export function referenceKind(provider: string | undefined, locator: string): ReferenceKind {
  referenceDeclarations ??= officialConnectors.flatMap((connector) =>
    (connector.manifest.references ?? []).map(
      (reference) => [connector.manifest.id, reference.kind, new RegExp(`^(?:${reference.locator})$`, "u")] as const,
    ));
  return referenceDeclarations.find(([id, , pattern]) =>
    (provider === undefined || id === provider) && pattern.test(locator))?.[1] ?? "external_url";
}

/** Read versus mutate, derived from the verb. */
export function operationAccess(operation: ConnectorOperation): "read" | "mutate" {
  return operation.verb === "read" ? "read" : "mutate";
}

/** The declared requests the hosted placement sends, when it offers them. */
export function hostedHttp(operation: ConnectorOperation): HttpImplementation | undefined {
  return operation.hosted ? operation.implementation?.http : undefined;
}

const inputValidators = new WeakMap<JsonObject, ValidateFunction>();

/**
 * Where a call's input fails the operation's declared schema, as the kernel
 * checks it at admission (draft 2020-12, formats as annotations), or
 * undefined when it conforms. Never names the value.
 */
export function operationInputError(operation: ConnectorOperation, input: unknown): string | undefined {
  const schema = operation.input_schema;
  if (!schema) return undefined;
  let validate = inputValidators.get(schema);
  if (!validate) {
    validate = new Ajv2020({ strict: false, validateFormats: false, allErrors: false }).compile(schema);
    inputValidators.set(schema, validate);
  }
  if (validate(input)) return undefined;
  const error = validate.errors?.[0];
  return `the input at ${JSON.stringify(error?.instancePath ?? "")} does not satisfy the operation's schema at ${JSON.stringify(error?.schemaPath ?? "")}`;
}

export class TemplateError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TemplateError";
  }
}

/** A binding whose source the call does not carry. */
class Unavailable extends TemplateError {
  constructor(readonly binding: string, reference: string) {
    super(`template binding ${binding} ${JSON.stringify(reference)} is unavailable`);
  }
}

export interface TemplateInputs {
  readonly arguments?: JsonValue;
  readonly parameters?: JsonObject;
  readonly responses?: { readonly [request: string]: JsonObject };
  /** The element a repeated request is sent for. */
  readonly item?: JsonValue;
  readonly idempotencyKey?: string;
}

const MAX_DEPTH = 32;
const MAX_CONCAT_BYTES = 512;
const BINDINGS = new Set(["$argument", "$parameter", "$response", "$item", "$runx", "$concat"]);

export function renderTemplate(template: Template, inputs: TemplateInputs): JsonValue {
  return render(template, inputs, 0);
}

function render(value: Template, inputs: TemplateInputs, depth: number): JsonValue {
  if (depth > MAX_DEPTH) throw new TemplateError(`template nests deeper than ${MAX_DEPTH} levels`);
  if (Array.isArray(value)) return value.map((entry) => render(entry, inputs, depth + 1));
  if (value === null || typeof value !== "object") return value;
  const object = value as JsonObject;
  const keys = Object.keys(object);
  if (keys.length === 1 && BINDINGS.has(keys[0]!)) {
    return renderBinding(keys[0]!, object[keys[0]!]!, inputs, depth);
  }
  // A member that is an argument the call does not carry is omitted, as an
  // optional field is; the input schema decides which arguments a call must
  // carry.
  const rendered: Record<string, JsonValue> = {};
  for (const key of keys) {
    const member = object[key]!;
    try {
      rendered[key] = render(member, inputs, depth + 1);
    } catch (error) {
      if (!(error instanceof Unavailable && error.binding === "$argument" && isArgument(member))) throw error;
    }
  }
  return rendered;
}

function isArgument(value: Template): boolean {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    && Object.keys(value).length === 1 && Object.hasOwn(value, "$argument");
}

function renderBinding(key: string, argument: JsonValue, inputs: TemplateInputs, depth: number): JsonValue {
  const reference = typeof argument === "string" ? argument : "";
  const found = (value: JsonValue | undefined): JsonValue => {
    if (value === undefined) throw new Unavailable(key, reference);
    return value;
  };
  switch (key) {
    case "$argument":
      return found(inputs.arguments === undefined ? undefined : pointer(inputs.arguments, reference));
    case "$parameter":
      return found(inputs.parameters === undefined ? undefined : pointer(inputs.parameters, reference));
    case "$response":
      return found(inputs.responses === undefined ? undefined : response(inputs.responses, reference));
    case "$item":
      return found(inputs.item === undefined ? undefined : pointer(inputs.item, reference));
    case "$runx":
      return found(inputs.idempotencyKey);
    default: {
      let joined = "";
      for (const part of Array.isArray(argument) ? argument : []) {
        const rendered = render(part, inputs, depth + 1);
        if (typeof rendered === "string") joined += rendered;
        else if (typeof rendered === "number") joined += String(rendered);
        else throw new TemplateError("template $concat part must be a string or number");
        if (new TextEncoder().encode(joined).length > MAX_CONCAT_BYTES) {
          throw new TemplateError(`template $concat exceeds ${MAX_CONCAT_BYTES} bytes`);
        }
      }
      return joined;
    }
  }
}

/** RFC 6901 pointer lookup. */
function pointer(value: JsonValue, reference: string): JsonValue | undefined {
  if (reference === "") return value;
  if (!reference.startsWith("/")) return undefined;
  let current: JsonValue | undefined = value;
  for (const raw of reference.slice(1).split("/")) {
    const token = raw.replaceAll("~1", "/").replaceAll("~0", "~");
    current = child(current, token);
    if (current === undefined) return undefined;
  }
  return current;
}

function response(responses: { readonly [request: string]: JsonObject }, reference: string): JsonValue | undefined {
  const dot = reference.indexOf(".");
  if (dot <= 0) return undefined;
  const answer = responses[reference.slice(0, dot)];
  if (answer === undefined) return undefined;
  let current: JsonValue | undefined = answer;
  for (const segment of reference.slice(dot + 1).split(".")) {
    current = child(current, segment);
    if (current === undefined) return undefined;
  }
  return current;
}

function child(value: JsonValue | undefined, token: string): JsonValue | undefined {
  if (Array.isArray(value)) {
    return /^(0|[1-9][0-9]*)$/.test(token) ? value[Number(token)] : undefined;
  }
  if (value !== null && typeof value === "object") {
    return Object.hasOwn(value, token) ? (value as JsonObject)[token] : undefined;
  }
  return undefined;
}

/** A declared request, rendered: what runx Connect sends, credential aside. */
export interface RenderedRequest {
  readonly method: HttpMethod;
  readonly url: string;
  readonly headers: { readonly [name: string]: string };
  readonly body?: JsonValue;
}

export interface DeclaredRequestCall {
  readonly arguments: JsonObject;
  readonly parameters?: JsonObject;
  readonly idempotencyKey?: string;
  /** The declared request's id, among requests, preconditions and readbacks. */
  readonly request: string;
  /** The position of the element a repeated request is sent for. */
  readonly item?: number;
  /** Earlier answers; only checking reads bind them. */
  readonly responses?: { readonly [request: string]: JsonObject };
}

/** Whether a declared request is a checking read (precondition or readback). */
export function isCheckingRead(http: HttpImplementation, id: string): boolean {
  return [...(http.preconditions ?? []), ...(http.readbacks ?? [])].some((check) => check.request.id === id);
}

/** Render one declared request of an implementation for a call. */
export function renderDeclaredRequest(http: HttpImplementation, call: DeclaredRequestCall): RenderedRequest {
  const request = [
    ...http.requests,
    ...(http.preconditions ?? []).map((check) => check.request),
    ...(http.readbacks ?? []).map((check) => check.request),
  ].find((candidate) => candidate.id === call.request);
  if (!request) throw new TemplateError(`the implementation declares no request ${call.request}`);
  const base: TemplateInputs = {
    arguments: call.arguments,
    parameters: call.parameters ?? {},
    ...(call.idempotencyKey === undefined ? {} : { idempotencyKey: call.idempotencyKey }),
  };
  let item: JsonValue | undefined;
  if (call.item !== undefined) {
    // A checking read of a repeated write binds the write's item.
    const each = request.each
      ?? (isCheckingRead(http, request.id) ? http.requests.find((write) => write.each)?.each : undefined);
    if (each === undefined) throw new TemplateError(`request ${request.id} does not repeat`);
    const items = renderTemplate(each, base);
    if (!Array.isArray(items)) throw new TemplateError("each must render to an array");
    item = items[call.item];
    if (item === undefined) throw new TemplateError(`the batch has no item ${call.item}`);
  }
  const inputs: TemplateInputs = {
    ...base,
    ...(item === undefined ? {} : { item }),
    ...(call.responses === undefined ? {} : { responses: call.responses }),
  };
  const rendered = renderTemplate(request.url, inputs);
  if (typeof rendered !== "string") throw new TemplateError("template rendered a non-string where a string was required");
  let url: URL;
  try {
    url = new URL(rendered);
  } catch {
    throw new TemplateError(`request ${request.id} has an invalid URL`);
  }
  const host = url.hostname.toLowerCase();
  if (url.protocol !== "https:" || !http.allowed_hosts.some((allowed) => hostAllowed(allowed, host))) {
    throw new TemplateError(`request ${request.id} targets a host the implementation does not allow`);
  }
  let href = url.href;
  const queryNames = Object.keys(request.query ?? {}).sort();
  if (queryNames.length > 0) {
    // As the kernel does: a query value whose argument the call does not
    // carry is not sent, and the query is opened even when nothing is.
    const pairs = new URLSearchParams();
    for (const name of queryNames) {
      let value: JsonValue;
      try {
        value = renderTemplate(request.query![name]!, inputs);
      } catch (error) {
        if (error instanceof Unavailable && error.binding === "$argument") continue;
        throw error;
      }
      const scalar = scalarString(value);
      if (scalar !== undefined) pairs.append(name, scalar);
    }
    const serialized = pairs.toString();
    href = url.search === "" ? `${href.replace(/\?$/, "")}?${serialized}` : serialized ? `${href}&${serialized}` : href;
  }
  const headers: Record<string, string> = {};
  for (const name of Object.keys(request.headers ?? {}).sort()) {
    const value = renderTemplate(request.headers![name]!, inputs);
    if (typeof value !== "string") throw new TemplateError("template rendered a non-string where a string was required");
    headers[name] = value;
  }
  if (request.body === undefined) return { method: request.method, url: href, headers };
  const body = renderTemplate(request.body, inputs);
  headers["content-type"] = "application/json";
  return { method: request.method, url: href, headers, body };
}

/** An allowlist entry is a host, or `*.domain` for exactly one label under that domain. */
function hostAllowed(allowed: string, host: string): boolean {
  if (!allowed.startsWith("*.")) return allowed === host;
  const domain = allowed.slice(1);
  const label = host.endsWith(domain) ? host.slice(0, -domain.length) : "";
  return label !== "" && !label.includes(".");
}

function scalarString(value: JsonValue): string | undefined {
  if (typeof value === "string") return value === "" ? undefined : value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return undefined;
}
