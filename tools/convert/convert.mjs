#!/usr/bin/env node
// Converts runx packages and connectors to the 0.12 format: run.yaml, its tests,
// its evals and its support graphs, and connector.yaml. The format and connector
// contracts (plans/0.12/contracts/run-yaml.md, connector-yaml.md) own every rule here. Every key the converter reads is named below, and a
// package holding any other key is refused, never guessed at. Comments survive.
//
//   node tools/convert/convert.mjs [--write] <package-or-connector-dir>...
//
// Without --write it changes nothing and reports what it would do.

import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import YAML, { isMap, isScalar, isSeq, Pair, Scalar, YAMLMap, YAMLSeq } from "yaml";

const MINIMUM_RUNX = "0.12.0";
const OUTPUT = { lineWidth: 0, flowCollectionPadding: false };

// A file keeps the list style most of its lists use, so a diff shows only what changed.
function styleOf(text) {
  const flush = (text.match(/^( *)[^\s#-][^\n]*:[ \t]*\n\1- /gm) ?? []).length;
  const indented = (text.match(/^( *)[^\s#-][^\n]*:[ \t]*\n\1  - /gm) ?? []).length;
  return { ...OUTPUT, indentSeq: indented >= flush };
}

const ROOT_KEYS = ["skill", "version", "catalog", "connectors", "tools", "skills", "asks", "input_definitions", "marketplace", "runners", "harness"];
const CATALOG_KEPT = ["visibility", "category", "tags"];
const CATALOG_GONE = ["audience", "role", "links", "kind", "execution", "completion", "approval", "publisher", "provider", "part_of", "canonical_skill"];
const CONNECTOR_KEYS = ["operations", "connector_from", "allowed_connectors", "manifest_digest"];
const METHOD_KEYS = ["asks", "default", "continuations", "description", "examples", "inputs", "outputs", "operations", "policy", "result_from", "skills", "steps", "tools"];
const STEP_KINDS = ["agent", "tool", "operation", "ask", "skill"];
const STEP_COMMON = ["id", "label", "when"];
const STEP_KIND_KEYS = {
  agent: ["inputs", "outputs", "artifacts", "context_skills"],
  tool: ["inputs", "outputs", "artifacts", "scopes"],
  skill: ["runner", "inputs", "act", "artifacts", "scopes"],
  ask: ["inputs"],
  operation: ["operation_name", "input", "approval", "amount", "expected_result", "result_fields", "optional_result_fields", "ephemeral_result_paths", "readback_of", "idempotency_key", "act", "duplicates"],
};
const TOOL_KEYS = ["module", "export", "requires", "timeout_seconds", "environment", "pages", "command", "args", "cwd", "input_mode"];
const PAGES_KEYS = { artifact_from: "artifact", path_from: "path", path_scope_from: "path_scope", framing: "framing", media_type: "media_type", page_bytes: "page_bytes" };
const INPUT_KEYS = ["type", "required", "description", "default", "artifact", "packet", "schema"];
const OUTPUT_KEYS = ["type", "description", "required", "enum", "schema"];
const RESULT_KEYS = { result_fields: "required", optional_result_fields: "optional", expected_result: "equals", ephemeral_result_paths: "ephemeral" };
const CASE_KEYS = ["name", "kind", "target", "runner", "inputs", "env", "setup", "caller", "expect", "metadata", "operator_journeys"];
const CALLER_TO = { act: ["host", "act"], answers: ["host", "answers"], turns: ["host", "turns"], approvals: ["host", "approvals"], http_exchanges: ["providers", "http"], mcp_calls: ["providers", "mcp"], process_calls: ["providers", "processes"], provider_responses: ["providers", "hosted"] };
const TEST_ORDER = ["name", "method", "package", "setup", "inputs", "env", "host", "providers", "expect"];
const GRAPH_KEYS = ["name", "steps", "publisher"];

class Refusal extends Error {}
const refuse = (message) => { throw new Refusal(message); };

// ---- YAML nodes

const keyOf = (pair) => (isScalar(pair.key) ? pair.key.value : pair.key);
const pairOf = (map, key) => map.items.find((pair) => keyOf(pair) === key);
const has = (map, key) => pairOf(map, key) !== undefined;
const node = (map, key) => pairOf(map, key)?.value;
const plain = (value) => (value && typeof value.toJSON === "function" ? value.toJSON() : value);
const read = (map, key) => plain(node(map, key));

function take(map, key) {
  const index = map.items.findIndex((pair) => keyOf(pair) === key);
  if (index < 0) return undefined;
  const [pair] = map.items.splice(index, 1);
  return pair.value;
}

function rename(map, from, to) {
  const pair = pairOf(map, from);
  if (!pair) return false;
  if (has(map, to)) refuse(`both ${from} and ${to} are set`);
  if (isScalar(pair.key)) pair.key.value = to;
  else pair.key = new Scalar(to);
  return true;
}

function insertAt(map, index, key, value) {
  map.items.splice(Math.max(0, Math.min(index, map.items.length)), 0, new Pair(new Scalar(key), value));
}

function indexOf(map, key) {
  return map.items.findIndex((pair) => keyOf(pair) === key);
}

function only(map, allowed, where) {
  if (!isMap(map)) refuse(`${where} is not a map`);
  for (const pair of map.items) {
    if (!allowed.includes(keyOf(pair))) refuse(`${where}.${keyOf(pair)} is not a key the converter knows`);
  }
}

function flow(doc, value) {
  const created = doc.createNode(value);
  if (isMap(created) || isSeq(created)) created.flow = true;
  return created;
}

// ---- Fields: inputs and outputs

// A field is its JSON Schema, required unless it says `optional: true`.
// Today an input defaults to an optional string and an output to a required
// value of any type, so those defaults are written out where they applied.
function convertField(doc, value, role, where) {
  if (role === "output" && isScalar(value)) return flow(doc, { type: value.value });
  if (value === null || value === undefined || (isScalar(value) && value.value === null)) {
    if (role === "input") return flow(doc, { type: "string", optional: true });
    refuse(`${where} is empty`);
  }
  only(value, role === "input" ? INPUT_KEYS : OUTPUT_KEYS, where);
  if (has(value, "artifact")) refuse(`${where}.artifact is gone: an input that takes an artifact names its packet, written by hand`);

  const fieldType = read(value, "type");
  const required = read(value, "required");
  const schema = take(value, "schema");
  const out = new YAMLMap();
  out.flow = value.flow;
  if (schema !== undefined && !isMap(schema)) refuse(`${where}.schema is not a map`);
  const schemaType = schema ? read(schema, "type") : undefined;

  let type;
  if (fieldType === undefined) type = schemaType ?? (role === "input" ? "string" : undefined);
  else if (fieldType === "json") type = schemaType;
  else {
    if (schemaType !== undefined && schemaType !== fieldType) refuse(`${where} says type ${fieldType} and its schema says ${schemaType}`);
    type = fieldType;
  }
  if (type !== undefined) {
    const typePair = pairOf(value, "type");
    out.items.push(typePair && plain(typePair.value) === type ? typePair : new Pair(new Scalar("type"), new Scalar(type)));
  }
  for (const pair of value.items) {
    const key = keyOf(pair);
    if (key === "type" || key === "required") continue;
    out.items.push(pair);
  }
  for (const pair of schema?.items ?? []) {
    const key = keyOf(pair);
    if (key === "type") continue;
    if (has(out, key)) {
      if (JSON.stringify(read(out, key)) !== JSON.stringify(plain(pair.value))) refuse(`${where}.${key} differs from its schema's ${key}`);
      continue;
    }
    out.items.push(pair);
  }
  const optional = role === "input" ? required !== true : required === false;
  if (optional) out.items.push(new Pair(new Scalar("optional"), new Scalar(true)));
  return out;
}

function convertFields(doc, map, role, where) {
  if (map === undefined) return;
  if (!isMap(map)) refuse(`${where} is not a map`);
  for (const pair of map.items) pair.value = convertField(doc, pair.value, role, `${where}.${keyOf(pair)}`);
}

// ---- Steps

function stepKind(step, where) {
  const kinds = STEP_KINDS.filter((kind) => has(step, kind));
  if (kinds.length !== 1) refuse(`${where} has ${kinds.length === 0 ? "no kind" : `kinds ${kinds.join(", ")}`}`);
  return kinds[0];
}

function convertStep(doc, step, scope, notes) {
  const id = read(step, "id");
  const where = `${scope.where}.steps.${id}`;
  if (!isMap(step)) refuse(`${where} is not a map`);
  const kind = stepKind(step, where);
  only(step, [...STEP_COMMON, kind, ...STEP_KIND_KEYS[kind]], where);
  take(step, "label");
  take(step, "scopes");

  if (kind === "agent") {
    const agent = node(step, "agent");
    if (isMap(agent)) {
      only(agent, ["role", "task"], `${where}.agent`);
      pairOf(step, "agent").value = new Scalar(read(agent, "task"));
    }
    scope.tasks.add(read(step, "agent"));
    const contextSkills = take(step, "context_skills");
    if (contextSkills !== undefined) {
      const context = new YAMLMap();
      context.items.push(new Pair(new Scalar("skills"), contextSkills));
      insertAt(step, step.items.length, "context", context);
    }
    const calls = scope.calls(doc);
    if (calls) insertAt(step, indexOf(step, "agent") + 1, "calls", calls);
  }
  if (kind === "tool") {
    const tool = node(step, "tool");
    if (isMap(tool)) {
      only(tool, TOOL_KEYS, `${where}.tool`);
      const pages = node(tool, "pages");
      if (pages !== undefined) {
        only(pages, Object.keys(PAGES_KEYS), `${where}.tool.pages`);
        for (const [from, to] of Object.entries(PAGES_KEYS)) if (from !== to) rename(pages, from, to);
        rename(tool, "pages", "paged_input");
      }
      rename(tool, "input_mode", "inputs_via");
    }
  }
  if (kind === "skill") {
    const method = take(step, "runner");
    if (method !== undefined) pairOf(step, "skill").value = new Scalar(`${read(step, "skill")}#${plain(method)}`);
  }
  if (kind === "operation") convertOperation(doc, step, scope, where, notes);
  if (kind === "ask") only(node(step, "ask"), ["gate", "reason", "type"], `${where}.ask`);
  convertOutputs(doc, step, where);
}

function convertOperation(doc, step, scope, where, notes) {
  const chosen = take(step, "operation_name");
  if (chosen !== undefined) {
    const requirement = read(step, "operation");
    const from = scope.requirementOperations.get(requirement);
    if (!from) refuse(`${where} chooses an operation of ${requirement}, which declares no operations`);
    const operation = new YAMLMap();
    operation.items.push(new Pair(new Scalar("connector"), new Scalar(requirement)), new Pair(new Scalar("choose"), chosen), new Pair(new Scalar("from"), flow(doc, from)));
    pairOf(step, "operation").value = operation;
  }
  rename(step, "input", "inputs");
  let resultAt = -1;
  const result = new YAMLMap();
  for (const [from, to] of Object.entries(RESULT_KEYS)) {
    const index = indexOf(step, from);
    if (index < 0) continue;
    if (resultAt < 0 || index < resultAt) resultAt = index;
  }
  for (const [from, to] of Object.entries(RESULT_KEYS)) {
    const value = take(step, from);
    if (value !== undefined) result.items.push(new Pair(new Scalar(to), value));
  }
  if (result.items.length > 0) insertAt(step, resultAt, "result", result);
  const readback = read(step, "readback_of");
  if (readback !== undefined) {
    const match = typeof readback === "string" && /^\$steps\.([^.]+)\.provider_operation$/.exec(readback);
    if (!match) refuse(`${where}.readback_of is ${JSON.stringify(readback)}, not a step's provider_operation`);
    rename(step, "readback_of", "confirms");
    pairOf(step, "confirms").value = new Scalar(match[1]);
  }
  if (take(step, "amount") !== undefined) notes.push(`${where}.amount removed: admission derives it from the operation's inputs`);
  // Whether recovery may resend a write whose outcome is unknown says what it
  // permits; `forbidden` was the default, so it says nothing.
  const duplicatesAt = indexOf(step, "duplicates");
  const duplicates = take(step, "duplicates");
  if (duplicates !== undefined) {
    const value = plain(duplicates);
    if (value === "visible" || value === "allowed") insertAt(step, duplicatesAt, "retry_unknown_outcomes", new Scalar(true));
    else if (value !== "forbidden") refuse(`${where}.duplicates is ${JSON.stringify(value)}, which the converter doesn't know`);
  }
}

// An output with a packet is an artifact. A named emit with no packet becomes
// a plain output, and `wrap_as` becomes `result_as` with its packet on that output.
function convertOutputs(doc, step, where) {
  const artifacts = take(step, "artifacts");
  let outputs = node(step, "outputs");
  convertFields(doc, outputs, "output", `${where}.outputs`);
  if (artifacts === undefined) return;
  only(artifacts, ["named_emits", "packets", "wrap_as", "packet"], `${where}.artifacts`);
  const named = read(artifacts, "named_emits") ?? {};
  const packets = read(artifacts, "packets") ?? {};
  const wrapAs = read(artifacts, "wrap_as");
  const wrapPacket = read(artifacts, "packet");
  for (const name of Object.keys(packets)) if (!(name in named)) refuse(`${where}.artifacts.packets.${name} has no named emit`);
  if (wrapPacket !== undefined && wrapAs === undefined) refuse(`${where}.artifacts.packet has no wrap_as`);

  const field = (name) => {
    if (outputs === undefined) {
      outputs = new YAMLMap();
      insertAt(step, step.items.length, "outputs", outputs);
    }
    let value = node(outputs, name);
    if (value === undefined) {
      value = flow(doc, {});
      outputs.items.push(new Pair(new Scalar(name), value));
      return { value, created: true };
    }
    return { value, created: false };
  };
  const setPacket = (map, packet, name) => {
    const existing = read(map, "packet");
    if (existing !== undefined && existing !== packet) refuse(`${where}.outputs.${name} has two packets`);
    if (existing === undefined) map.items.push(new Pair(new Scalar("packet"), new Scalar(packet)));
  };
  for (const name of Object.keys(named)) {
    const packet = packets[name];
    const { value, created } = field(name);
    if (packet !== undefined) setPacket(value, packet, name);
    // Today an emitted field that isn't returned is skipped, so one the step didn't declare is optional.
    if (created) value.items.push(new Pair(new Scalar("optional"), new Scalar(true)));
  }
  if (wrapAs !== undefined) {
    insertAt(step, indexOf(step, "outputs") >= 0 ? indexOf(step, "outputs") + 1 : step.items.length, "result_as", new Scalar(wrapAs));
    if (wrapPacket !== undefined) setPacket(field(wrapAs).value, wrapPacket, wrapAs);
  }
}

// ---- Offers: what a paid method charges, on the method itself

const OFFER_KEYS = ["price", "currency", "accepted_settlement_families", "input_schema_digest", "output_schema_digest", "acts_on"];
const PRICE_KEYS = ["model", "amount_minor", "selectors", "cases", "artifact_input", "base_amount_minor", "unit_bytes", "unit_amount_minor", "max_bytes"];
const PRICE_MODELS = { fixed: "fixed", input_choice: "by_input", artifact_bytes: "by_size" };

function convertOffer(doc, offer, where) {
  only(offer, OFFER_KEYS, where);
  // The registry computes these from the method's own inputs and outputs at publish.
  take(offer, "input_schema_digest");
  take(offer, "output_schema_digest");
  rename(offer, "accepted_settlement_families", "accepts");
  rename(offer, "acts_on", "requires_lease");
  const price = node(offer, "price");
  only(price, PRICE_KEYS, `${where}.price`);
  const modelPair = pairOf(price, "model");
  const model = PRICE_MODELS[plain(modelPair?.value)];
  if (model === undefined) refuse(`${where}.price.model is ${plain(modelPair?.value)}, which the converter doesn't know`);
  modelPair.value = new Scalar(model);
  modelPair.key = new Scalar("basis");
  rename(price, "artifact_input", "input");
  if (model === "by_input") {
    const selectors = plain(take(price, "selectors"));
    if (!Array.isArray(selectors) || selectors.length === 0) refuse(`${where}.price has no selectors`);
    for (const priced of node(price, "cases")?.items ?? []) {
      only(priced, ["when", "amount_minor"], `${where}.price.cases`);
      const values = read(priced, "when");
      if (!Array.isArray(values) || values.length !== selectors.length) refuse(`${where}.price.cases has a when that doesn't match its selectors`);
      const inputs = flow(doc, Object.fromEntries(selectors.map((name, index) => [name, values[index]])));
      const when = pairOf(priced, "when");
      when.key.value = "inputs";
      when.value = inputs;
    }
  }
  return offer;
}

// ---- Methods

function allowanceCalls(allowance) {
  return (doc) => {
    const calls = {};
    if (allowance.tools?.length) calls.tools = allowance.tools;
    if (allowance.operations?.length) calls.operations = allowance.operations;
    const skills = (allowance.skills ?? []).filter((entry) => typeof entry === "string");
    const load = (allowance.skills ?? []).filter((entry) => typeof entry !== "string");
    if (skills.length) calls.skills = skills;
    if (load.length) calls.load = load;
    if (allowance.asks === true) calls.ask = true;
    if (Object.keys(calls).length === 0) return undefined;
    const map = doc.createNode(calls);
    for (const pair of map.items) if (isSeq(pair.value)) pair.value.flow = true;
    return map;
  };
}

function convertMethod(doc, name, method, context, notes) {
  const where = `methods.${name}`;
  only(method, METHOD_KEYS, where);
  if (has(method, "continuations")) refuse(`${where} has continuations: D42 makes it a run that waits, written by hand`);

  const inputs = node(method, "inputs");
  if (inputs !== undefined) {
    if (!isMap(inputs)) refuse(`${where}.inputs is not a map`);
    for (const pair of inputs.items) {
      const input = keyOf(pair);
      const definition = isMap(pair.value) ? read(pair.value, "definition") : undefined;
      if (definition === undefined) {
        pair.value = convertField(doc, pair.value, "input", `${where}.inputs.${input}`);
        continue;
      }
      if (pair.value.items.length !== 1) refuse(`${where}.inputs.${input} sets keys beside its definition`);
      const shared = context.shared.get(definition);
      if (!shared) refuse(`${where}.inputs.${input} names definition ${definition}, which isn't defined`);
      if (definition === input) pair.value = flow(doc, {});
      else {
        pair.value = shared.clone();
        notes.push(`${where}.inputs.${input} copies shared input ${definition}: one name for both is its owner's call`);
      }
    }
  }
  const allowance = {
    tools: has(method, "tools") ? plain(take(method, "tools")) : context.root.tools,
    skills: has(method, "skills") ? plain(take(method, "skills")) : context.root.skills,
    asks: has(method, "asks") ? plain(take(method, "asks")) : context.root.asks,
    operations: plain(take(method, "operations")) ?? [],
  };
  const policy = take(method, "policy");
  let guards = [];
  if (policy !== undefined) {
    only(policy, ["guards"], `${where}.policy`);
    guards = plain(node(policy, "guards")) ?? [];
  }

  let steps = node(method, "steps");
  if (steps === undefined) {
    // Today's parser makes this method one agent turn named for the method,
    // given the method's input and answering in its outputs
    // (runner_definition.rs), and a test's answers are keyed by that name.
    // So the step keeps the name, binds $input and takes the outputs, which
    // the method then returns as its result step's.
    const turn = doc.createNode({ id: name, agent: name, inputs: "$input" });
    const outputs = take(method, "outputs");
    if (outputs !== undefined) turn.items.push(new Pair(new Scalar("outputs"), outputs));
    steps = new YAMLSeq();
    steps.items.push(turn);
    insertAt(method, method.items.length, "steps", steps);
    notes.push(`${where} had no steps: it's now one agent step, the task "${name}"`);
  }
  convertFields(doc, node(method, "outputs"), "output", `${where}.outputs`);
  if (!isSeq(steps)) refuse(`${where}.steps is not a list`);
  const scope = { where, calls: allowanceCalls(allowance), requirementOperations: context.requirementOperations, tasks: context.tasks };
  let agents = 0;
  for (const step of steps.items) {
    if (isMap(step) && has(step, "agent")) agents += 1;
    convertStep(doc, step, scope, notes);
  }
  if (agents === 0 && (allowance.tools?.length || allowance.operations.length || allowance.skills?.length || allowance.asks === true)) {
    notes.push(`${where} declared what a turn may call but has no agent step, so the allowance governed nothing and is dropped`);
  }

  for (const guard of guards) {
    const keys = Object.keys(guard);
    if (!keys.includes("step") || !keys.includes("value") || keys.length !== 3 || !(keys.includes("equals") || keys.includes("not_equals"))) {
      refuse(`${where}.policy.guards has an entry the converter doesn't know: ${JSON.stringify(guard)}`);
    }
    const target = steps.items.find((step) => read(step, "id") === guard.step);
    if (!target) refuse(`${where}.policy.guards names step ${guard.step}, which isn't in the method`);
    let list = node(target, "guard");
    if (list === undefined) {
      list = new YAMLSeq();
      const at = indexOf(target, "when") >= 0 ? indexOf(target, "when") + 1 : indexOf(target, "id") + 1;
      insertAt(target, at, "guard", list);
    }
    const { step: _ignored, ...condition } = guard;
    list.items.push(flow(doc, condition));
  }
}

// ---- Tests and evals

const slug = (value) => String(value).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");

function convertCase(doc, testCase, where, packageOf, evals, notes) {
  only(testCase, CASE_KEYS, where);
  const name = read(testCase, "name");
  if (typeof name !== "string" || !name) refuse(`${where} has no name`);
  take(testCase, "metadata");
  const kind = plain(take(testCase, "kind"));
  const target = plain(take(testCase, "target"));
  if (kind === "graph") {
    const fixture = packageOf(target, where);
    insertAt(testCase, 1, "package", new Scalar(fixture));
  } else if (!(kind === undefined || (kind === "skill" && (target === ".." || target === undefined)))) {
    refuse(`${where} targets ${kind} ${JSON.stringify(target)}, not its own package or a support graph`);
  }
  rename(testCase, "runner", "method");

  const caller = take(testCase, "caller");
  const host = new YAMLMap();
  const providers = new YAMLMap();
  if (caller !== undefined) {
    only(caller, Object.keys(CALLER_TO), `${where}.caller`);
    for (const pair of caller.items) {
      const [into, key] = CALLER_TO[keyOf(pair)];
      (into === "host" ? host : providers).items.push(new Pair(new Scalar(key), pair.value));
    }
  }
  // What Connect already holds for the case's principal: its leases.
  const hosted = node(providers, "hosted");
  if (hosted !== undefined && isMap(hosted)) rename(hosted, "holds", "leases");
  if (host.items.length) testCase.items.push(new Pair(new Scalar("host"), host));
  if (providers.items.length) testCase.items.push(new Pair(new Scalar("providers"), providers));

  const journeys = plain(take(testCase, "operator_journeys")) ?? [];
  const method = read(testCase, "method");
  for (const journey of journeys) evals.push({ ...journey, case: name, method });

  testCase.items.sort((a, b) => TEST_ORDER.indexOf(keyOf(a)) - TEST_ORDER.indexOf(keyOf(b)));
  return name;
}

function writeEvals(skillName, journeys, files, dir, notes) {
  const evals = [];
  let composed = 0;
  let confusors = 0;
  for (const journey of journeys) {
    const known = ["mode", "request", "expected_outcome", "must_not_repeat", "prior_evidence", "confusors", "exercises_runner", "case", "method"];
    for (const key of Object.keys(journey)) if (!known.includes(key)) refuse(`operator_journeys in ${journey.case} has ${key}, which the converter doesn't know`);
    if (journey.confusors?.length) confusors += 1;
    if (journey.prior_evidence?.length || journey.mode === "composed") {
      composed += 1;
      continue;
    }
    const entry = { id: evals.length + 1, prompt: journey.request, expected_output: journey.expected_outcome, files: [] };
    if (journey.must_not_repeat?.length) entry.expectations = journey.must_not_repeat;
    const method = journey.exercises_runner ?? undefined;
    if (method) entry.method = method;
    evals.push(entry);
  }
  if (evals.length) files.set(path.join(dir, "evals", "evals.json"), `${JSON.stringify({ skill_name: skillName, evals }, null, 2)}\n`);
  if (composed) notes.push(`${composed} operator journeys build on an earlier run: each needs that run as a fixture before it's an eval (Stage 5)`);
  if (confusors) notes.push(`${confusors} journeys listed confusors, which are skill names, not queries: eval_queries.json is written fresh`);
}

// ---- Support graphs, which become fixture packages

function convertGraph(file, into, skillName, notes) {
  const graphText = readFileSync(file, "utf8");
  const doc = YAML.parseDocument(graphText);
  const root = doc.contents;
  only(root, GRAPH_KEYS, path.basename(file));
  if (has(root, "publisher")) notes.push(`${path.basename(file)}: publisher removed, as catalog.publisher is`);
  const graphName = read(root, "name");
  const steps = node(root, "steps");
  const out = new YAML.Document();
  out.contents = out.createNode({ minimum_runx: MINIMUM_RUNX, catalog: { visibility: "internal" }, methods: { run: {} } });
  const method = node(node(out.contents, "methods"), "run");
  method.items.push(new Pair(new Scalar("steps"), steps));
  const scope = { where: `${path.basename(file)}`, calls: () => undefined, requirementOperations: new Map(), tasks: new Set() };
  for (const step of steps.items) convertStep(out, step, scope, notes);
  if (scope.tasks.size) refuse(`${path.basename(file)} has agent steps, whose fixture package needs their task sections written by hand`);
  // A process runs in its package's directory, which is now `into`, so a
  // relative path naming a file beside the graph is rewritten to name the
  // same file from there.
  const relocate = (scalar, where) => {
    if (!(scalar instanceof Scalar) || typeof scalar.value !== "string" || !/^\.\.?\//.test(scalar.value)) return;
    const target = path.resolve(path.dirname(file), scalar.value);
    if (!existsSync(target)) return;
    const moved = path.relative(into, target);
    const next = moved.startsWith(".") ? moved : `./${moved}`;
    if (next === scalar.value) return;
    notes.push(`${path.basename(file)} ${where}: ${scalar.value} is ${next} from the fixture package`);
    scalar.value = next;
  };
  for (const step of steps.items) {
    const tool = isMap(step) ? node(step, "tool") : undefined;
    if (!isMap(tool)) continue;
    const where = `step ${read(step, "id")}`;
    relocate(node(tool, "command"), where);
    relocate(node(tool, "cwd"), where);
    const args = node(tool, "args");
    if (isSeq(args)) for (const arg of args.items) relocate(arg, where);
  }
  const skill = `---\nname: ${graphName}\ndescription: A test fixture that composes ${skillName} with the packages it calls.\n---\n\nUsed only by ${skillName}'s tests.\n`;
  return { runYaml: out.toString(styleOf(graphText)), skill, graphName };
}


// ---- SKILL.md: one `## Agent task: <name>` section per agent task

const TASK_HEADING = /^## Agent task(?: contracts?)?(?::\s*(.+?))?\s*$/;
const unquote = (text) => text.trim().replace(/^`(.*)`$/, "$1").trim();

// Each agent step's task is a `## Agent task: <name>` section of SKILL.md,
// carrying its instructions. Today's manuals write them three ways: one
// unnamed `## Agent task contract`, a plural `## Agent task contracts` with a
// `### <task>` per task, or already `## Agent task: <name>`, some with the
// name in backticks. A `### <task>` heading elsewhere is promoted the same
// way. Each promoted task's own subheadings rise one level. A heading that
// matches no single task, or a task with no section, is a note for its owner.
function agentTaskSections(markdown, tasks, notes) {
  const lines = markdown.split("\n");
  let fence = null;
  const headings = [];
  lines.forEach((line, index) => {
    const marker = /^(```|~~~)/.exec(line);
    if (marker) fence = fence === null ? marker[1] : fence === marker[1] ? null : fence;
    else if (fence === null) {
      const heading = /^(#{1,6})\s+(.*)$/.exec(line);
      if (heading) headings.push({ index, level: heading[1].length, text: heading[2] });
    }
  });
  const sectioned = new Set();
  const named = new Map(); // line index -> task name
  const raise = new Set();
  const drop = new Set();
  const end = (from, level) => headings.find((heading) => heading.index > from && heading.level <= level)?.index ?? lines.length;
  const promote = (heading, task) => {
    named.set(heading.index, task);
    sectioned.add(task);
    for (const inner of headings) if (inner.index > heading.index && inner.index < end(heading.index, heading.level) && inner.level > heading.level) raise.add(inner.index);
  };
  for (const heading of headings.filter((heading) => heading.level === 2)) {
    const match = TASK_HEADING.exec(lines[heading.index]);
    if (!match) continue;
    if (match[1] !== undefined) {
      const task = unquote(match[1]);
      if (!tasks.has(task)) notes.push(`SKILL.md's "${lines[heading.index]}" names ${task}, which no agent step runs`);
      named.set(heading.index, task);
      sectioned.add(task);
      continue;
    }
    const inner = headings.filter((sub) => sub.index > heading.index && sub.index < end(heading.index, 2) && sub.level === 3 && tasks.has(unquote(sub.text)));
    if (inner.length) {
      drop.add(heading.index);
      for (const sub of inner) promote(sub, unquote(sub.text));
      continue;
    }
    const open = [...tasks].filter((task) => !sectioned.has(task));
    if (lines[heading.index].trim() === "## Agent task contract" && tasks.size === 1 && open.length === 1) {
      named.set(heading.index, open[0]);
      sectioned.add(open[0]);
    } else notes.push(`SKILL.md's "${lines[heading.index]}" matches no single agent task`);
  }
  for (const heading of headings.filter((heading) => heading.level === 3 && !named.has(heading.index))) {
    const task = unquote(heading.text);
    if (tasks.has(task) && !sectioned.has(task)) promote(heading, task);
  }
  for (const task of tasks) if (!sectioned.has(task)) notes.push(`agent task ${task} has no "## Agent task: ${task}" section in SKILL.md`);
  const out = [];
  lines.forEach((line, index) => {
    if (drop.has(index)) {
      if (lines[index + 1] === "") drop.add(index + 1);
      return;
    }
    if (named.has(index)) out.push(`## Agent task: ${named.get(index)}`);
    else if (raise.has(index)) out.push(line.slice(1));
    else out.push(line);
  });
  return out.join("\n");
}

// ---- A package

function skillName(dir) {
  const file = path.join(dir, "SKILL.md");
  if (!existsSync(file)) return undefined;
  const match = /^---\n([\s\S]*?)\n---/.exec(readFileSync(file, "utf8"));
  const name = match && /^name:\s*["']?([^"'\n]+?)["']?\s*$/m.exec(match[1]);
  return name ? name[1] : undefined;
}

function convertPackage(dir) {
  const notes = [];
  const files = new Map(); // path -> new content, or null to delete
  const runPath = path.join(dir, "run.yaml");
  const runText = readFileSync(runPath, "utf8");
  const style = styleOf(runText);
  const doc = YAML.parseDocument(runText);
  if (doc.errors.length) refuse(`run.yaml doesn't parse: ${doc.errors[0].message}`);
  const root = doc.contents;
  only(root, ROOT_KEYS, "run.yaml");

  const declared = read(root, "skill");
  const name = skillName(dir);
  if (name === undefined) refuse("SKILL.md, which holds the package's name now, is missing or has no name");
  if (declared !== undefined && declared !== name) refuse(`run.yaml names ${declared} and SKILL.md names ${name}`);
  take(root, "skill");
  insertAt(root, has(root, "version") ? indexOf(root, "version") + 1 : 0, "minimum_runx", new Scalar(MINIMUM_RUNX));

  const catalog = node(root, "catalog");
  if (catalog !== undefined) {
    only(catalog, [...CATALOG_KEPT, ...CATALOG_GONE], "catalog");
    for (const key of CATALOG_GONE) take(catalog, key);
    if (catalog.items.length === 0) take(root, "catalog");
  }

  const requirementOperations = new Map();
  const connectors = node(root, "connectors");
  if (connectors !== undefined) {
    for (const pair of [...connectors.items]) {
      const requirement = keyOf(pair);
      if (pair.value === null || (isScalar(pair.value) && pair.value.value === null)) {
        take(connectors, requirement);
        continue;
      }
      only(pair.value, CONNECTOR_KEYS, `connectors.${requirement}`);
      const operations = plain(take(pair.value, "operations"));
      if (operations) requirementOperations.set(requirement, operations);
      rename(pair.value, "connector_from", "choose");
      rename(pair.value, "allowed_connectors", "from");
      if (pair.value.items.length === 0) take(connectors, requirement);
    }
    if (connectors.items.length === 0) take(root, "connectors");
  }

  const marketplace = take(root, "marketplace");
  const offers = new Map();
  if (marketplace !== undefined) {
    only(marketplace, ["offers"], "marketplace");
    for (const pair of node(marketplace, "offers")?.items ?? []) {
      offers.set(keyOf(pair), convertOffer(doc, pair.value, `marketplace.offers.${keyOf(pair)}`));
    }
  }

  const shared = new Map();
  if (rename(root, "input_definitions", "shared_inputs")) {
    for (const pair of node(root, "shared_inputs").items) {
      pair.value = convertField(doc, pair.value, "input", `shared_inputs.${keyOf(pair)}`);
      shared.set(keyOf(pair), pair.value);
    }
  }

  const rootAllowance = { tools: plain(take(root, "tools")), skills: plain(take(root, "skills")), asks: plain(take(root, "asks")) };
  const harness = take(root, "harness");

  if (!rename(root, "runners", "methods")) refuse("run.yaml has no runners");
  // Which method a bare reference runs is one name at the root, so `default`
  // keeps its one JSON Schema meaning, and two methods can't both claim it.
  const defaults = [];
  for (const pair of node(root, "methods").items) {
    const flag = isMap(pair.value) ? take(pair.value, "default") : undefined;
    if (flag !== undefined && plain(flag) === true) defaults.push(keyOf(pair));
    else if (flag !== undefined && plain(flag) !== false) refuse(`methods.${keyOf(pair)}.default is ${JSON.stringify(plain(flag))}`);
  }
  if (defaults.length > 1) refuse(`methods ${defaults.join(", ")} are each marked default`);
  // With one method a bare reference runs it, so the key has no job.
  if (defaults.length === 1 && node(root, "methods").items.length > 1) {
    insertAt(root, indexOf(root, "minimum_runx") + 1, "default_method", new Scalar(defaults[0]));
  }
  const tasks = new Set();
  for (const pair of node(root, "methods").items) {
    convertMethod(doc, keyOf(pair), pair.value, { root: rootAllowance, shared, requirementOperations, tasks }, notes);
  }
  for (const [method, offer] of offers) {
    const target = node(node(root, "methods"), method);
    if (!isMap(target)) refuse(`marketplace.offers.${method} names a method that isn't there`);
    insertAt(target, indexOf(target, "steps") >= 0 ? indexOf(target, "steps") : target.items.length, "offer", offer);
  }

  // Tests: the cases in run.yaml and in fixtures/*.yaml, and support graphs.
  const evals = [];
  const tests = new Map();
  const fixtures = path.join(dir, "fixtures");
  const graphPackages = new Map();
  const packageOf = (target, where) => {
    if (typeof target !== "string" || !/^support\/[^/]+\.graph$/.test(target)) refuse(`${where} targets graph ${JSON.stringify(target)}, not a support graph`);
    const file = path.join(fixtures, target);
    if (!existsSync(file)) refuse(`${where} targets ${target}, which doesn't exist`);
    const stem = path.basename(target, ".graph");
    if (!graphPackages.has(file)) {
      const into = path.join(fixtures, stem);
      if (existsSync(into)) refuse(`fixtures/${stem} already exists, so ${target} can't become a fixture package there`);
      graphPackages.set(file, { into, ...convertGraph(file, into, name, notes) });
    }
    return `fixtures/${stem}`;
  };
  const addTest = (testDoc, where, base) => {
    const caseName = convertCase(testDoc, testDoc.contents, where, packageOf, evals, notes);
    const file = path.join(dir, "tests", `${base ?? slug(caseName)}.yaml`);
    if (tests.has(file)) refuse(`two tests would both be ${path.relative(dir, file)}`);
    tests.set(file, testDoc);
  };
  if (harness !== undefined) {
    only(harness, ["cases", "files"], "harness");
    for (const file of plain(node(harness, "files")) ?? []) {
      if (!String(file).startsWith("fixtures/")) notes.push(`harness.files listed ${file}, outside fixtures/, which is the only directory tests stage`);
    }
    for (const testCase of node(harness, "cases")?.items ?? []) {
      const testDoc = new YAML.Document();
      testDoc.contents = testCase;
      testDoc.style = style;
      addTest(testDoc, `harness.cases.${read(testCase, "name")}`);
    }
  }
  if (existsSync(fixtures)) {
    for (const entry of readdirSync(fixtures).sort()) {
      if (!entry.endsWith(".yaml")) continue;
      const file = path.join(fixtures, entry);
      const caseText = readFileSync(file, "utf8");
      const testDoc = YAML.parseDocument(caseText);
      testDoc.style = styleOf(caseText);
      const contents = testDoc.contents;
      if (!isMap(contents) || !has(contents, "name") || !has(contents, "expect")) continue;
      addTest(testDoc, `fixtures/${entry}`, path.basename(entry, ".yaml"));
      files.set(file, null);
    }
  }
  for (const [file, testDoc] of tests) files.set(file, testDoc.toString(testDoc.style));
  for (const [file, graph] of graphPackages) {
    files.set(file, null);
    files.set(path.join(graph.into, "run.yaml"), graph.runYaml);
    files.set(path.join(graph.into, "SKILL.md"), graph.skill);
  }
  writeEvals(name, evals, files, dir, notes);
  files.set(runPath, doc.toString(style));
  const manualPath = path.join(dir, "SKILL.md");
  const manual = readFileSync(manualPath, "utf8");
  const sectioned = agentTaskSections(manual, tasks, notes);
  if (sectioned !== manual) files.set(manualPath, sectioned);
  return { files, notes, tests: tests.size, graphs: graphPackages.size };
}

// ---- A connector (plans/0.12/contracts/connector-yaml.md)

const CONNECTOR_ROOT_KEYS = ["schema", "id", "name", "description", "category", "auth", "parameters", "placements", "operations", "references", "pays", "runtime_capability"];
const OPERATION_KEYS = ["description", "verb", "scopes", "credentials", "target", "approval", "idempotency_key_format", "guarantees", "amount", "cost", "input_schema", "result_fields", "effect", "implementation", "holds", "hosted"];
const VERBS = { read: "read", write: "write", execute: "execute", revoke: "revoke", create: "write", comment: "write", delete: "write", merge: "write", update: "write", review: "write" };
const PAGE_KEYS = ["param", "in", "cursor", "items", "limit"];

// A lease is a provider-issued credential for one resource, which Connect holds
// for its owner. Each action carries only what it needs: a take names the new
// resource, and a renew or a release acts on the operation's own target.
const LEASE_ACTIONS = { take: "take", refresh: "renew", release: "release" };

function convertLease(operation, where) {
  const holds = pairOf(operation, "holds");
  if (!holds) return;
  only(holds.value, ["action", "credential", "target", "expires_at", "fields"], `${where}.holds`);
  const action = LEASE_ACTIONS[read(holds.value, "action")];
  if (action === undefined) refuse(`${where}.holds.action is ${read(holds.value, "action")}, which the converter doesn't know`);
  const lease = new YAMLMap();
  lease.items.push(new Pair(new Scalar(action), node(holds.value, "credential")));
  if (action === "take") {
    for (const key of ["target", "expires_at", "fields"]) {
      if (!has(holds.value, key)) refuse(`${where}.holds is a take with no ${key}`);
      lease.items.push(pairOf(holds.value, key));
    }
  } else {
    const target = read(holds.value, "target");
    const own = read(node(operation, "target") ?? new YAMLMap(), "template");
    if (JSON.stringify(target) !== JSON.stringify(own)) refuse(`${where}.holds names a target other than the operation's own, which a ${action} can't say`);
    if (has(holds.value, "fields")) refuse(`${where}.holds is a ${action} with fields, which only a take fills`);
    if (action === "renew") {
      if (!has(holds.value, "expires_at")) refuse(`${where}.holds is a renew with no expires_at`);
      lease.items.push(pairOf(holds.value, "expires_at"));
    } else if (has(holds.value, "expires_at")) refuse(`${where}.holds is a release with an expires_at`);
  }
  lease.flow = holds.value.flow;
  holds.key.value = "lease";
  holds.value = lease;
}

function renameProcessScopes(process) {
  if (process === undefined || !isMap(process)) return;
  rename(process, "runtime_scopes", "requires");
}

function convertConnector(dir) {
  const notes = [];
  const file = path.join(dir, "connector.yaml");
  const text = readFileSync(file, "utf8");
  const doc = YAML.parseDocument(text);
  if (doc.errors.length) refuse(`connector.yaml doesn't parse: ${doc.errors[0].message}`);
  const root = doc.contents;
  only(root, CONNECTOR_ROOT_KEYS, "connector.yaml");
  if (read(root, "schema") !== "runx.connector.v1") refuse(`connector.yaml's schema is ${read(root, "schema")}`);
  rename(root, "name", "title");
  rename(root, "id", "name");
  take(root, "runtime_capability");
  rename(root, "pays", "payment");
  // A protocol's `methods` competed with a skill's methods.
  for (const pair of node(root, "payment")?.items ?? []) if (isMap(pair.value)) rename(pair.value, "methods", "payment_methods");

  for (const pair of node(node(root, "auth") ?? new YAMLMap(), "credentials")?.items ?? []) {
    const kind = pairOf(pair.value, "kind");
    if (kind && plain(kind.value) === "purchase") kind.value = new Scalar("lease");
  }
  const session = node(node(root, "auth") ?? new YAMLMap(), "session");
  if (session !== undefined) renameProcessScopes(node(session, "process"));

  for (const pair of node(root, "operations")?.items ?? []) {
    const where = `operations.${keyOf(pair)}`;
    const operation = pair.value;
    only(operation, OPERATION_KEYS, where);
    const verb = pairOf(operation, "verb");
    if (verb) {
      const to = VERBS[plain(verb.value)];
      if (to === undefined) refuse(`${where}.verb is ${plain(verb.value)}, which the converter doesn't know`);
      verb.value = new Scalar(to);
    }
    const hosted = pairOf(operation, "hosted");
    if (hosted) {
      only(hosted.value, ["provider_scopes"], `${where}.hosted`);
      const scopes = node(hosted.value, "provider_scopes");
      const at = indexOf(operation, "hosted");
      take(operation, "hosted");
      if (scopes !== undefined) insertAt(operation, at, "provider_scopes", scopes);
    }
    convertLease(operation, where);
    const implementation = node(operation, "implementation");
    renameProcessScopes(implementation && node(implementation, "process"));
    const http = implementation && node(implementation, "http");
    const checks = ["preconditions", "readbacks"].flatMap((key) => (http && node(http, key)?.items) ?? []);
    const requests = [...((http && node(http, "requests")?.items) ?? []), ...checks.map((check) => node(check, "request")).filter(isMap)];
    for (const request of requests) {
      // `method` is a skill's method everywhere else in runx, and a check's
      // request is a request like any other.
      rename(request, "method", "http_method");
      const pages = node(request, "pages");
      if (pages === undefined) continue;
      only(pages, PAGE_KEYS, `${where}.implementation.http.requests.pages`);
      const next = new YAMLMap();
      next.flow = true;
      for (const key of ["cursor", "param", "in"]) {
        const value = take(pages, key);
        if (value !== undefined) next.items.push(new Pair(new Scalar(key), value));
      }
      if (next.items.length) pages.items.push(new Pair(new Scalar("next"), next));
    }
  }
  return { files: new Map([[file, doc.toString(styleOf(text))]]), notes };
}

// ---- Command line

function main(argv) {
  const write = argv.includes("--write");
  const dirs = argv.filter((arg) => arg !== "--write");
  if (dirs.length === 0) {
    process.stderr.write("usage: convert.mjs [--write] <package-or-connector-dir>...\n");
    return 2;
  }
  let refused = 0;
  for (const dir of dirs) {
    try {
      // A path that holds neither file, such as a namespace folder or a stray
      // match of a glob, is refused by name, and the rest still convert.
      const stat = statSync(dir, { throwIfNoEntry: false });
      if (stat === undefined) refuse("doesn't exist");
      if (!stat.isDirectory()) refuse("is not a directory");
      if (!existsSync(path.join(dir, "connector.yaml")) && !existsSync(path.join(dir, "run.yaml"))) {
        refuse("holds neither a run.yaml nor a connector.yaml, so it's not a package or a connector");
      }
      if (existsSync(path.join(dir, "connector.yaml"))) {
        const { files, notes } = convertConnector(dir);
        if (write) for (const [file, content] of files) writeFileSync(file, content);
        process.stdout.write(`converted ${dir}: connector${write ? "" : " (dry run)"}\n`);
        for (const note of notes) process.stdout.write(`  note: ${note}\n`);
        continue;
      }
      const { files, notes, tests, graphs } = convertPackage(dir);
      if (write) {
        for (const [file, content] of files) {
          if (content === null) rmSync(file);
          else {
            mkdirSync(path.dirname(file), { recursive: true });
            writeFileSync(file, content);
          }
        }
        const support = path.join(dir, "fixtures", "support");
        if (existsSync(support) && readdirSync(support).length === 0) rmSync(support, { recursive: true });
      }
      process.stdout.write(`converted ${dir}: ${tests} tests, ${graphs} fixture packages${write ? "" : " (dry run)"}\n`);
      for (const note of notes) process.stdout.write(`  note: ${note}\n`);
    } catch (error) {
      if (!(error instanceof Refusal)) throw error;
      refused += 1;
      process.stdout.write(`refused ${dir}: ${error.message}\n`);
    }
  }
  return refused === 0 ? 0 : 1;
}

process.exitCode = main(process.argv.slice(2));
