import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import {
  escapeRegExp,
  isArchitectureCheckFile,
  productionRustSource,
  relative,
  rustFiles,
  skillProductionFiles,
  walk,
  workspaceRoot,
} from "./context.mjs";

export function checkNormativeArchitectureContract(findings) {
  const architecturePath = path.join(workspaceRoot, "docs/architecture/runx-system.md");
  if (!existsSync(architecturePath)) {
    findings.push("docs/architecture/runx-system.md is missing");
    return;
  }
  const source = readFileSync(architecturePath, "utf8");
  for (const heading of [
    "## Repository ownership",
    "## Skill knowledge contract",
    "## Execution lanes",
    "## Deterministic module boundary",
    "## Native capability boundary",
    "## Effect and finality boundary",
    "## Authoring and extension boundary",
    "## Cloud boundary",
    "## Performance contract",
    "## Replacement rule",
  ]) {
    if (!source.includes(heading)) {
      findings.push(`docs/architecture/runx-system.md lacks normative section ${heading}`);
    }
  }
}

export function checkCrateDependencyDirection(findings) {
  const forbidden = new Map([
    ["runx-contracts", ["runx-core", "runx-parser", "runx-receipts", "runx-runtime", "runx-cli"]],
    ["runx-core", ["runx-parser", "runx-receipts", "runx-runtime", "runx-cli"]],
    ["runx-parser", ["runx-receipts", "runx-runtime", "runx-cli"]],
    ["runx-receipts", ["runx-core", "runx-parser", "runx-runtime", "runx-cli"]],
    ["runx-runtime", ["runx-cli"]],
  ]);
  for (const [crateName, dependencies] of forbidden) {
    const manifestPath = path.join(workspaceRoot, "crates", crateName, "Cargo.toml");
    if (!existsSync(manifestPath)) {
      findings.push(`missing crate manifest ${relative(manifestPath)}`);
      continue;
    }
    const source = readFileSync(manifestPath, "utf8");
    for (const dependency of dependencies) {
      if (new RegExp(`^${escapeRegExp(dependency)}\\s*=`, "mu").test(source)) {
        findings.push(`${relative(manifestPath)} violates dependency direction with ${dependency}`);
      }
    }
  }
}

export function checkCanonicalToolManifestOwnership(findings) {
  const manifestPaths = [];
  const toolRoot = path.join(workspaceRoot, "tools");
  for (const filePath of existsSync(toolRoot) ? walk(toolRoot) : []) {
    if (path.basename(filePath) === "manifest.json") manifestPaths.push(filePath);
  }
  const skillRoot = path.join(workspaceRoot, "skills");
  for (const filePath of existsSync(skillRoot) ? walk(skillRoot) : []) {
    const parts = path.relative(skillRoot, filePath).split(path.sep);
    if (path.basename(filePath) === "manifest.json" && parts.includes("tools")) {
      manifestPaths.push(filePath);
    }
  }

  for (const manifestPath of manifestPaths) {
    let manifest;
    try {
      manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
    } catch {
      findings.push(`${relative(manifestPath)} is not valid JSON`);
      continue;
    }
    if (manifest.schema !== "runx.tool.manifest.v1") {
      findings.push(`${relative(manifestPath)} must declare schema runx.tool.manifest.v1`);
    }
    for (const field of [
      "output",
      "runx",
      "runtime",
      "schema_digest",
      "source_digest",
      "toolkit_version",
    ]) {
      if (Object.hasOwn(manifest, field)) {
        findings.push(`${relative(manifestPath)} duplicates derived runtime ownership through ${field}`);
      }
    }
  }
}

export function checkCloudOwnershipBoundary(findings) {
  const roots = ["crates", "packages", "skills", "src"];
  const extensions = new Set([".rs", ".ts", ".tsx", ".js", ".mjs", ".cjs"]);
  const cloudReference = /(?:\.\.\/)+cloud(?:\/|\b)|\brunx\/cloud\b|\/cloud\//u;
  for (const root of roots) {
    const absoluteRoot = path.join(workspaceRoot, root);
    if (!existsSync(absoluteRoot)) {
      continue;
    }
    for (const filePath of walk(absoluteRoot)) {
      if (!extensions.has(path.extname(filePath))) {
        continue;
      }
      if (cloudReference.test(readFileSync(filePath, "utf8"))) {
        findings.push(`${relative(filePath)} reaches into the Cloud tree from an OSS production surface`);
      }
    }
  }
}

