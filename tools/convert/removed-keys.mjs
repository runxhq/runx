// The keys 0.12's format contract removes, as the stale-name guard reads them:
// every delete, fold, move and rename row of the rename table
// (plans/0.12/rename-table.tsv) whose old side is a key in a file an author
// writes: run.yaml, a test, connector.yaml or a tool manifest, and the store,
// authority, context and GitHub keys those files pass. A key is stale where it
// is written as one: a YAML or JSON key, in a file or in a Markdown fence, a
// string in code that writes one, or a Markdown mention of a key no other sense
// shares. A code identifier is not a key, so a local or a module that shares a
// key's spelling isn't read. The CLI's flags, the schema names and the words in
// the table aren't keys, and the rename and the runner and harness guards own
// the words.

/** Where a key is written: which documents read it, under which parent. */
const RUN = "run.yaml";
const TEST = "test";
const CONNECTOR = "connector.yaml";
const MANIFEST = "manifest";
const ANY = [RUN, TEST, CONNECTOR, MANIFEST];

// A field a method declares, not a value a step or a test binds.
const INPUT_FIELD = /^(methods\.[^.]+\.)?(inputs|shared_inputs)\.[^.]+$/;
const FIELD = /^(methods\.[^.]+\.)?((inputs|outputs|shared_inputs)|steps\.-\.outputs)\.[^.]+$/;
const STEP = /(^|\.)steps\.-$/;

/**
 * Each removed key: the key, the documents and parent path (dotted keys, `-`
 * for a list item) it was written under, the value that made it the removed
 * form when the key itself stays, and the rename table's row.
 */
