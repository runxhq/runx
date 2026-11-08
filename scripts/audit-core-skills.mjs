#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { repositorySkills } from "./lib/skills.mjs";
import { bundledCoreSkills } from "./classify-ci-changes.mjs";

const schema = "runx.core_skill_operator_value_audit.v1";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const allowedArchetypes = new Set([
  "artifact",
  "builder",
  "context",
  "operation",
  "runtime",
  "workflow",
]);
const allowedDecisions = new Set([
  "improve",
  "internal_fixture",
  "internal_runtime",
  "keep",
]);
const generatedComposesMarker =
  "<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->";
let nativeProcessCount = 0;

try {
  const options = parseOptions(process.argv.slice(2));
  if (options.selfTest) {
    runSelfTests();
    process.stdout.write("core skill audit self-test passed\n");
    process.exit(0);
  }

  const report = buildReport(options);
  if (options.json) {
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  } else {
    process.stdout.write(
      `core skill audit ${report.status}: ${report.summary.reviewed}/${report.summary.runx_maintained} reviewed\n`,
    );
  }
  if (report.status !== "passed") process.exitCode = 1;
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
}

function buildReport(options) {
  nativeProcessCount = 0;
  const runx = resolveRunxBinary(options.runxBin);
  const maintained = repositorySkills(root, { ...process.env, RUNX_RUST_CLI_BIN: runx }, options.skills);
  const review = readProductReview(options.skills);
  const catalog = readNativeCatalog(runx, options.skills);
  const unknown = [...(options.skills ?? [])].filter(
    (name) => !maintained.some((record) => record.name === name),
  );
  if (unknown.length > 0) throw new Error(`--skills names skills that are not runx-maintained: ${unknown.join(", ")}`);
  const audited = options.skills
    ? maintained.filter((record) => options.skills.has(record.name))
    : maintained;
  const findings = validateCoverage({ maintained: audited, review, catalog, selected: options.skills });
  const entries = [];
  let rewrittenManualCount = 0;

  for (const record of audited) {
    const { name } = record;
    const decision = review.get(name);
    const catalogItem = catalog.get(name);
    const inspection = inspectSkill(runx, name);
    const methodInspections = Array.isArray(inspection.method_inspections)
      ? inspection.method_inspections
      : [];
    if (inspection.status !== "ok") {
      findings.push(`${name}: native inspection returned ${inspection.status ?? "no status"}`);
    }
    if (inspection.name !== name) {
      findings.push(`${name}: native inspection returned name ${inspection.name ?? "<missing>"}`);
    }
    findings.push(
      ...validateInspectionClaims({
        name,
        record,
        decision,
        inspection,
        methodInspections,
      }),
    );
    const manualPath = path.join(root, "skills", name, "SKILL.md");
    let manualSource = readFileSync(manualPath, "utf8");
    if (options.writeComposes && record.visibility === "public") {
      const rewritten = rewriteComposesSection(
        manualSource,
        expectedCompositionRefs(methodInspections),
      );
      if (rewritten !== manualSource) {
        writeFileSync(manualPath, rewritten);
        manualSource = rewritten;
        rewrittenManualCount += 1;
      }
    }
    findings.push(
      ...validatePublicManual({
        name,
        record,
        methodInspections,
        source: manualSource,
      }),
    );
    entries.push({
      skill: name,
      visibility: record.visibility,
      claimed_visibility: decision?.visibility ?? null,
      native_visibility: inspection.catalog?.visibility ?? null,
      claimed_execution: decision?.execution ?? null,
      execution_closure: inspection.execution_closure ?? null,
      archetype: decision?.archetype ?? null,
      decision: decision?.decision ?? null,
      rationale: decision?.rationale ?? null,
      improvement: decision?.improvement === "none" ? null : decision?.improvement ?? null,
      evidence: decision?.evidence ?? null,
      native: {
        fixtures: catalogItem?.fixtures ?? 0,
        steps: catalogItem?.steps ?? 0,
        method: inspection.method ?? null,
        catalog: inspection.catalog ?? null,
        package_digest: inspection.package_digest ?? null,
        manual_digest: inspection.manual_digest ?? null,
        methods: methodInspections.map((methodInspection) => ({
          method: methodInspection.method?.name ?? null,
          execution_closure: methodInspection.execution_closure ?? null,
        })),
      },
    });
  }

  const maximumNativeProcesses = audited.length + 1;
  if (nativeProcessCount > maximumNativeProcesses) {
    findings.push(
      `native audit launched ${nativeProcessCount} processes; expected at most ${maximumNativeProcesses}`,
    );
  }
  entries.sort((left, right) => left.skill.localeCompare(right.skill));
  const publicCount = audited.filter((entry) => entry.visibility === "public").length;
  const internalCount = audited.length - publicCount;
  const decisions = countBy(entries, (entry) => entry.decision);
  const archetypes = countBy(entries, (entry) => entry.archetype);
  return {
    schema,
    status: findings.length === 0 ? "passed" : "failed",
    summary: {
      runx_maintained: audited.length,
      audited: audited.length,
      reviewed: options.skills ? [...review.keys()].filter((name) => options.skills.has(name)).length : review.size,
      public: publicCount,
      internal: internalCount,
      native_processes: nativeProcessCount,
      rewritten_manuals: rewrittenManualCount,
      decisions,
      archetypes,
    },
    findings: findings.sort(),
    entries,
  };
}

