#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { validateRunnerManifestYaml } from "./lib/native-parser.mjs";

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
  const maintained = readRunxMaintainedLock();
  const review = readProductReview();
  const catalog = readNativeCatalog(runx);
  const toolCatalog = readNativeToolCatalog(runx);
  const findings = validateCoverage({ maintained, review, catalog });
  // CI audits the skills a change proves; with none named, every one.
  const unknown = [...(options.skills ?? [])].filter(
    (name) => !maintained.some((record) => runxMaintainedName(record) === name),
  );
  if (unknown.length > 0) throw new Error(`--skills names skills that are not runx-maintained: ${unknown.join(", ")}`);
  const audited = options.skills
    ? maintained.filter((record) => options.skills.has(runxMaintainedName(record)))
    : maintained;
  const entries = [];
  let rewrittenManualCount = 0;
  let semanticDiagnosticCount = 0;

  for (const record of audited) {
    const name = runxMaintainedName(record);
    const decision = review.get(name);
    const catalogItem = catalog.get(name);
    const inspection = inspectSkill(runx, name);
    const runnerInspections = Array.isArray(inspection.runner_inspections)
      ? inspection.runner_inspections
      : [];
    if (inspection.status !== "ok") {
      findings.push(`${name}: native inspection returned ${inspection.status ?? "no status"}`);
    }
    if (inspection.name !== name) {
      findings.push(`${name}: native inspection returned name ${inspection.name ?? "<missing>"}`);
    }
    findings.push(...validateSemanticReport(name, inspection.semantic_report));
    findings.push(...validateAgentToolScopes(name, toolCatalog));
    semanticDiagnosticCount += Array.isArray(inspection.semantic_report?.diagnostics)
      ? inspection.semantic_report.diagnostics.length
      : 0;
    findings.push(
      ...validateInspectionClaims({
        name,
        record,
        decision,
        inspection,
        runnerInspections,
      }),
    );
    const manualPath = path.join(root, "skills", name, "SKILL.md");
    let manualSource = readFileSync(manualPath, "utf8");
    if (options.writeComposes && record.catalog_visibility === "public") {
      const rewritten = rewriteComposesSection(
        manualSource,
        expectedCompositionRefs(runnerInspections),
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
        runnerInspections,
        source: manualSource,
      }),
    );
    const canonicalCatalogRole =
      `${inspection.catalog?.visibility ?? "<missing>"}/${inspection.catalog?.role ?? "<missing>"}`;
    entries.push({
      skill: name,
      visibility: record.catalog_visibility,
      role: record.catalog_role ?? null,
      claimed_catalog_role: decision?.catalog_role ?? null,
      canonical_catalog_role: canonicalCatalogRole,
      claimed_execution: decision?.execution ?? null,
      execution_closure: inspection.execution_closure ?? null,
      archetype: decision?.archetype ?? null,
      decision: decision?.decision ?? null,
      rationale: decision?.rationale ?? null,
      improvement: decision?.improvement === "none" ? null : decision?.improvement ?? null,
      evidence: decision?.evidence ?? null,
      native: {
        kind: catalogItem?.kind ?? null,
        fixtures: catalogItem?.fixtures ?? 0,
        harness_cases: catalogItem?.harness_cases ?? 0,
        runner: inspection.runner ?? null,
        capabilities: inspection.capabilities ?? null,
        catalog: inspection.catalog ?? null,
        package_digest: inspection.package_digest ?? null,
        manual_digest: inspection.manual_digest ?? null,
        semantic_report: inspection.semantic_report ?? null,
        operator_journeys: inspection.operator_journeys ?? null,
        runners: runnerInspections.map((runnerInspection) => ({
          runner: runnerInspection.runner?.name ?? null,
          execution_closure: runnerInspection.execution_closure ?? null,
        })),
      },
    });
  }

  const maximumNativeProcesses = audited.length + 2;
  if (nativeProcessCount > maximumNativeProcesses) {
    findings.push(
      `native audit launched ${nativeProcessCount} processes; expected at most ${maximumNativeProcesses}`,
    );
  }
  entries.sort((left, right) => left.skill.localeCompare(right.skill));
  const publicCount = maintained.filter((entry) => entry.catalog_visibility === "public").length;
  const internalCount = maintained.length - publicCount;
  const decisions = countBy(entries, (entry) => entry.decision);
  const archetypes = countBy(entries, (entry) => entry.archetype);
  return {
    schema,
    status: findings.length === 0 ? "passed" : "failed",
    summary: {
      runx_maintained: maintained.length,
      audited: audited.length,
      reviewed: review.size,
      public: publicCount,
      internal: internalCount,
      native_processes: nativeProcessCount,
      rewritten_manuals: rewrittenManualCount,
      semantic_diagnostics: semanticDiagnosticCount,
      decisions,
      archetypes,
    },
    findings: findings.sort(),
    entries,
  };
}