export const REMOVED_KEYS = [
  // run.yaml
  { key: "skill", in: [RUN], parent: /^$/, row: "run.yaml: skill (root) -> SKILL.md name" },
  { key: /^(audience|role|links|kind|execution|completion|approval|publisher|provider|part_of|canonical_skill)$/, in: [RUN], parent: /^catalog$/, row: "run.yaml: catalog.audience, role, links, kind, execution, completion, approval, publisher, provider, part_of, canonical_skill -> (none)" },
  { key: "input_definitions", in: [RUN], row: "run.yaml: input_definitions -> shared_inputs" },
  { key: /^(definition|schema)$/, in: [RUN], parent: INPUT_FIELD, row: "run.yaml: inputs.<name>.definition and .schema -> the input itself" },
  { key: "required", in: [RUN, MANIFEST], parent: FIELD, value: /^(true|false)$/, row: "run.yaml and manifest.json: an input's or output's required -> optional: true on the exception" },
  { key: "type", in: ANY, value: /^"?json"?$/, row: "run.yaml and manifest.json: type: json -> (no type)" },
  { key: "operations", in: [RUN], parent: /^connectors\.[^.]+$/, row: "run.yaml: connectors.<r>.operations -> (derived from steps)" },
  { key: /^(connector_from|allowed_connectors)$/, in: [RUN], parent: /^connectors\.[^.]+$/, row: "run.yaml: connectors.<r>.connector_from, allowed_connectors -> choose, from" },
  { key: "marketplace", in: [RUN], parent: /^$/, row: "run.yaml: marketplace.offers.<method> -> methods.<method>.offer" },
  { key: /^(selectors|artifact_input)$/, in: [RUN], parent: /(^|\.)price$/, row: "run.yaml: offer price.selectors, artifact_input -> cases[].inputs, input" },
  { key: "model", in: [RUN], parent: /(^|\.)price$/, value: /^(input_choice|artifact_bytes)$/, row: "run.yaml: offer price.model input_choice, artifact_bytes -> by_input, by_size" },
  { key: /^(input_schema_digest|output_schema_digest|accepted_settlement_families|acts_on)$/, in: [RUN], parent: /(^|\.)(offer|offers\.[^.]+)$/, row: "run.yaml: an offer's input_schema_digest, output_schema_digest (computed at publish), accepted_settlement_families (accepts), acts_on (requires_lease)" },
  { key: "artifact", in: [RUN], parent: INPUT_FIELD, value: /^true$/, row: "run.yaml: inputs.<name>.artifact: true -> (the input names its packet)" },
  { key: /^(tools|skills|asks|operations)$/, in: [RUN], parent: /^(methods\.[^.]+)?$/, row: "run.yaml: tools, skills, asks (root and method), operations (method) -> steps[].calls" },
  { key: "guards", in: [RUN], parent: /(^|\.)policy$/, row: "run.yaml: policy.guards -> steps[].guard" },
  { key: "continuations", in: [RUN], parent: /^(methods\.[^.]+)?$/, row: "run.yaml: continuations -> (a run that waits, D42)" },
  { key: "label", in: [RUN], parent: /^(methods\.[^.]+)?$|(^|\.)steps\.-$/, row: "run.yaml: label -> (none)" },
  { key: /^(scopes|amount|input)$/, in: [RUN], parent: STEP, row: "run.yaml: a step's scopes (from the tool), amount (from the operation), input (inputs)" },
  { key: "artifacts", in: [RUN], parent: /(^|\.)steps\.-$|^(methods\.[^.]+)?$/, row: "run.yaml: artifacts.named_emits, packets, wrap_as, packet -> result_as, and the outputs' packet" },
  { key: /^(named_emits|wrap_as|emits)$/, in: ANY, row: "run.yaml and manifest.json: artifacts.named_emits, emits, wrap_as -> outputs.<f>.packet, result_as" },
  { key: "context_skills", in: [RUN], row: "run.yaml: context_skills -> context.skills" },
  { key: /^(role|task)$/, in: [RUN], parent: /(^|\.)steps\.-\.agent$/, row: "run.yaml: agent: { role, task } -> agent: <task>" },
  { key: /^(catalog|project)$/, in: [RUN], parent: /(^|\.)skills\.-$/, row: "run.yaml: skills: [{ catalog | project }] -> calls.load" },
  { key: "operation_name", in: [RUN], row: "run.yaml: operation_name -> operation: { connector, choose, from }" },
  { key: /^(result_fields|optional_result_fields|expected_result|ephemeral_result_paths)$/, in: [RUN], parent: STEP, row: "run.yaml: result_fields, optional_result_fields, expected_result, ephemeral_result_paths -> result: { required, optional, equals, ephemeral }" },
  { key: "kind", in: [RUN, TEST], value: /^graph$/, row: "run.yaml: kind: graph files -> fixture packages" },
  { key: "pages", in: [RUN], parent: /(^|\.)tool$/, row: "run.yaml: a worker tool's pages -> paged_input" },
  { key: /^(artifact_from|path_from|path_scope_from)$/, in: [RUN], row: "run.yaml: pages.artifact_from, path_from, path_scope_from -> paged_input.artifact, path, path_scope" },
  { key: "input_mode", in: [RUN, MANIFEST], row: "run.yaml and manifest.json: input_mode -> inputs_via" },
  { key: "duplicates", in: [RUN], value: /^visible$/, row: "run.yaml: duplicates: visible -> allowed" },
  { key: "effects", in: [RUN], value: /^free$/, row: "run.yaml: effects: free -> allow" },
  { key: "readback_of", in: [RUN], row: "run.yaml: readback_of -> confirms" },
  { key: "asks", in: [RUN], value: /^true$/, row: "run.yaml: asks: true -> calls.ask: true" },
  // a test
  { key: /^(kind|target|caller|metadata)$/, in: [TEST], parent: /^$/, row: "tests: kind, target -> package; caller -> host; metadata -> (none)" },
  { key: /^(http_exchanges|mcp_calls|process_calls|provider_responses)$/, in: [TEST], row: "tests: caller.http_exchanges, mcp_calls, process_calls, provider_responses -> providers.http, mcp, processes, hosted" },
  { key: /^(operator_journeys|confusors)$/, in: [TEST, RUN], row: "tests: operator_journeys -> evals/evals.json, confusors -> eval_queries.json" },
  { key: "holds", in: [TEST], parent: /(^|\.)hosted$/, row: "tests: providers.hosted.holds -> leases" },
  // connector.yaml
  { key: "runtime_capability", in: [CONNECTOR], row: "connector.yaml: runtime_capability -> (none)" },
  { key: "verb", in: [CONNECTOR], value: /^(create|comment|merge|update|review)$/, row: "connector.yaml: verb create, comment, merge, update, review -> write" },
  { key: /^(id|hosted|pays)$/, in: [CONNECTOR], parent: /^$/, row: "connector.yaml: id -> name, hosted.provider_scopes -> provider_scopes, pays -> payment" },
  { key: "holds", in: [CONNECTOR], row: "connector.yaml: holds -> lease" },
  { key: /^(param|in|cursor)$/, in: [CONNECTOR], parent: /(^|\.)pages$/, row: "connector.yaml: pages.param, in, cursor -> pages.next" },
  { key: "kind", in: [CONNECTOR], parent: /(^|\.)credentials\.[^.]+$/, value: /^purchase$/, row: "connector.yaml: auth.credentials.<name>.kind: purchase -> lease" },
  { key: "runtime_scopes", in: [CONNECTOR], row: "connector.yaml: process.runtime_scopes -> requires" },
  // a tool manifest
  { key: /^(source|scopes|idempotency|artifacts)$/, in: [MANIFEST], parent: /^$/, row: "manifest.json: source -> process; scopes, idempotency -> (none); artifacts -> result_as and the outputs' packet" },
  { key: "type", in: [MANIFEST], parent: /^source$/, row: "manifest.json: source.type -> (none)" },
  { key: "schema", in: [MANIFEST], parent: /^inputs\.[^.]+$/, row: "manifest.json: an input's schema: -> the input itself" },
  // the keys those files pass
  { key: "data_source_uri", in: ANY, row: "store: data_source_uri + resource -> stream" },
  { key: "agent_bounds", in: ANY, row: "authority: agent_bounds -> effects, reach" },
  { key: /^(verbs|bounds)$/, in: ANY, parent: /(^|\.)delegation$/, row: "authority: delegation { verbs, bounds } -> delegation { terms }" },
  { key: "goal_id", in: ANY, parent: /(^|\.)scope$/, row: "context: scope.goal_id -> (none)" },
  { key: /^(repo|page)$/, in: ANY, parent: /(^|\.)inputs\.activity$/, row: "github: activity.repo, activity.page -> activity.repositories" },
];