function readProductReview(selected) {
  const source = readFileSync(path.join(root, "docs", "core-skill-review.md"), "utf8");
  return parseReviewTable(source, selected);
}

function parseReviewTable(source, selected) {
  const header =
    "| Skill | Archetype | Visibility | Default execution shape | Evidence | Decision | Rationale | Improvement |";
  const lines = source.split(/\r?\n/u);
  const headerIndex = lines.indexOf(header);
  if (headerIndex < 0) throw new Error("docs/core-skill-review.md is missing its package review table");

  const records = new Map();
  for (const line of lines.slice(headerIndex + 2)) {
    if (!line.startsWith("|")) break;
    if (selected && !selected.has(/^\|\s*([^|]+?)\s*\|/u.exec(line)?.[1]?.trim())) continue;
    const cells = markdownCells(line);
    if (cells.length !== 8) {
      throw new Error(`invalid core skill review row with ${cells.length} cells: ${line}`);
    }
    const [skill, archetype, visibility, execution, evidence, decision, rationale, improvement] =
      cells;
    if (!skill) throw new Error("core skill review contains an empty skill name");
    if (records.has(skill)) throw new Error(`core skill review contains duplicate skill ${skill}`);
    records.set(skill, {
      archetype,
      visibility,
      execution,
      evidence,
      decision,
      rationale,
      improvement,
    });
  }
  return records;
}

function markdownCells(line) {
  const cells = [];
  let current = "";
  let escaped = false;
  for (const character of line.slice(1, -1)) {
    if (escaped) {
      current += character;
      escaped = false;
    } else if (character === "\\") {
      escaped = true;
    } else if (character === "|") {
      cells.push(current.trim());
      current = "";
    } else {
      current += character;
    }
  }
  cells.push(current.trim());
  return cells;
}

function readNativeCatalog(runx, selected) {
  const output = runJson(runx, ["list", "skills", "--ok-only", "--json"]);
  if (output.schema !== "runx.list.v1" || !Array.isArray(output.items)) {
    throw new Error("native skill catalog returned an unsupported envelope");
  }
  const topLevel = new Map();
  for (const item of output.items) {
    if (typeof item?.path !== "string" || typeof item?.name !== "string") continue;
    if (selected && !selected.has(item.name)) continue;
    if (item.path !== `skills/${item.name}/run.yaml`) continue;
    if (topLevel.has(item.name)) throw new Error(`native catalog returned duplicate ${item.name}`);
    topLevel.set(item.name, item);
  }
  return topLevel;
}

function inspectSkill(runx, name) {
  const args = ["skill", "inspect", `skills/${name}`];
  args.push("--json");
  return runJson(runx, args);
}

