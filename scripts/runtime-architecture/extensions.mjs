import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import { relative, walk, workspaceRoot } from "./context.mjs";

export function checkAdapterOwnership(findings) {
  for (const relPath of [
    "examples/adapter-kit/adapter.mjs",
    "scripts/lib/payment-finality-adapter.mjs",
    "scripts/x402-finality-adapter.mjs",
    "scripts/x402-finality-adapter.manifest.json",
    "scripts/stripe-spt-finality-adapter.mjs",
    "scripts/stripe-spt-finality-adapter.manifest.json",
    "scripts/mpp-tempo-finality-adapter.mjs",
    "scripts/mpp-tempo-finality-adapter.manifest.json",
    "scripts/mpp-fiat-finality-adapter.mjs",
    "scripts/mpp-fiat-finality-adapter.manifest.json",
    "tests/payment-finality-adapters.test.ts",
  ]) {
    if (existsSync(path.join(workspaceRoot, relPath))) {
      findings.push(`${relPath} duplicates a canonical runtime owner`);
    }
  }

  const langChainBridgePath = path.join(workspaceRoot, "packages/langchain/src/index.ts");
  const langChainBridge = existsSync(langChainBridgePath)
    ? readFileSync(langChainBridgePath, "utf8")
    : "";
  if (/\b(?:createLangChainToolCatalogAdapter|LangChainToolCatalogAdapterOptions)\b/u.test(langChainBridge)) {
    findings.push(`${relative(langChainBridgePath)} retains a nonfunctional catalog-adapter compatibility API`);
  }

  const parityContractPath = path.join(workspaceRoot, "tests/cli-feature-parity-contract.ts");
  const parityContract = existsSync(parityContractPath)
    ? readFileSync(parityContractPath, "utf8")
    : "";
  if (/adapter-catalog|runx-runtime catalog adapter/u.test(parityContract)) {
    findings.push(`${relative(parityContractPath)} retains the displaced catalog adapter as a parity surface`);
  }
}

export function checkAuthoringOwnership(findings) {
  const forbiddenPaths = [
    "packages/authoring",
    "fixtures/scaffold",
    "crates/runx-runtime/src/scaffold.rs",
    "crates/runx-runtime/src/scaffold",
    "crates/runx-cli/src/scaffold.rs",
    "scripts/generate-rust-scaffold-fixtures.ts",
    "scripts/materialize-upstream-skill-binding.mjs",
    "scripts/lib/skill-operator-value.mjs",
    "scripts/audit-skill-operator-value.mjs",
    "scripts/trial-core-skills.mjs",
    "scripts/check-skill-capabilities.mjs",
  ];
  for (const relPath of forbiddenPaths) {
    if (existsSync(path.join(workspaceRoot, relPath))) {
      findings.push(`${relPath} is a retired parallel authoring surface`);
    }
  }

  const projectPath = path.join(workspaceRoot, "crates/runx-cli/src/project.rs");
  const projectSource = existsSync(projectPath) ? readFileSync(projectPath, "utf8") : "";
  for (const token of ["PathBuf::from(\"skill-lab\")", "runner: Some(\"build\".to_owned())"] ) {
    if (!projectSource.includes(token)) {
      findings.push(`${relative(projectPath)} must delegate runx new to skill-lab build`);
    }
  }
}

export function checkContractBindingOwnership(findings) {
  const bindings = [
    ["packages/contracts/src/schemas/registry.ts", [
      "registry-binding.schema.json",
      "review-receipt-output.schema.json",
    ]],
    ["packages/contracts/src/schemas/operational-policy.ts", [
      "operational-policy.schema.json",
    ]],
  ];
  for (const [relPath, artifacts] of bindings) {
    const filePath = path.join(workspaceRoot, relPath);
    const source = existsSync(filePath) ? readFileSync(filePath, "utf8") : "";
    for (const artifact of artifacts) {
      if (!source.includes("generatedSchema") || !source.includes(`"${artifact}"`)) {
        findings.push(`${relPath} must consume Rust-owned generated schema ${artifact}`);
      }
    }
    if (/\bType\.(?:Object|Union|Record)\s*\(/u.test(source)) {
      findings.push(`${relPath} reconstructs a Rust-owned wire schema in TypeScript`);
    }
  }
}

export function checkGeneratedMirrorOwnership(findings) {
  for (const relPath of [
    "packages/cli/dist",
    "packages/cli/skills",
    "packages/cli/tools",
    "scripts/registry-publish-summary.ts",
    "scripts/generate-runtime-catalog-adapter-oracles.ts",
    "scripts/generate-agent-adapter-fixtures.ts",
    "scripts/runtime-adapter-oracle-checks.ts",
    "scripts/check-runtime-catalog-adapter-oracles.sh",
    "scripts/check-tool-catalog-oracles.sh",
    "dist/packets/spec.normalized-scafld-spec.v1.schema.json",
    "dist/packets/spec.declared-file-context.v1.schema.json",
    "examples/host-protocol/openai.ts",
    "crates/runx-contracts/src/cli.rs",
    "crates/runx-contracts/src/receipts.rs",
    "crates/runx-contracts/src/registry.rs",
    "scripts/payment-bridge-spike.mjs",
    "scripts/settlement-finality.mjs",
    "scripts/check-cli-package-contract.mjs",
    "scripts/check-deterministic-module-platform-evidence.mjs",
    "scripts/check-orchestrator-directory-listings.mjs",
    "scripts/check-runtime-cutover-legacy.mjs",
    "scripts/publish-public-package.mjs",
    "scripts/public-package-utils.mjs",
    "docs/runtime-cutover-inventory.json",
    "docs/core-skill-review-decisions.json",
    "docs/core-skill-trial-results.json",
    "docs/core-skill-provider-trials.json",
    "fixtures/runtime/adapters/agent",
  ]) {
    if (existsSync(path.join(workspaceRoot, relPath))) {
      findings.push(`${relPath} is stale generated or mirrored state without a shipping owner`);
    }
  }
  const releaseWorkflowPath = path.join(workspaceRoot, ".github/workflows/release.yml");
  const releaseWorkflow = existsSync(releaseWorkflowPath)
    ? readFileSync(releaseWorkflowPath, "utf8")
    : "";
  if (releaseWorkflow.includes(".scafld/")) {
    findings.push(`${relative(releaseWorkflowPath)} depends on ignored scafld execution state`);
  }
  const skillRoot = path.join(workspaceRoot, "skills");
  for (const filePath of existsSync(skillRoot) ? walk(skillRoot) : []) {
    if (filePath.split(path.sep).includes(".runx")) {
      findings.push(`${relative(filePath)} is generated local runtime state inside a skill package`);
    }
  }

  const coreReviewPath = path.join(workspaceRoot, "docs/core-skill-review.md");
  const coreReview = existsSync(coreReviewPath) ? readFileSync(coreReviewPath, "utf8") : "";
  for (const retired of ["tool:spec.normalize_scafld_frontmatter", "tool:spec.read_declared_files"]) {
    if (coreReview.includes(retired)) {
      findings.push(`${relative(coreReviewPath)} advertises retired capability ${retired}`);
    }
  }
}