/**
 * The removed keys a document names wherever it names them, outside its code
 * fences: no other sense shares their spelling. A connector's `result_fields`
 * keeps its key, so it isn't one, and a signed purchase's or listing's
 * `input_schema_digest`, `output_schema_digest` and
 * `accepted_settlement_families` are only stale in an offer.
 */
export const MENTIONED = new Set([
  "input_definitions", "connector_from", "allowed_connectors", "artifact_input", "acts_on", "part_of", "canonical_skill",
  "named_emits", "wrap_as", "context_skills", "operation_name", "optional_result_fields", "expected_result",
  "ephemeral_result_paths", "artifact_from", "path_from", "path_scope_from", "input_mode", "readback_of",
  "http_exchanges", "mcp_calls", "process_calls", "provider_responses", "operator_journeys", "runtime_capability",
  "runtime_scopes", "data_source_uri", "agent_bounds",
]);

/**
 * What keeps a removed key where it's written: a parser test that writes one
 * to prove it's refused.
 */
export const KEPT = [
  { file: /^crates\/runx-parser\/src\/graph\/validate\.rs$/, key: "effects", line: /effects: free\\n/, why: "the parser's test that effects: free is refused" },
  { file: /^crates\/runx-parser\/tests\/parser_skill_package\.rs$/, key: "skill", line: /\\nskill: other\\n/, why: "the parser's test that a root skill is refused" },
  { file: /^crates\/runx-parser\/tests\/parser_turn_limits\.rs$/, key: "operations", line: /\\noperations: \[\]\\n/, why: "the parser's test that root operations are refused" },
];

const matches = (pattern, text) => (typeof pattern === "string" ? pattern === text : pattern.test(text));