function readRunxMaintainedLock() {
  const value = JSON.parse(readFileSync(path.join(root, "skills", "runx-maintained.lock.json"), "utf8"));
  if (!Array.isArray(value)) throw new Error("skills/runx-maintained.lock.json must be an array");
  return [...value].sort((left, right) => runxMaintainedName(left).localeCompare(runxMaintainedName(right)));
}

function runxMaintainedName(record) {
  if (typeof record?.skill_id !== "string") {
    throw new Error(`invalid runx-maintained skill record: ${JSON.stringify(record)}`);
  }
  const parts = record.skill_id.split("/");
  if (parts.length !== 2 || !parts[0] || !parts[1]) {
    throw new Error(`invalid runx-maintained skill record: ${JSON.stringify(record)}`);
  }
  return parts[1];
}

function readProductReview() {
  const source = readFileSync(path.join(root, "docs", "core-skill-review.md"), "utf8");
  return parseReviewTable(source);
}

function parseReviewTable(source) {
  const header =
    "| Skill | Archetype | Catalog role | Default execution shape | Evidence | Decision | Rationale | Improvement |";
  const lines = source.split(/\r?\n/u);
  const headerIndex = lines.indexOf(header);
  if (headerIndex < 0) throw new Error("docs/core-skill-review.md is missing its package review table");

  const records = new Map();
  for (const line of lines.slice(headerIndex + 2)) {
    if (!line.startsWith("|")) break;
    const cells = markdownCells(line);
    if (cells.length !== 8) {
      throw new Error(`invalid core skill review row with ${cells.length} cells: ${line}`);
    }
    const [skill, archetype, catalogRole, execution, evidence, decision, rationale, improvement] =
      cells;
    if (!skill) throw new Error("core skill review contains an empty skill name");
    if (records.has(skill)) throw new Error(`core skill review contains duplicate skill ${skill}`);
    records.set(skill, {
      archetype,
      catalog_role: catalogRole,
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

function readNativeCatalog(runx) {
  const output = runJson(runx, ["list", "skills", "--ok-only", "--json"]);
  if (output.schema !== "runx.list.v1" || !Array.isArray(output.items)) {
    throw new Error("native skill catalog returned an unsupported envelope");
  }
  const topLevel = new Map();
  for (const item of output.items) {
    if (typeof item?.path !== "string" || typeof item?.name !== "string") continue;
    if (item.path !== `skills/${item.name}/run.yaml`) continue;
    if (topLevel.has(item.name)) throw new Error(`native catalog returned duplicate ${item.name}`);
    topLevel.set(item.name, item);
  }
  return topLevel;
}

function readNativeToolCatalog(runx) {
  const output = runJson(runx, ["list", "tools", "--ok-only", "--json"]);
  if (output.schema !== "runx.list.v1" || !Array.isArray(output.items)) {
    throw new Error("native tool catalog returned an unsupported envelope");
  }
  return new Map(output.items.flatMap((item) => {
    if (typeof item?.name !== "string" || !Array.isArray(item.scopes)) return [];
    return [[item.name, new Set(item.scopes)]];
  }));
}

function validateAgentToolScopes(name, toolCatalog) {
  const source = readFileSync(path.join(root, "skills", name, "run.yaml"), "utf8");
  const profile = validateRunnerManifestYaml(source);
  const findings = [];
  for (const [runnerName, runner] of Object.entries(profile.runners ?? {})) {
    for (const step of runner.graph?.steps ?? []) {
      findings.push(...missingAgentToolScopes(
        `${name}#${runnerName}.${step.id ?? "<unknown-step>"}`,
        step.allowedTools,
        step.scopes,
        toolCatalog,
      ));
    }
  }
  return findings;
}

function missingAgentToolScopes(label, allowedTools, declaredScopes, toolCatalog) {
  if (!Array.isArray(allowedTools) || allowedTools.length === 0) return [];
  const declared = new Set(Array.isArray(declaredScopes) ? declaredScopes : []);
  const missing = new Set();
  for (const tool of allowedTools) {
    for (const scope of toolCatalog.get(tool) ?? []) {
      if (!declared.has(scope)) missing.add(scope);
    }
  }
  return missing.size === 0
    ? []
    : [`${label}: allowed agent tools require undeclared scopes ${[...missing].sort().join(", ")}`];
}

function inspectSkill(runx, name) {
  const args = ["skill", "inspect", `skills/${name}`];
  args.push("--json");
  return runJson(runx, args);
}

function validateCoverage({ maintained, review, catalog }) {
  const findings = [];
  const runxMaintainedNames = new Set();
  for (const record of maintained) {
    const name = runxMaintainedName(record);
    if (runxMaintainedNames.has(name)) findings.push(`${name}: duplicate runx-maintained lock entry`);
    runxMaintainedNames.add(name);
    if (!["internal", "public"].includes(record.catalog_visibility)) {
      findings.push(`${name}: unsupported visibility ${record.catalog_visibility ?? "<missing>"}`);
    }
    const claimed = review.get(name)?.catalog_role;
    const locked = `${record.catalog_visibility}/${record.catalog_role ?? "<missing>"}`;
    if (claimed !== undefined && claimed !== locked) {
      findings.push(`${name}: review catalog role ${claimed} does not match lock ${locked}`);
    }
  }

  for (const [name, decision] of review) {
    if (!runxMaintainedNames.has(name)) findings.push(`${name}: review row is not in the runx-maintained lock`);
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
    if (!runxMaintainedNames.has(name)) findings.push(`${name}: top-level native catalog entry is unlocked`);
  }
  return findings;
}

function validateInspectionClaims({
  name,
  record,
  decision,
  inspection,
  runnerInspections,
}) {
  const findings = [];
  const nativeVisibility = inspection.catalog?.visibility;
  const nativeRole = inspection.catalog?.role;
  if (nativeVisibility !== record.catalog_visibility) {
    findings.push(
      `${name}: native visibility ${nativeVisibility ?? "<missing>"} does not match lock ${record.catalog_visibility}`,
    );
  }
  if (nativeRole !== record.catalog_role) {
    findings.push(
      `${name}: native role ${nativeRole ?? "<missing>"} does not match lock ${record.catalog_role ?? "<missing>"}`,
    );
  }
  findings.push(...validateOperatorJourneys(name, record, inspection));
  findings.push(...validatePublicInvocationReadiness(name, record, inspection, runnerInspections));
  const closure = inspection.execution_closure;
  if (!hasCanonicalExecutionClosure(closure)) {
    findings.push(`${name}: native inspection omitted the canonical execution closure`);
  } else if (decision?.execution !== closure.summary) {
    findings.push(
      `${name}: review execution ${decision?.execution ?? "<missing>"} does not match native closure ${closure.summary}`,
    );
  }
  if (!Array.isArray(inspection.runners)) {
    findings.push(`${name}: native inspection omitted its runner catalog`);
    return findings;
  }
  if (runnerInspections.length !== inspection.runners.length) {
    findings.push(`${name}: native inspection did not inspect every declared runner`);
  }
  for (const [index, runnerInspection] of runnerInspections.entries()) {
    const expectedRunner = inspection.runners[index];
    if (
      runnerInspection.runner?.name !== expectedRunner
    ) {
      findings.push(`${name}#${expectedRunner}: native runner inspection identity mismatch`);
    }
    if (!hasCanonicalExecutionClosure(runnerInspection.execution_closure)) {
      findings.push(`${name}#${expectedRunner}: native inspection omitted the execution closure`);
    }
  }
  return findings;
}

function validatePublicInvocationReadiness(name, record, inspection, runnerInspections) {
  if (record.catalog_visibility !== "public") return [];
  const findings = [];
  const readiness = inspection.semantic_report?.readiness;
  if (readiness?.evaluated !== true) {
    findings.push(`${name}: public skill has no evaluated native readiness report`);
  }
  if (readiness?.coldSelection !== true || readiness?.standaloneDefault !== true) {
    findings.push(`${name}: public skill is not verified for direct agent invocation`);
  }
  if (readiness?.composedReuse !== true) {
    findings.push(`${name}: public skill is not verified for composed Runx invocation`);
  }
  const defaultRunner = inspection.semantic_report?.defaultRunner;
  const defaultInspection = runnerInspections.find(
    (entry) => entry.runner?.name === defaultRunner,
  );
  const examples = defaultInspection?.runner?.input_schema?.examples;
  if (!Array.isArray(examples) || examples.length === 0) {
    findings.push(`${name}#${defaultRunner ?? "<missing>"}: agent default has no copy-valid invocation example`);
  } else if (Buffer.byteLength(JSON.stringify(examples[0]), "utf8") > 4096) {
    findings.push(`${name}#${defaultRunner}: agent default invocation example exceeds 4096 bytes`);
  }
  return findings;
}

function validateOperatorJourneys(name, record, inspection) {
  if (record.catalog_visibility !== "public") return [];
  const findings = [];
  const journeys = inspection.operator_journeys;
  if (!Array.isArray(journeys)) {
    return [`${name}: native inspection omitted operator journeys`];
  }
  const modes = new Set();
  const identities = new Set();
  for (const [index, journey] of journeys.entries()) {
    const label = `${name}: operator journey ${index}`;
    if (!journey || typeof journey !== "object") {
      findings.push(`${label} is malformed`);
      continue;
    }
    if (!["standalone", "composed", "refusal"].includes(journey.mode)) {
      findings.push(`${label} has unsupported mode ${journey.mode ?? "<missing>"}`);
    } else {
      modes.add(journey.mode);
    }
    for (const field of ["case", "request", "expected_outcome"]) {
      if (typeof journey[field] !== "string" || journey[field].trim().length === 0) {
        findings.push(`${label} has no ${field}`);
      }
    }
    const identity = `${journey.runner ?? ""}\u0000${journey.case ?? ""}\u0000${journey.mode ?? ""}`;
    if (identities.has(identity)) findings.push(`${label} duplicates an earlier journey claim`);
    identities.add(identity);
    if (journey.mode === "composed") {
      if (!nonEmptyStrings(journey.prior_evidence)) {
        findings.push(`${label} has no reusable prior evidence`);
      }
      if (!nonEmptyStrings(journey.must_not_repeat)) {
        findings.push(`${label} has no explicit non-repetition boundary`);
      }
    }
  }
  for (const required of ["standalone", "composed"]) {
    if (!modes.has(required)) findings.push(`${name}: public skill has no ${required} operator journey`);
  }
  return findings;
}

function nonEmptyStrings(value) {
  return Array.isArray(value)
    && value.length > 0
    && value.every((entry) => typeof entry === "string" && entry.trim().length > 0);
}

function validateSemanticReport(name, report) {
  const findings = [];
  if (!report || typeof report !== "object") {
    return [`${name}: native inspection omitted its catalog semantic report`];
  }
  if (report.mode !== "operator_readiness") {
    findings.push(`${name}: semantic report mode is ${report.mode ?? "<missing>"}`);
  }
  if (report.skill !== name) {
    findings.push(`${name}: semantic report names ${report.skill ?? "<missing>"}`);
  }
  if (report.defaultRunner !== undefined && typeof report.defaultRunner !== "string") {
    findings.push(`${name}: semantic report default runner is malformed`);
  }
  if (!Array.isArray(report.diagnostics)) {
    findings.push(`${name}: semantic report diagnostics are missing`);
    return findings;
  }
  for (const [index, diagnostic] of report.diagnostics.entries()) {
    const label = `${name}: semantic diagnostic ${index}`;
    if (typeof diagnostic?.code !== "string" || diagnostic.code.length === 0) {
      findings.push(`${label} has no code`);
    }
    if (diagnostic?.skill !== name) {
      findings.push(`${label} names ${diagnostic?.skill ?? "<missing>"}`);
    }
    if (typeof diagnostic?.runner !== "string" || diagnostic.runner.length === 0) {
      findings.push(`${label} has no runner`);
    }
    if (!Array.isArray(diagnostic?.observed)) {
      findings.push(`${label} has no observed execution facts`);
    }
    if (
      typeof diagnostic?.requiredCorrection !== "string"
      || diagnostic.requiredCorrection.length === 0
    ) {
      findings.push(`${label} has no required correction`);
    }
    findings.push(
      `${label} ${diagnostic?.code ?? "<missing>"} rejects runner ${diagnostic?.runner ?? "<missing>"}: ${diagnostic?.requiredCorrection ?? "<missing correction>"}`,
    );
  }
  return findings;
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
  runnerInspections,
  source,
}) {
  if (record.catalog_visibility !== "public") return [];
  const findings = [];
  const body = source.replace(/^---[\s\S]*?---\s*/u, "").trim();
  const taskContract = /^#{2,6}\s+Agent task contracts?\s*$/imu.exec(body);
  const guide = taskContract ? body.slice(0, taskContract.index).trim() : body;
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
      `${name}: public manual needs an operator guide before any agent task contracts`,
    );
  }

  for (const edge of runnerInspections.flatMap(
    (inspection) => inspection.execution_closure?.direct_external_skill_edges ?? [],
  )) {
    if (
      typeof edge?.skill !== "string"
      || edge.skill.length === 0
      || typeof edge?.runner !== "string"
      || edge.runner.length === 0
    ) {
      findings.push(`${name}: native direct external skill edge is malformed`);
    }
  }
  const expectedCompositions = expectedCompositionRefs(runnerInspections);
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

function expectedCompositionRefs(runnerInspections) {
  const refs = new Set();
  for (const edge of runnerInspections.flatMap(
    (inspection) => inspection.execution_closure?.direct_external_skill_edges ?? [],
  )) {
    if (
      typeof edge?.skill === "string"
      && edge.skill.length > 0
      && typeof edge?.runner === "string"
      && edge.runner.length > 0
    ) {
      refs.add(`${edge.skill}#${edge.runner}`);
    }
  }
  return new Set([...refs].sort());
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
    writeComposes: false,
  };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--check") continue;
    if (argument === "--json") options.json = true;
    else if (argument === "--self-test") options.selfTest = true;
    else if (argument === "--write-composes") options.writeComposes = true;
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
  return options;
}

function runSelfTests() {
  const toolCatalog = new Map([
    ["fs.read", new Set(["fs.read"])],
    ["git.status", new Set(["git.read"])],
  ]);
  assert(
    missingAgentToolScopes(
      "alpha#default.inspect",
      ["fs.read", "git.status"],
      ["fs.read"],
      toolCatalog,
    )[0]?.includes("git.read"),
    "agent tool scopes must be declared before managed execution",
  );
  assert(
    missingAgentToolScopes(
      "alpha#default.inspect",
      ["fs.read", "git.status"],
      ["fs.read", "git.read"],
      toolCatalog,
    ).length === 0,
    "complete agent tool scopes must pass",
  );
  const table = [
    "# Review",
    "",
    "| Skill | Archetype | Catalog role | Default execution shape | Evidence | Decision | Rationale | Improvement |",
    "|---|---|---|---|---|---|---|---|",
    "| alpha | workflow | public/canonical | graph | harness passed | keep | Useful \\| governed. | none |",
    "",
  ].join("\n");
  const review = parseReviewTable(table);
  assert(review.size === 1, "review parser must return one row");
  assert(review.get("alpha")?.rationale === "Useful | governed.", "escaped pipes must survive");
  const maintained = [{
    skill_id: "runx/alpha",
    catalog_visibility: "public",
    catalog_role: "canonical",
  }];
  const catalog = new Map([["alpha", { name: "alpha" }]]);
  assert(
    validateCoverage({ maintained, review, catalog }).length === 0,
    "matching native, lock, and review evidence must pass",
  );
  const missing = validateCoverage({ maintained, review: new Map(), catalog });
  assert(missing.includes("alpha: missing product review row"), "missing review rows must fail");
  const wrongRole = new Map([
    ["alpha", { ...review.get("alpha"), catalog_role: "public/branded" }],
  ]);
  assert(
    validateCoverage({ maintained, review: wrongRole, catalog }).some((finding) =>
      finding.includes("review catalog role public/branded does not match lock public/canonical")
    ),
    "catalog-role drift must fail",
  );
  const inspection = {
    status: "ok",
    name: "alpha",
    runners: ["default"],
    runner: { name: "default", input_schema: { examples: [{ objective: "fixture" }] } },
    catalog: { visibility: "public", role: "canonical" },
    execution_closure: {
      summary: "tool:data.read",
      components: ["tool:data.read"],
      skill_edges: [],
      direct_external_skill_edges: [],
      profiles: ["run.yaml#default"],
      agent_acts: 0,
      declared_artifact: false,
    },
    semantic_report: {
      mode: "operator_readiness",
      skill: "alpha",
      defaultRunner: "default",
      diagnostics: [],
      readiness: {
        evaluated: true,
        coldSelection: true,
        standaloneDefault: true,
        composedReuse: true,
        suppliedAgentAnswers: false,
        coldSelectionConfusors: ["extract", "issue-intake", "research"],
        standaloneCase: "standalone",
        composedCase: "composed",
      },
    },
    operator_journeys: [
      {
        case: "standalone",
        runner: "default",
        mode: "standalone",
        request: "Perform alpha directly.",
        expected_outcome: "Return the completed alpha result.",
        prior_evidence: [],
        must_not_repeat: [],
      },
      {
        case: "composed",
        runner: "default",
        mode: "composed",
        request: "Continue alpha from prior evidence.",
        expected_outcome: "Return the completed alpha result without repeating discovery.",
        prior_evidence: ["prior alpha evidence"],
        must_not_repeat: ["discovery"],
      },
    ],
  };
  assert(
    validateSemanticReport("alpha", inspection.semantic_report).length === 0,
    "native semantic reports must be consumed without reimplementing their analysis",
  );
  assert(
    validateSemanticReport("alpha", { ...inspection.semantic_report, skill: "beta" })
      .some((finding) => finding.includes("names beta")),
    "semantic report identity drift must fail",
  );
  assert(
    validateSemanticReport("alpha", {
      ...inspection.semantic_report,
      diagnostics: [{
        code: "default_runner_is_planning_only",
        skill: "alpha",
        runner: "default",
        claimedExecution: "execute",
        claimedCompletion: "provider_readback",
        observed: ["source:javascript"],
        requiredCorrection: "Select an executing default.",
      }],
    }).some((finding) => finding.includes("rejects runner default")),
    "semantic diagnostics must block the core-skill audit",
  );
  assert(
    validateInspectionClaims({
      name: "alpha",
      record: maintained[0],
      decision: { execution: "javascript" },
      inspection,
      runnerInspections: [inspection],
    }).some((finding) => finding.includes("does not match native closure")),
    "execution-closure drift must fail",
  );
  assert(
    validateInspectionClaims({
      name: "alpha",
      record: maintained[0],
      decision: { execution: "tool:data.read" },
      inspection,
      runnerInspections: [inspection],
    }).length === 0,
    "matching catalog and execution claims must pass",
  );
  assert(
    validatePublicInvocationReadiness(
      "alpha",
      maintained[0],
      {
        ...inspection,
        semantic_report: {
          ...inspection.semantic_report,
          readiness: {
            ...inspection.semantic_report.readiness,
            standaloneDefault: false,
          },
        },
      },
      [inspection],
    ).some((finding) => finding.includes("not verified for direct agent invocation")),
    "unverified direct invocation must fail the core-skill audit",
  );
  const missingJourney = {
    ...inspection,
    operator_journeys: inspection.operator_journeys.filter((journey) =>
      journey.mode !== "composed"
    ),
  };
  assert(
    validateOperatorJourneys("alpha", maintained[0], missingJourney)
      .some((finding) => finding.includes("no composed operator journey")),
    "public skills without a composed journey must fail",
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
    "## Agent task contracts",
    "",
    "Return the bounded decision.",
  ].join("\n");
  assert(
    validatePublicManual({
      name: "alpha",
      record: maintained[0],
      runnerInspections: [inspection],
      source: validGuide,
    }).length === 0,
    "a real guide before task contracts must pass",
  );
  const invalidGuide = validGuide.replace("## Operating model\n\nCompose `research` evidence before making the decision.\n\n", "");
  const guideFindings = validatePublicManual({
    name: "alpha",
    record: maintained[0],
    runnerInspections: [inspection],
    source: invalidGuide,
  });
  assert(
    guideFindings.some((finding) => finding.includes("operator guide")),
    "task contracts must not substitute for an operator guide",
  );
  const composingInspection = {
    ...inspection,
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
      runnerInspections: [composingInspection],
      source: generatedGuide,
    }).length === 0,
    "a generated exact composition section must pass",
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