export function checkManagedAgentDefault(findings) {
  const orchestratorPath = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/execution/orchestrator.rs",
  );
  const source = existsSync(orchestratorPath) ? readFileSync(orchestratorPath, "utf8") : "";
  if (!/#\[derive\([^\]]*Default[^\]]*\)\][\s\S]{0,300}enum\s+ManagedAgentPolicy[\s\S]{0,180}#\[default\]\s*HostDriven/u.test(source)) {
    findings.push(`${relative(orchestratorPath)} must default managed-agent execution to HostDriven`);
  }
  const cliParserPath = path.join(workspaceRoot, "crates/runx-cli/src/skill/parser.rs");
  const cliSource = existsSync(cliParserPath) ? readFileSync(cliParserPath, "utf8") : "";
  const managedAgentPath = path.join(workspaceRoot, "crates/runx-cli/src/managed_agent.rs");
  const managedAgentSource = existsSync(managedAgentPath)
    ? readFileSync(managedAgentPath, "utf8")
    : "";
  const skillUsesSharedPolicy = /managed_agent_policy\(\s*"skill"/u.test(cliSource);
  const sharedPolicyRequiresConsent = /if\s+!enabled\s*\{[\s\S]{0,180}max_rounds\.is_some\(\)[\s\S]{0,220}--managed-agent-rounds requires --managed-agent/u.test(
    managedAgentSource,
  );
  if (!skillUsesSharedPolicy || !sharedPolicyRequiresConsent) {
    findings.push("managed-agent policy must reject a round budget without explicit consent through the shared CLI policy");
  }
}

export function checkTypedCapabilityPlane(findings) {
  const capabilityPath = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/capability.rs",
  );
  const capability = existsSync(capabilityPath)
    ? readFileSync(capabilityPath, "utf8")
    : "";
  for (const token of [
    "trait CapabilityContract",
    "fn input_schema",
    "fn output_schema",
    "fn normalize_inputs",
    "fn validate_output",
  ]) {
    if (!capability.includes(token)) {
      findings.push(`${relative(capabilityPath)} lacks typed capability contract token ${token}`);
    }
  }
}

export function checkDeterministicWorkerOwnership(findings) {
  const workerManifestPath = path.join(workspaceRoot, "crates/runx-js-worker/Cargo.toml");
  const workerManifest = existsSync(workerManifestPath)
    ? readFileSync(workerManifestPath, "utf8")
    : "";
  if (!workerManifest.includes("publish = false")) {
    findings.push(`${relative(workerManifestPath)} must remain a private shipping binary crate`);
  }
  for (const forbidden of ["runx-runtime", "runx-cli"]) {
    if (new RegExp(`^${forbidden}\\s*=`, "mu").test(workerManifest)) {
      findings.push(`${relative(workerManifestPath)} must not depend on authority-bearing crate ${forbidden}`);
    }
  }

  const runtimeManifestPath = path.join(workspaceRoot, "crates/runx-runtime/Cargo.toml");
  const runtimeManifest = existsSync(runtimeManifestPath)
    ? readFileSync(runtimeManifestPath, "utf8")
    : "";
  for (const forbidden of ["runx-js-worker", "boa_engine"]) {
    if (new RegExp(`^${forbidden}\\s*=`, "mu").test(runtimeManifest)) {
      findings.push(`${relative(runtimeManifestPath)} must keep deterministic JavaScript out of process (${forbidden})`);
    }
  }

  const ciPath = path.join(workspaceRoot, ".github/workflows/ci.yml");
  const ci = existsSync(ciPath) ? readFileSync(ciPath, "utf8") : "";
  if (!ci.includes("cargo build -p runx-js-worker")) {
    findings.push(`${relative(ciPath)} must build the worker from its owning crate`);
  }

  const globalsPath = path.join(
    workspaceRoot,
    "crates/runx-js-worker/src/engine/globals.rs",
  );
  const globals = existsSync(globalsPath) ? readFileSync(globalsPath, "utf8") : "";
  for (const token of ["FunctionObjectBuilder", "register_global_property", "Math.random"]) {
    if (!globals.includes(token)) {
      findings.push(`${relative(globalsPath)} lacks native deterministic-global token ${token}`);
    }
  }
  if (/\b(?:context\.)?eval\s*\(|\bSource::/u.test(globals)) {
    findings.push(`${relative(globalsPath)} must not parse a JavaScript bootstrap per invocation`);
  }
}

export function checkNoRuntimeCompatModules(findings) {
  for (const filePath of rustFiles("crates/runx-runtime/src")) {
    const source = readFileSync(filePath, "utf8");
    const rel = relative(filePath);
    if (/\bmod\s+\w+_(?:legacy|compat)\b/u.test(source)) {
      findings.push(`${rel} declares a legacy/compat runtime module`);
    }
    if (/\b(?:LegacyExecutor|CompatExecutor)\b/u.test(source)) {
      findings.push(`${rel} declares legacy executor compatibility vocabulary`);
    }
  }
}
