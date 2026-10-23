// Connector manifests as runx Connect reads them, and their declared requests
// rendered exactly as the kernel's http driver renders them
// (crates/runx-contracts/src/connector/template.rs and
// crates/runx-runtime/src/drivers/http.rs). fixtures/contracts/connector-request
// holds both renderers to the same answers. Manifests are validated in Rust;
// this module reads them, it never validates them.

import { Ajv2020, type ValidateFunction } from "ajv/dist/2020.js";

import { generatedSchema, validateContractSchema } from "./internal.js";

/** Exact revision identity, validated by the Rust-owned binding schema. */
export interface ConnectorBinding {
  readonly connector: string;
  readonly manifest_digest: string;
  readonly connector_digest: string;
}

export const connectorBindingSchema = generatedSchema<ConnectorBinding>("connector-binding.schema.json");

export function validateConnectorBinding(value: unknown, label = "connector_binding"): ConnectorBinding {
  return validateContractSchema(connectorBindingSchema, value, label);
}

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

export type HttpAuth =
  | { readonly type: "bearer"; readonly field?: string }
  | { readonly type: "header"; readonly name: string; readonly field: string }
  | { readonly type: "oauth2_client_credentials"; readonly token_url: Template; readonly client_id: string; readonly client_secret: string; readonly scope?: string }
  | { readonly type: "oauth1" }
  | { readonly type: "url"; readonly field: string }
  /** A credential field placed in the JSON body at an RFC 6901 pointer; local placement only. */
  | { readonly type: "body"; readonly field: string; readonly pointer: string };

/** Public endpoint rendering only; app secrets and tokens stay in credential custody. */
export function renderOAuth2TokenEndpoint(template: Template, inputs: TemplateInputs, allowedHosts: readonly string[]): string {
  const value = renderTemplate(template, inputs);
  if (typeof value !== "string") throw new TemplateError("OAuth2 token endpoint must render a string");
  let url: URL;
  try { url = new URL(value); }
  catch { throw new TemplateError("OAuth2 token endpoint is invalid"); }
  if (url.protocol !== "https:" || url.username || url.password || url.search || url.hash || url.port
    || !allowedHosts.includes(url.hostname)) throw new TemplateError("OAuth2 token endpoint is outside its declared hosts");
  return url.href;
}

export interface CredentialField {
  readonly name: string;
  readonly label?: string;
  readonly secret?: boolean;
  readonly required?: boolean;
  readonly prefixes?: readonly string[];
}

export interface CredentialDeclaration {
  /** `api_key`, `oauth1_user`, ... or `purchase`: filled by a purchase, held by runx Connect. */
  readonly kind: string;
  readonly fields: readonly CredentialField[];
  /** How the local placement delivers it; a purchase credential has none. */
  readonly delivery?: { readonly env: string };
  readonly audience?: readonly string[];
}

/**
 * What a hosted call does to a resource a purchase holds: runx Connect takes
 * the credential's fields from the provider's answer, moves its expiry, or
 * ends the hold, and the kernel sees only the hold's evidence.
 */
export interface OperationHold {
  readonly action: "take" | "refresh" | "release";
  /** The connector's `kind: purchase` credential. */
  readonly credential: string;
  /** The held resource: from the call's result on a take, its arguments otherwise. */
  readonly target: Template;
  /** One `$result` binding; a take and a refresh state it. */
  readonly expires_at?: Template;
  /** Each credential field and the `$response` binding on the write that fills it. */
  readonly fields?: { readonly [field: string]: Template };
}

/** Where runx Connect put a held value in a provider answer, as `{"$held": ...}`. */
export interface HoldEvidence {
  readonly target: string;
  readonly expires_at: string;
  readonly value_digest: string;
}