/** Which document a file or a fence is, by what it says. */
export function documentKind(file, keys) {
  const base = file.split("/").pop();
  if (base === "run.yaml") return RUN;
  if (base === "connector.yaml") return CONNECTOR;
  if (base === "manifest.json") return MANIFEST;
  if (/(^|\/)tests\/[^/]+\.ya?ml$/.test(file)) return TEST;
  // A fence, or a document a test writes: what its root keys say it is.
  const roots = new Set(keys.filter((entry) => entry.path === "").map((entry) => entry.key));
  if (roots.has("schema") && keys.some((entry) => entry.path === "" && entry.key === "schema" && /runx\.tool\.manifest/.test(entry.value))) return MANIFEST;
  if (roots.has("methods") || roots.has("runners") || roots.has("minimum_runx") || roots.has("marketplace")) return RUN;
  if (roots.has("operations") && (roots.has("auth") || roots.has("process") || roots.has("http") || roots.has("hosted"))) return CONNECTOR;
  if ((roots.has("expect") || roots.has("caller") || roots.has("host")) && (roots.has("inputs") || roots.has("method") || roots.has("runner"))) return TEST;
  return undefined;
}

/**
 * The block and flow keys of YAML text, each with its offset in the text, its
 * scalar value and its parent path. A block scalar's lines are its value, not
 * keys.
 */
