#!/usr/bin/env node
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const releaseTopology = JSON.parse(
  readFileSync(
    path.join(workspaceRoot, "packages", "cli", "native", "supported-platforms.json"),
    "utf8",
  ),
);
if (releaseTopology.schema !== "runx.rust_cli_selector_topology.v1") {
  fail("release platform topology has an unsupported schema");
}

const decisionPath = process.argv[2];
if (!decisionPath || process.argv.length !== 3) {
  fail("usage: check-deterministic-module-engine-decision.mjs <decision.json>");
}

let document;
try {
  document = JSON.parse(readFileSync(path.resolve(decisionPath), "utf8"));
} catch (error) {
  fail(`cannot read engine decision: ${error instanceof Error ? error.message : String(error)}`);
}

const findings = [];
const requiredTargets = Object.keys(releaseTopology.nativePackages).sort();
const requiredCandidateClasses = new Set([
  "memory_safe_no_host_engine",
  "native_quickjs_worker",
  "no_wasi_javascript_on_wasm",
]);
const workerManifest = readFileSync(
  path.join(workspaceRoot, "crates", "runx-js-worker", "Cargo.toml"),
  "utf8",
);
const boaVersion = workerManifest.match(/^boa_engine\s*=\s*\{\s*version\s*=\s*"=([^"]+)"/mu)?.[1];

exactKeys(document, [
  "schema",
  "captured_at",
  "requirements",
  "probe",
  "candidates",
  "decision",
  "decision_digest",
], "document");
expect(document.schema === "runx.deterministic_module_engine_decision.v1", "unexpected decision schema");
expectRfc3339(document.captured_at, "captured_at");
checkRequirements(document.requirements);
checkProbe(document.probe, document.candidates);
checkCandidates(document.candidates);
checkDecision(document.decision, document.candidates);
expect(Boolean(boaVersion), "runx-js-worker must exactly pin boa_engine");
expect(
  document.decision?.selected_engine !== "boa_native"
    || document.decision?.selected_version === `boa_engine ${boaVersion}`,
  "decision.selected_version must match runx-js-worker's exact boa_engine pin",
);
checkDigest(document);

if (findings.length > 0) {
  process.stderr.write("Deterministic module engine decision check failed:\n");
  for (const finding of findings) {
    process.stderr.write(`- ${finding}\n`);
  }
  process.exit(1);
}

process.stdout.write(
  `Deterministic module engine decision passed: ${document.decision.selected_engine}@${document.decision.selected_version}.\n`,
);

function checkRequirements(value) {
  expectObject(value, "requirements");
  exactKeys(value, ["release_targets", "host_authority", "limits", "worker"], "requirements");
  expectSameStrings(value?.release_targets, requiredTargets, "requirements.release_targets");
  expectSameStrings(value?.host_authority, [
    "no_child_process",
    "no_credentials",
    "no_environment",
    "no_filesystem",
    "no_host_clock",
    "no_host_randomness",
    "no_network",
  ], "requirements.host_authority");
  expectObject(value?.limits, "requirements.limits");
  for (const [field, expected] of Object.entries({
    source_bytes: 4_194_304,
    input_bytes: 4_194_304,
    output_bytes: 4_194_304,
    heap_bytes: 67_108_864,
    stack_bytes: 4_194_304,
    wall_milliseconds: 2_000,
    queued_jobs: 4_096,
  })) {
    expect(value?.limits?.[field] === expected, `requirements.limits.${field} must equal ${expected}`);
  }
  expectObject(value?.worker, "requirements.worker");
  expect(value?.worker?.protocol === "length_delimited_json_v1", "requirements.worker.protocol must be length_delimited_json_v1");
  expect(value?.worker?.session_reuse === true, "requirements.worker.session_reuse must be true");
  expect(value?.worker?.fresh_context_per_invocation === true, "requirements.worker.fresh_context_per_invocation must be true");
  expect(value?.worker?.node_fallback === false, "requirements.worker.node_fallback must be false");
}

function checkProbe(value, candidates) {
  expectObject(value, "probe");
  exactKeys(value, ["host", "source_digests", "commands", "measurements"], "probe");
  expectObject(value?.host, "probe.host");
  expectNonEmpty(value?.host?.os, "probe.host.os");
  expectNonEmpty(value?.host?.cpu, "probe.host.cpu");
  expectObject(value?.source_digests, "probe.source_digests");
  for (const [name, digest] of Object.entries(value?.source_digests ?? {})) {
    expect(/^sha256:[a-f0-9]{64}$/u.test(digest), `probe.source_digests.${name} must be sha256-bound`);
  }
  expect(Object.keys(value?.source_digests ?? {}).length >= 4, "probe.source_digests must bind the probe sources and lockfile");
  expect(Array.isArray(value?.commands) && value.commands.length >= 5, "probe.commands must record representative commands");
  expectObject(value?.measurements, "probe.measurements");
  for (const candidate of Array.isArray(candidates) ? candidates : []) {
    expectObject(value?.measurements?.[candidate?.id], `probe.measurements.${candidate?.id}`);
  }
}