export interface HttpRequest {
  readonly id: string;
  readonly each?: Template;
  readonly method: HttpMethod;
  readonly url: Template;
  readonly query?: { readonly [name: string]: Template };
  readonly headers?: { readonly [name: string]: Template };
  readonly body?: Template;
  readonly body_encoding?: "json" | "form";
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
  | { readonly op: "field" | "reject_true" | "reject_present"; readonly path: string }
  | { readonly op: "project"; readonly fields: { readonly [field: string]: string } };

export interface HttpImplementation {
  readonly allowed_hosts: readonly string[];
  /** How each credential authenticates a request, by credential name. */
  readonly auth?: {
    readonly [credential: string]: HttpAuth;
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
  readonly approval?: "none" | "required";
  readonly idempotency_key_format?: "runx" | "uuid";
  readonly guarantees?: JsonObject;
  readonly amount?: "none" | { readonly units: Template; readonly currency: Template };
  readonly input_schema?: JsonObject;
  readonly result_fields?: readonly string[];
  /**
   * What a successful write produced: a locator rendered from the call's
   * arguments and result, one per element of the `each` result array.
   */
  readonly effect?: OperationEffect;
  /** What the call does to a resource a purchase holds. */
  readonly holds?: OperationHold;
  /** Present when the hosted placement offers the operation. */
  readonly hosted?: { readonly provider_scopes?: readonly string[] };
  readonly implementation?: {
    readonly http?: HttpImplementation;
    readonly mcp?: JsonObject;
    readonly process?: {
      readonly tool: string;
      readonly runtime_scopes?: readonly string[];
      readonly readback?: { readonly matches: readonly JsonValue[] };
      readonly result_from?: "acknowledgement" | "readback";
    };
    readonly web?: JsonObject;
    readonly store?: JsonObject;
    readonly artifact?: JsonObject;
  };
}

/** The native admission amount, resolved from the same inputs sent to the
 * provider. Currency/asset codes are canonical uppercase; no FX is implied. */
export function operationAmount(operation: ConnectorOperation, arguments_: JsonObject): { readonly units: number; readonly unit: string } | undefined {
  if (operation.amount === undefined || operation.amount === "none") return undefined;
  const inputs = { arguments: arguments_ };
  const units = renderTemplate(operation.amount.units, inputs);
  const currency = renderTemplate(operation.amount.currency, inputs);
  if (typeof units !== "number" || !Number.isSafeInteger(units) || units < 0) throw new TemplateError("the operation's amount units must be a portable nonnegative integer");
  if (typeof currency !== "string" || !/^[A-Za-z][A-Za-z0-9]{2,15}$/.test(currency)) throw new TemplateError("the operation's amount currency must be an ASCII currency or asset code");
  return { units, unit: currency.toUpperCase() };
}

/** The request that names whose authority a session or a delegated grant holds. */
export interface PrincipalRequest {
  /** An https URL the authority may be sent to. */
  readonly url: string;
  /** Where the answer carries the account's stable id. */
  readonly id: string;
  /** Where the answer carries the account's name. */
  readonly login: string;
}

export interface ConnectorManifest {
  readonly schema: "runx.connector.v1";
  /** The connector's whole name: `stripe`, `google/calendar`. */
  readonly id: string;
  readonly name: string;
  readonly description: string;
  /** A connector category of the registry taxonomy; absent means uncategorized. */
  readonly category?: string;
  /** Whether the connector is the runtime's own capability rather than an outside service. */
  readonly runtime_capability: boolean;
  readonly auth: {
    readonly credentials?: { readonly [name: string]: CredentialDeclaration };
    /** A grant runx Connect brokers, naming its account by its principal request. */
    readonly delegated?: { readonly principal: PrincipalRequest };
    readonly session?: JsonObject;
    readonly none?: JsonObject;
  };
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
  readonly references?: readonly ReferenceDeclaration[];
  /** What a payment connector pays in, so a purchase offers it only for a quote it can pay. */
  readonly pays?: { readonly [family: string]: PaymentCompatibility };
}

/** Rust-owned connector payment compatibility, projected without provider names. */
export interface PaymentCompatibility {
  readonly methods?: readonly string[];
  readonly networks?: readonly string[];
  readonly currencies?: readonly string[];
}

/** Discovery only: the selected operation still admits the exact payment/account. */
export function connectorPays(
  connector: Pick<ConnectorManifest, "pays">,
  terms: { readonly family: string; readonly method?: string; readonly network?: string; readonly currency?: string },
): boolean {
  if (connector.pays === undefined || !Object.hasOwn(connector.pays, terms.family)) return false;
  const compatibility = connector.pays?.[terms.family];
  if (!compatibility) return false;
  return (compatibility.methods === undefined || (terms.method !== undefined && compatibility.methods.includes(terms.method)))
    && (compatibility.networks === undefined || (terms.network !== undefined && compatibility.networks.includes(terms.network)))
    && (compatibility.currencies === undefined || (terms.currency !== undefined && compatibility.currencies.includes(terms.currency)));
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

/** The reference a write leaves for what it produced. */
export interface OperationEffect {
  readonly kind: ReferenceKind;
  readonly locator: Template;
  readonly each?: Template;
}

/** One kind of thing a connector's locators name. */
export interface ReferenceDeclaration {
  readonly kind: ReferenceKind;
  /** A regular expression the whole locator matches. */
  readonly locator: string;
}

const referencePatterns = new WeakMap<readonly ReferenceDeclaration[], readonly (readonly [ReferenceKind, RegExp])[]>();

/**
 * What `locator` names, from the reference kinds `connector` declares, as the
 * kernel classifies it: the first declaration whose locator pattern matches
 * the whole locator, and an external URL when none does.
 */
export function referenceKind(connector: Pick<ConnectorManifest, "references">, locator: string): ReferenceKind {
  const declarations = connector.references ?? [];
  let patterns = referencePatterns.get(declarations);
  if (!patterns) {
    patterns = declarations.map((reference) => [reference.kind, new RegExp(`^(?:${reference.locator})$`, "u")] as const);
    referencePatterns.set(declarations, patterns);
  }
  return patterns.find(([, pattern]) => pattern.test(locator))?.[0] ?? "external_url";
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
  /** The element a repeated request is sent for, or an effect rendered for. */
  readonly item?: JsonValue;
  /** The call's decoded result, which only an operation's effect reads. */
  readonly result?: JsonValue;
  readonly idempotencyKey?: string;
  /**
   * The purchase the call runs under. runx Connect supplies it only
   * from the run its gateway session authenticates; the kernel never does.
   */
  readonly purchaseId?: string;
}

const MAX_DEPTH = 32;
const MAX_CONCAT_BYTES = 512;
const BINDINGS = new Set(["$argument", "$parameter", "$response", "$item", "$result", "$runx", "$concat", "$minor_units"]);

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
    case "$result":
      return found(inputs.result === undefined ? undefined : pointer(inputs.result, reference));
    case "$runx":
      if (reference === "idempotency_key") return found(inputs.idempotencyKey);
      if (reference === "purchase") return found(inputs.purchaseId);
      throw new TemplateError(`template binding $runx ${JSON.stringify(reference)} is not a runtime value`);
    case "$minor_units": {
      const invalid = () => new TemplateError("template $minor_units needs a canonical decimal, scale 0 to 18, and a portable integer result");
      if (argument === null || typeof argument !== "object" || Array.isArray(argument)
        || Object.keys(argument).length !== 2 || !Object.hasOwn(argument, "decimal") || !Object.hasOwn(argument, "scale")) throw invalid();
      const fields = argument as JsonObject;
      const decimal = render(fields.decimal!, inputs, depth + 1);
      const scale = render(fields.scale!, inputs, depth + 1);
      if (typeof decimal !== "string" || decimal.length > 64 || typeof scale !== "number" || !Number.isInteger(scale) || scale < 0 || scale > 18) throw invalid();
      const pattern = scale === 0 ? /^(0|[1-9][0-9]*)$/ : new RegExp(`^(0|[1-9][0-9]*)\\.([0-9]{${scale}})$`);
      if (!pattern.test(decimal)) throw invalid();
      const units = BigInt(decimal.replace(".", ""));
      if (units > BigInt(Number.MAX_SAFE_INTEGER)) throw invalid();
      return Number(units);
    }
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
  /** Serialized wire bytes, whether JSON or form encoded. */
  readonly body?: string;
}

export interface DeclaredRequestCall {
  readonly arguments: JsonObject;
  readonly parameters?: JsonObject;
  readonly idempotencyKey?: string;
  /** The purchase the call runs under, which only runx Connect supplies. */
  readonly purchaseId?: string;
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
    ...(call.purchaseId === undefined ? {} : { purchaseId: call.purchaseId }),
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
  // As the kernel does: a fragment never reaches the server, so a URL
  // carrying one names a request other than the one sent.
  if (url.href.includes("#")) throw new TemplateError(`request ${request.id} has a URL fragment`);
  const host = url.hostname.toLowerCase();
  if (url.protocol !== "https:" || !http.allowed_hosts.some((allowed) => hostAllowed(allowed, host))) {
    throw new TemplateError(`request ${request.id} targets a host the implementation does not allow`);
  }
  let href = url.href;
  const queryNames = Object.keys(request.query ?? {}).sort();
  if (queryNames.length > 0) {
    // As the kernel does: a query value whose argument the call does not
    // carry is not sent, and a request that sends none has no query.
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
    if (serialized) href = url.search === "" ? `${href.replace(/\?$/, "")}?${serialized}` : `${href}&${serialized}`;
  }
  const headers: Record<string, string> = {};
  for (const name of Object.keys(request.headers ?? {}).sort()) {
    const value = renderTemplate(request.headers![name]!, inputs);
    if (typeof value !== "string") throw new TemplateError("template rendered a non-string where a string was required");
    headers[name] = value;
  }
  if (request.body === undefined) return { method: request.method, url: href, headers };
  const body = renderTemplate(request.body, inputs);
  for (const name of Object.keys(headers)) {
    if (name.toLowerCase() === "content-type") delete headers[name];
  }
  if (request.body_encoding === "form") {
    if (body === null || typeof body !== "object" || Array.isArray(body)) throw new TemplateError("a form body must render to an object");
    const form = new URLSearchParams();
    for (const name of Object.keys(body).sort()) {
      const value = (body as JsonObject)[name];
      if ((typeof value !== "string" && typeof value !== "number" && typeof value !== "boolean") || (typeof value === "number" && !Number.isFinite(value))) throw new TemplateError("a form field must render to a string, number or boolean");
      form.append(name, String(value));
    }
    headers["content-type"] = "application/x-www-form-urlencoded";
    return { method: request.method, url: href, headers, body: form.toString() };
  }
  headers["content-type"] = "application/json";
  return { method: request.method, url: href, headers, body: JSON.stringify(body) };
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