function validateCoverage({ maintained, review, catalog, selected }) {
  const findings = [];
  const runxMaintainedNames = new Set(maintained.map((record) => record.name));
  for (const { name, visibility } of maintained) {
    if (!["internal", "public"].includes(visibility)) {
      findings.push(`${name}: unsupported visibility ${visibility}`);
    }
    const claimed = review.get(name)?.visibility;
    if (claimed !== undefined && claimed !== visibility) {
      findings.push(`${name}: review visibility ${claimed} does not match its run.yaml ${visibility}`);
    }
  }

  for (const [name, decision] of review) {
    if (selected && !selected.has(name)) continue;
    if (!runxMaintainedNames.has(name)) findings.push(`${name}: review row names no skill in skills/`);
    if (!allowedArchetypes.has(decision.archetype)) {
      findings.push(`${name}: unsupported archetype ${decision.archetype}`);
    }
    if (!allowedDecisions.has(decision.decision)) {
      findings.push(`${name}: unsupported decision ${decision.decision}`);
    }
    if (!decision.rationale) findings.push(`${name}: review rationale is empty`);
    if (!decision.evidence) findings.push(`${name}: review evidence is empty`);
  }

  for (const name of runxMaintainedNames) {
    if (!review.has(name)) findings.push(`${name}: missing product review row`);
    if (!catalog.has(name)) findings.push(`${name}: missing top-level native catalog entry`);
  }
  for (const name of catalog.keys()) {
    if (selected && !selected.has(name)) continue;
    if (!runxMaintainedNames.has(name)) findings.push(`${name}: top-level native catalog entry has no skill in skills/`);
  }
  return findings;
}

function validateInspectionClaims({
  name,
  record,
  decision,
  inspection,
  methodInspections,
}) {
  const findings = [];
  findings.push(...validatePublicInvocationExample(name, record, inspection));
  const closure = inspection.execution_closure;
  if (!hasCanonicalExecutionClosure(closure)) {
    findings.push(`${name}: native inspection omitted the canonical execution closure`);
  } else if (decision?.execution !== closure.summary) {
    findings.push(
      `${name}: review execution ${decision?.execution ?? "<missing>"} does not match native closure ${closure.summary}`,
    );
  }
  if (!Array.isArray(inspection.methods)) {
    findings.push(`${name}: native inspection omitted its method catalog`);
    return findings;
  }
  if (methodInspections.length !== inspection.methods.length) {
    findings.push(`${name}: native inspection did not inspect every declared method`);
  }
  for (const [index, methodInspection] of methodInspections.entries()) {
    const expectedMethod = inspection.methods[index];
    if (
      methodInspection.method?.name !== expectedMethod
    ) {
      findings.push(`${name}#${expectedMethod}: native method inspection identity mismatch`);
    }
    if (!hasCanonicalExecutionClosure(methodInspection.execution_closure)) {
      findings.push(`${name}#${expectedMethod}: native inspection omitted the execution closure`);
    }
  }
  return findings;
}

/**
 * A public skill's default method, the one inspection selects when none is
 * named, carries an invocation example an agent can copy as it is.
 */
function validatePublicInvocationExample(name, record, inspection) {
  if (record.visibility !== "public") return [];
  const defaultMethod = inspection.method?.name;
  const examples = inspection.method?.input_schema?.examples;
  if (!Array.isArray(examples) || examples.length === 0) {
    return [`${name}#${defaultMethod ?? "<missing>"}: agent default has no copy-valid invocation example`];
  }
  if (Buffer.byteLength(JSON.stringify(examples[0]), "utf8") > 4096) {
    return [`${name}#${defaultMethod}: agent default invocation example exceeds 4096 bytes`];
  }
  return [];
}

function hasCanonicalExecutionClosure(closure) {
  return typeof closure?.summary === "string"
    && Array.isArray(closure?.components)
    && Array.isArray(closure?.skill_edges)
    && Array.isArray(closure?.direct_external_skill_edges)
    && Array.isArray(closure?.profiles)
    && Number.isInteger(closure?.agent_acts)
    && typeof closure?.declared_artifact === "boolean";
}