export function yamlKeys(text) {
  const keys = [];
  const stack = [];
  let offset = 0;
  let scalarIndent;
  for (const raw of text.split("\n")) {
    const line = raw.replace(/\r$/, "");
    const indent = /^\s*/.exec(line)[0].length;
    const blank = /^\s*(#.*)?$/.test(line);
    if (scalarIndent !== undefined && (blank || indent > scalarIndent)) {
      offset += raw.length + 1;
      continue;
    }
    scalarIndent = undefined;
    const entry = /^(\s*)((?:-\s+)*)(?:"([^"]+)"|'([^']+)'|([A-Za-z_$][\w$.-]*))\s*:(?=\s|$)(.*)$/.exec(line);
    if (entry) {
      const [, lead, dashes, double, single, bare, rest] = entry;
      while (stack.length && stack.at(-1).indent >= lead.length) stack.pop();
      let at = lead.length;
      for (const dash of dashes.match(/-\s+/g) ?? []) {
        stack.push({ indent: at, key: "-" });
        at += dash.length;
      }
      const key = double ?? single ?? bare;
      const value = rest.replace(/\s+#.*$/, "").trim();
      const parent = stack.map((item) => item.key).join(".");
      keys.push({ at: offset + at, key, value, path: parent });
      if (/^[|>][+-]?\d*$/.test(value)) scalarIndent = at;
      if (value.startsWith("{") || value.startsWith("[")) {
        keys.push(...flowKeys(value, offset + line.length - rest.length + rest.indexOf(value), parent === "" ? key : `${parent}.${key}`));
      }
      stack.push({ indent: at, key });
    } else if (!blank && /^\s*-(\s|$)/.test(line)) {
      while (stack.length && stack.at(-1).indent >= indent) stack.pop();
      const item = /^(\s*-\s+)(.*)$/.exec(line);
      if (item && (item[2].startsWith("{") || item[2].startsWith("["))) {
        const parent = [...stack.map((entry) => entry.key), "-"].join(".");
        keys.push(...flowKeys(item[2], offset + item[1].length, parent));
      }
    }
    offset += raw.length + 1;
  }
  return keys;
}

/**
 * The keys of a flow mapping or sequence: each bracket opens under the key
 * before it, or under the sequence it's an item of.
 */
function flowKeys(text, start, path) {
  const keys = [];
  const open = [];
  let next = path;
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (char === "{" || char === "[") {
      open.push(char === "[" ? `${next}.-` : next);
      next = open.at(-1);
    } else if (char === "}" || char === "]") {
      open.pop();
      next = open.at(-1) ?? path;
    } else if (char === ",") {
      next = open.at(-1) ?? path;
    } else if (/[A-Za-z_$"']/.test(char) && /[{,]\s*$/.test(text.slice(0, index))) {
      const key = /^(?:"([^"]+)"|'([^']+)'|([A-Za-z_$][\w$.-]*))\s*:(?=\s|$|[{["'])/.exec(text.slice(index));
      if (key) {
        const name = key[1] ?? key[2] ?? key[3];
        const parent = open.at(-1) ?? path;
        const value = /^\s*([^,{}[\]]*)/.exec(text.slice(index + key[0].length))[1].trim();
        keys.push({ at: start + index, key: name, value: value.replace(/^["']|["']$/g, ""), path: parent });
        next = `${parent}.${name}`;
        index += key[0].length - 1;
      }
    } else if (char === '"' || char === "'") {
      const end = text.indexOf(char, index + 1);
      index = end < 0 ? text.length : end;
    }
  }
  return keys;
}

/** The keys of a JSON document, each with its offset, scalar value and parent path. */
export function jsonKeys(text) {
  let document;
  try {
    document = JSON.parse(text);
  } catch {
    return undefined;
  }
  const keys = [];
  let from = 0;
  const walk = (value, parent) => {
    if (Array.isArray(value)) {
      for (const item of value) walk(item, parent === "" ? "-" : `${parent}.-`);
    } else if (value && typeof value === "object") {
      for (const [key, child] of Object.entries(value)) {
        const at = text.indexOf(JSON.stringify(key), from);
        if (at >= 0) from = at + 1;
        keys.push({ at: Math.max(at, 0), key, value: child === null || typeof child !== "object" ? String(child) : "", path: parent });
        walk(child, parent === "" ? key : `${parent}.${key}`);
      }
    }
  };
  walk(document, "");
  return keys;
}

/** The removed keys written in one document, each with its row. */
export function removedIn(kind, keys) {
  if (kind === undefined) return [];
  return keys.flatMap((entry) => {
    const removed = REMOVED_KEYS.find(
      (rule) =>
        rule.in.includes(kind) &&
        matches(rule.key, entry.key) &&
        (rule.parent === undefined || rule.parent.test(entry.path)) &&
        (rule.value === undefined || rule.value.test(entry.value)),
    );
    return removed ? [{ ...entry, row: removed.row }] : [];
  });
}

// A YAML document written in a code string: its escaped newlines read as lines.
const ESCAPES = /\\n/g;

/**
 * The removed keys a file writes, but those a parser test writes to prove
 * they're refused: in its own document, in its Markdown fences and mentions,
 * and in the documents its code strings write.
 */
export function removedKeys(file, text, readers) {
  const lineAt = (at) => text.slice(text.lastIndexOf("\n", at - 1) + 1, text.indexOf("\n", at) < 0 ? text.length : text.indexOf("\n", at));
  return writtenKeys(file, text, readers).filter((entry) => !KEPT.some((kept) => kept.file.test(file) && kept.key === entry.key && kept.line.test(lineAt(entry.at))));
}

// A Markdown fence keeps its offsets in the file; a code string's are where it starts.
function writtenKeys(file, text, { fences, strings }) {
  if (/\.json$/.test(file)) {
    const keys = jsonKeys(text) ?? [];
    return removedIn(documentKind(file, keys), keys);
  }
  if (/\.ya?ml$/.test(file)) {
    const keys = yamlKeys(text);
    return removedIn(documentKind(file, keys), keys);
  }
  const found = [];
  if (file.endsWith(".md")) {
    const fenced = fences(text);
    for (const mention of text.matchAll(/[A-Za-z_][A-Za-z0-9_]*/g)) {
      if (!MENTIONED.has(mention[0]) || fenced.some(([from, to]) => mention.index >= from && mention.index < to)) continue;
      found.push({ at: mention.index, key: mention[0], value: "", path: "", row: `a mention of ${mention[0]}, which the format removes` });
    }
    for (const [from, to] of fenced) {
      const body = text.slice(from, to);
      const keys = jsonKeys(body) ?? yamlKeys(body);
      for (const entry of removedIn(documentKind("", keys), keys)) found.push({ ...entry, at: from + entry.at });
    }
  } else {
    for (const [from, to] of strings(text, file)) {
      const body = text.slice(from, to);
      if (!/\\n|\n/.test(body) || !/:/.test(body)) continue;
      const keys = yamlKeys(body.replace(ESCAPES, "\n"));
      // An escaped newline is two characters in the code, one in the document.
      for (const entry of removedIn(documentKind("", keys), keys)) found.push({ ...entry, at: from + offsetInCode(body, entry.at) });
    }
  }
  return found;
}

function offsetInCode(body, at) {
  let document = 0;
  for (let index = 0; index < body.length; index += 1) {
    if (document >= at) return index;
    document += 1;
    if (body[index] === "\\" && body[index + 1] === "n") index += 1;
  }
  return body.length;
}