function checkCandidates(value) {
  expect(Array.isArray(value) && value.length >= 3, "candidates must contain at least three compared engine classes");
  if (!Array.isArray(value)) return;
  const ids = new Set(value.map((candidate) => candidate?.id));
  const classes = new Set(value.map((candidate) => candidate?.class));
  expect(ids.size === value.length, "candidate ids must be unique");
  for (const candidateClass of requiredCandidateClasses) {
    expect(classes.has(candidateClass), `candidate class ${candidateClass} is missing`);
  }
  for (const candidate of value) {
    const field = `candidates.${candidate?.id ?? "unknown"}`;
    expectObject(candidate, field);
    exactKeys(candidate, [
      "id",
      "class",
      "version",
      "license",
      "official_sources",
      "security_boundary",
      "module_model",
      "resource_controls",
      "platform_evidence",
      "local_probe",
      "verdict",
      "reason",
    ], field);
    for (const name of ["id", "class", "version", "license", "security_boundary", "module_model", "verdict", "reason"]) {
      expectNonEmpty(candidate?.[name], `${field}.${name}`);
    }
    expect(Array.isArray(candidate?.official_sources) && candidate.official_sources.length >= 2, `${field}.official_sources must cite at least two primary sources`);
    for (const source of candidate?.official_sources ?? []) {
      expect(/^https:\/\//u.test(source), `${field}.official_sources must use HTTPS URLs`);
    }
    expect(Array.isArray(candidate?.resource_controls) && candidate.resource_controls.length >= 2, `${field}.resource_controls must be substantive`);
    expectObject(candidate?.platform_evidence, `${field}.platform_evidence`);
    expectSameStrings(Object.keys(candidate?.platform_evidence ?? {}), requiredTargets, `${field}.platform_evidence keys`);
    for (const target of requiredTargets) {
      expectNonEmpty(candidate?.platform_evidence?.[target]?.status, `${field}.platform_evidence.${target}.status`);
      expectNonEmpty(candidate?.platform_evidence?.[target]?.evidence, `${field}.platform_evidence.${target}.evidence`);
    }
    expectObject(candidate?.local_probe, `${field}.local_probe`);
  }
}

function checkDecision(value, candidates) {
  expectObject(value, "decision");
  exactKeys(value, [
    "selected_engine",
    "selected_version",
    "worker_design",
    "rationale",
    "required_controls",
    "release_gate",
    "rejected_alternatives",
  ], "decision");
  const selected = Array.isArray(candidates)
    ? candidates.find((candidate) => candidate?.id === value?.selected_engine)
    : undefined;
  expect(Boolean(selected), "decision.selected_engine must name a compared candidate");
  expect(value?.selected_version === selected?.version, "decision.selected_version must match the selected candidate");
  expect(selected?.verdict === "selected", "the selected candidate's verdict must be selected");
  expectNonEmpty(value?.worker_design, "decision.worker_design");
  expect(Array.isArray(value?.rationale) && value.rationale.length >= 4, "decision.rationale must explain the tradeoff");
  expectSameStrings(value?.required_controls, [
    "aggregate_process_memory_limit",
    "bounded_job_queue",
    "env_clear",
    "fresh_engine_context_per_invocation",
    "hostile_module_contract_on_all_release_targets",
    "in_memory_relative_module_loader",
    "no_node_fallback",
    "non_workspace_cwd",
    "protocol_version_handshake",
    "supervised_wall_timeout",
    "worker_replacement_after_fault",
  ], "decision.required_controls");
  expectNonEmpty(value?.release_gate, "decision.release_gate");
  expect(
    Array.isArray(value?.rejected_alternatives)
      && value.rejected_alternatives.length === Math.max((candidates?.length ?? 0) - 1, 0),
    "decision.rejected_alternatives must name every rejected candidate",
  );
  const rejected = new Set((value?.rejected_alternatives ?? []).map((entry) => entry?.id));
  for (const candidate of candidates ?? []) {
    if (candidate?.id !== value?.selected_engine) {
      expect(rejected.has(candidate?.id), `decision.rejected_alternatives is missing ${candidate?.id}`);
    }
  }
  for (const target of requiredTargets) {
    expect(selected?.platform_evidence?.[target]?.status === "passed", `selected engine must pass ${target}`);
  }
  for (const control of ["host_access", "termination", "fresh_instance_isolation", "in_memory_imports"]) {
    expect(selected?.local_probe?.[control] === "passed", `selected engine local probe ${control} must pass`);
  }
}

function checkDigest(value) {
  expect(/^sha256:[a-f0-9]{64}$/u.test(value?.decision_digest ?? ""), "decision_digest must be sha256-prefixed");
  const unsigned = { ...value };
  delete unsigned.decision_digest;
  const actual = `sha256:${createHash("sha256").update(canonicalJson(unsigned)).digest("hex")}`;
  expect(value?.decision_digest === actual, `decision_digest mismatch: expected ${actual}`);
}

function canonicalJson(value) {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
}

function exactKeys(value, expected, field) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return;
  expectSameStrings(Object.keys(value), expected, `${field} keys`);
}

function expectSameStrings(actual, expected, field) {
  const left = Array.isArray(actual) ? [...actual].sort() : [];
  const right = [...expected].sort();
  expect(JSON.stringify(left) === JSON.stringify(right), `${field} must equal ${JSON.stringify(right)}`);
}

function expectObject(value, field) {
  expect(Boolean(value) && typeof value === "object" && !Array.isArray(value), `${field} must be an object`);
}

function expectNonEmpty(value, field) {
  expect(typeof value === "string" && value.trim().length > 0, `${field} must be a non-empty string`);
}

function expectRfc3339(value, field) {
  expect(typeof value === "string" && !Number.isNaN(Date.parse(value)), `${field} must be RFC3339`);
}

function expect(condition, message) {
  if (!condition) findings.push(message);
}

function fail(message) {
  process.stderr.write(`${message}\n`);
  process.exit(1);
}