function validatePublicManual({
  name,
  record,
  methodInspections,
  source,
}) {
  if (record.visibility !== "public") return [];
  const findings = [];
  const body = source.replace(/^---[\s\S]*?---\s*/u, "").trim();
  // Each agent task is a `## Agent task: <name>` section; the guide is what
  // comes before the first.
  const firstTask = /^##\s+Agent task:\s*\S.*$/mu.exec(body);
  const guide = firstTask ? body.slice(0, firstTask.index).trim() : body;
  const guideSections = guide.match(/^##\s+\S.*$/gmu) ?? [];
  const hasExplanation = guide.split(/\r?\n/u).some((line) => {
    const trimmed = line.trim();
    return trimmed.length > 0
      && !trimmed.startsWith("#")
      && !trimmed.startsWith("```")
      && !/^[-*]\s*$/u.test(trimmed);
  });
  if (!/^#\s+\S.*$/mu.test(guide) || guideSections.length === 0 || !hasExplanation) {
    findings.push(
      `${name}: public manual needs an operator guide before its agent tasks`,
    );
  }

  for (const edge of methodInspections.flatMap(
    (inspection) => inspection.execution_closure?.direct_external_skill_edges ?? [],
  )) {
    if (typeof edge?.skill !== "string" || edge.skill.length === 0 || !edgeMethod(edge)) {
      findings.push(`${name}: native direct external skill edge is malformed`);
    }
  }
  const expectedCompositions = expectedCompositionRefs(methodInspections);
  const declaredCompositions = manualCompositionRefs(guide);
  if (expectedCompositions.size > 0 && !guide.includes(generatedComposesMarker)) {
    findings.push(`${name}: Composes section is not generator-owned`);
  }
  for (const expected of expectedCompositions) {
    if (!declaredCompositions.has(expected)) {
      findings.push(`${name}: Composes section omits native edge ${expected}`);
    }
  }
  for (const declared of declaredCompositions) {
    if (!expectedCompositions.has(declared)) {
      findings.push(`${name}: Composes section declares stale edge ${declared}`);
    }
  }
  return findings;
}

function expectedCompositionRefs(methodInspections) {
  const refs = new Set();
  for (const edge of methodInspections.flatMap(
    (inspection) => inspection.execution_closure?.direct_external_skill_edges ?? [],
  )) {
    const method = edgeMethod(edge);
    if (typeof edge?.skill === "string" && edge.skill.length > 0 && method) {
      refs.add(`${edge.skill}#${method}`);
    }
  }
  return new Set([...refs].sort());
}

/** The method a closure's skill edge names; its wire key is still `runner`. */
function edgeMethod(edge) {
  const method = edge?.runner;
  return typeof method === "string" && method.length > 0 ? method : undefined;
}

function manualCompositionRefs(source) {
  const heading = /^##\s+Composes\s*$/imu.exec(source);
  if (!heading) return new Set();
  const bodyStart = heading.index + heading[0].length;
  const remainder = source.slice(bodyStart);
  const nextHeading = /^##\s+\S.*$/mu.exec(remainder);
  const section = nextHeading ? remainder.slice(0, nextHeading.index) : remainder;
  return new Set(
    [...section.matchAll(/^\s*-\s+`([^`\s]+#[^`\s]+)`\s*$/gmu)]
      .map((match) => match[1]),
  );
}

function rewriteComposesSection(source, refs) {
  const trailingNewline = source.endsWith("\n");
  const lines = source.replace(/\n$/u, "").split("\n");
  const start = lines.findIndex((line) => /^##\s+Composes\s*$/u.test(line));
  const end = start < 0
    ? -1
    : lines.findIndex((line, index) => index > start && /^##\s+\S.*$/u.test(line));
  const sectionEnd = end < 0 ? lines.length : end;
  const block = refs.size === 0
    ? []
    : [
        "## Composes",
        "",
        generatedComposesMarker,
        "",
        ...[...refs].map((ref) => `- \`${ref}\``),
        "",
      ];

  if (start >= 0) {
    lines.splice(start, sectionEnd - start, ...block);
  } else if (block.length > 0) {
    const firstSection = lines.findIndex((line) => /^##\s+\S.*$/u.test(line));
    const insertion = firstSection < 0 ? lines.length : firstSection;
    lines.splice(insertion, 0, ...block);
  }

  while (lines.length > 1 && lines.at(-1) === "" && lines.at(-2) === "") {
    lines.pop();
  }
  const rewritten = lines.join("\n");
  return trailingNewline ? `${rewritten}\n` : rewritten;
}

function countBy(entries, selector) {
  const counts = {};
  for (const entry of entries) {
    const key = selector(entry) ?? "missing";
    counts[key] = (counts[key] ?? 0) + 1;
  }
  return Object.fromEntries(Object.entries(counts).sort(([left], [right]) => left.localeCompare(right)));
}

function resolveRunxBinary(explicit) {
  const candidate = explicit
    ?? process.env.RUNX_RUST_CLI_BIN
    ?? path.join(root, "crates", "target", "debug", process.platform === "win32" ? "runx.exe" : "runx");
  const resolved = path.resolve(root, candidate);
  if (!existsSync(resolved)) {
    throw new Error(`native Runx CLI is required; build runx-cli or set RUNX_RUST_CLI_BIN (${resolved})`);
  }
  return resolved;
}

function runJson(runx, args) {
  nativeProcessCount += 1;
  const result = spawnSync(runx, args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
    env: { ...process.env, INIT_CWD: root, PWD: root, NO_COLOR: "1" },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${path.basename(runx)} ${args.join(" ")} failed: ${result.stderr.trim()}`);
  }
  try {
    return JSON.parse(result.stdout);
  } catch {
    throw new Error(`${path.basename(runx)} ${args.join(" ")} returned invalid JSON`);
  }
}

function parseOptions(args) {
  const options = {
    json: false,
    selfTest: false,
    runxBin: undefined,
    skills: undefined,
    coreOnly: false,
    writeComposes: false,
  };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--check") continue;
    if (argument === "--json") options.json = true;
    else if (argument === "--self-test") options.selfTest = true;
    else if (argument === "--write-composes") options.writeComposes = true;
    else if (argument === "--core-only") options.coreOnly = true;
    else if (argument === "--skills") {
      const value = args[index + 1];
      if (!value) throw new Error("--skills requires a comma-separated list");
      options.skills = new Set(value.split(",").map((name) => name.trim()).filter(Boolean));
      index += 1;
    }
    else if (argument === "--runx-bin") {
      const value = args[index + 1];
      if (!value) throw new Error("--runx-bin requires a path");
      options.runxBin = value;
      index += 1;
    }
    else throw new Error(`unknown option: ${argument}`);
  }
  if (options.coreOnly && options.skills) throw new Error("--core-only and --skills cannot be combined");
  if (options.coreOnly) options.skills = new Set(bundledCoreSkills());
  return options;
}

function runSelfTests() {
  const table = [
    "# Review",
    "",
    "| Skill | Archetype | Visibility | Default execution shape | Evidence | Decision | Rationale | Improvement |",
    "|---|---|---|---|---|---|---|---|",
    "| alpha | workflow | public | workflow | tests passed | keep | Useful \\| governed. | none |",
    "",
  ].join("\n");
  const review = parseReviewTable(table);
  assert(review.size === 1, "review parser must return one row");
  assert(review.get("alpha")?.rationale === "Useful | governed.", "escaped pipes must survive");
  const maintained = [{ name: "alpha", visibility: "public" }];
  const catalog = new Map([["alpha", { name: "alpha" }]]);
  assert(
    validateCoverage({ maintained, review, catalog }).length === 0,
    "matching native, run.yaml and review evidence must pass",
  );
  const missing = validateCoverage({ maintained, review: new Map(), catalog });
  assert(missing.includes("alpha: missing product review row"), "missing review rows must fail");
  const wrongVisibility = new Map([
    ["alpha", { ...review.get("alpha"), visibility: "internal" }],
  ]);
  assert(
    validateCoverage({ maintained, review: wrongVisibility, catalog }).some((finding) =>
      finding.includes("review visibility internal does not match its run.yaml public")
    ),
    "visibility drift must fail",
  );
  const inspection = {
    status: "ok",
    name: "alpha",
    methods: ["default"],
    method: { name: "default", input_schema: { examples: [{ objective: "fixture" }] } },
    catalog: { visibility: "public" },
    execution_closure: {
      summary: "tool:data.read",
      components: ["tool:data.read"],
      skill_edges: [],
      direct_external_skill_edges: [],
      profiles: ["run.yaml#default"],
      agent_acts: 0,
      declared_artifact: false,
    },
  };
  const methodInspection = { method: inspection.method, execution_closure: inspection.execution_closure };
  assert(
    validateInspectionClaims({
      name: "alpha",
      record: maintained[0],
      decision: { execution: "javascript" },
      inspection,
      methodInspections: [methodInspection],
    }).some((finding) => finding.includes("does not match native closure")),
    "execution-closure drift must fail",
  );
  assert(
    validateInspectionClaims({
      name: "alpha",
      record: maintained[0],
      decision: { execution: "tool:data.read" },
      inspection,
      methodInspections: [methodInspection],
    }).length === 0,
    "matching catalog and execution claims must pass",
  );
  assert(
    validateInspectionClaims({
      name: "alpha",
      record: maintained[0],
      decision: { execution: "tool:data.read" },
      inspection: { ...inspection, methods: ["default", "other"] },
      methodInspections: [methodInspection],
    }).some((finding) => finding.includes("did not inspect every declared method")),
    "an uninspected method must fail",
  );
  assert(
    validatePublicInvocationExample("alpha", maintained[0], {
      ...inspection,
      method: { name: "default", input_schema: {} },
    }).some((finding) => finding.includes("no copy-valid invocation example")),
    "a public default with no example must fail the core-skill audit",
  );
  const validGuide = [
    "---",
    "name: alpha",
    "description: Test a manual.",
    "---",
    "",
    "# Alpha",
    "",
    "## Operating model",
    "",
    "Compose `research` evidence before making the decision.",
    "",
    "## Agent task: alpha-decide",
    "",
    "Return the bounded decision.",
  ].join("\n");
  assert(
    validatePublicManual({
      name: "alpha",
      record: maintained[0],
      methodInspections: [methodInspection],
      source: validGuide,
    }).length === 0,
    "a real guide before its agent tasks must pass",
  );
  const invalidGuide = validGuide.replace("## Operating model\n\nCompose `research` evidence before making the decision.\n\n", "");
  const guideFindings = validatePublicManual({
    name: "alpha",
    record: maintained[0],
    methodInspections: [methodInspection],
    source: invalidGuide,
  });
  assert(
    guideFindings.some((finding) => finding.includes("operator guide")),
    "agent tasks must not substitute for an operator guide",
  );
  const composingInspection = {
    ...methodInspection,
    execution_closure: {
      ...inspection.execution_closure,
      direct_external_skill_edges: [{ skill: "research", runner: "research" }],
    },
  };
  const generatedGuide = rewriteComposesSection(
    validGuide,
    new Set(["research#research"]),
  );
  assert(
    generatedGuide.includes(generatedComposesMarker)
      && manualCompositionRefs(generatedGuide).has("research#research"),
    "the generator must own the exact native composition section",
  );
  assert(
    validatePublicManual({
      name: "alpha",
      record: maintained[0],
      methodInspections: [composingInspection],
      source: generatedGuide,
    }).length === 0,
    "a generated exact composition section must pass",
  );
  assert(
    validatePublicManual({
      name: "alpha",
      record: maintained[0],
      methodInspections: [composingInspection],
      source: validGuide,
    }).some((finding) => finding.includes("omits native edge research#research")),
    "a composition the manual omits must fail",
  );
  let missingRunxPathRejected = false;
  try {
    parseOptions(["--runx-bin"]);
  } catch {
    missingRunxPathRejected = true;
  }
  assert(missingRunxPathRejected, "--runx-bin without a path must fail");
}

function assert(condition, message) {
  if (!condition) throw new Error(`self-test failed: ${message}`);
}
