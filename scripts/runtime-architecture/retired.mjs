import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import {
  isArchitectureCheckFile,
  productionRustSource,
  relative,
  rustFiles,
  splitIdentifierParts,
  walk,
  workspaceRoot,
} from "./context.mjs";

export function checkRetiredRuntimeSurfaces(findings) {
  for (const relPath of [
    "crates/runx-runtime/src/adapters/catalog.rs",
    "crates/runx-runtime/src/adapters/http.rs",
    "fixtures/runtime/adapters/catalog",
    "fixtures/parser/tool-manifests/catalog-tool-json.json",
    "examples/http-tool-catalog",
    "examples/orchestrator-webhooks",
    "tools/orchestrators/n8n_handoff",
    "tools/orchestrators/zapier_handoff",
    "scripts/check-orchestrator-webhook-templates.mjs",
  ]) {
    if (existsSync(path.join(workspaceRoot, relPath))) {
      findings.push(`${relPath} is a retired parallel runtime surface`);
    }
  }
  for (const root of ["tools", "examples", "skills"]) {
    const absoluteRoot = path.join(workspaceRoot, root);
    for (const filePath of existsSync(absoluteRoot) ? walk(absoluteRoot) : []) {
      if (path.basename(filePath) !== "manifest.json") continue;
      let manifest;
      try {
        manifest = JSON.parse(readFileSync(filePath, "utf8"));
      } catch {
        continue;
      }
      if (["http", "catalog"].includes(manifest?.source?.type)) {
        findings.push(
          `${relative(filePath)} retains retired source.type ${manifest.source.type}; use a graph tool step`,
        );
      }
    }
  }
  const toolParserPath = path.join(workspaceRoot, "crates/runx-parser/src/tool.rs");
  const toolParser = existsSync(toolParserPath) ? readFileSync(toolParserPath, "utf8") : "";
  if (/normalize_tool_manifest_shape|"catalog"\s*\|/u.test(toolParser)) {
    findings.push(`${relative(toolParserPath)} retains retired tool-source normalization or admission`);
  }
  const toolContractPath = path.join(
    workspaceRoot,
    "packages/contracts/src/schemas/tool-manifest.ts",
  );
  const toolContract = existsSync(toolContractPath) ? readFileSync(toolContractPath, "utf8") : "";
  if (/ToolManifestHttpSourceContract|"catalog"|"http"|catalog_ref/u.test(toolContract)) {
    findings.push(`${relative(toolContractPath)} drifts from the generated canonical tool-source schema`);
  }
  for (const filePath of rustFiles("crates/runx-runtime/src")) {
    const source = readFileSync(filePath, "utf8");
    if (/\bCatalogAdapter\b|\badapters::catalog\b/u.test(source)) {
      findings.push(`${relative(filePath)} retains the displaced catalog adapter`);
    }
  }
  const runnerFiles = [
    ...rustFiles("crates/runx-runtime/src/execution/runner"),
    path.join(workspaceRoot, "crates/runx-runtime/src/execution/runner.rs"),
  ].filter(existsSync);
  for (const filePath of runnerFiles) {
    const source = readFileSync(filePath, "utf8");
    for (const pattern of [
      /\bpayment_supervisor\b/u,
      /\b(?:crate|runx_runtime)::payment::state\b/u,
      /\b(?:use\s+)?crate::payment::/u,
    ]) {
      if (pattern.test(source)) {
        findings.push(`${relative(filePath)} retains retired payment orchestration ${pattern}`);
      }
    }
  }

  const domainTokens = new Set(["payment", "settlement", "spend", "x402", "rail"]);
  const paidInvocationContractOwner = path.join(
    workspaceRoot,
    "crates/runx-contracts/src/paid_invocation.rs",
  );
  const paidInvocationFingerprintOwner = path.join(
    workspaceRoot,
    "crates/runx-contracts/src/paid_invocation_fingerprint.rs",
  );
  const paidSkillListingContractOwner = path.join(
    workspaceRoot,
    "crates/runx-contracts/src/paid_skill_listing.rs",
  );
  const paidInvocationFixtureProducer = path.join(
    workspaceRoot,
    "crates/runx-contracts/src/bin/runx-paid-invocation-fixtures.rs",
  );
  const x402ContractOwner = path.join(workspaceRoot, "crates/runx-contracts/src/x402.rs");
  const x402FixtureProducer = path.join(
    workspaceRoot,
    "crates/runx-contracts/src/bin/runx-x402-fixtures.rs",
  );
  const schemaArtifactsProjection = path.join(
    workspaceRoot,
    "crates/runx-contracts/src/schema_artifacts.rs",
  );
  const contractsLib = path.join(workspaceRoot, "crates/runx-contracts/src/lib.rs");
  // Generated from connectors/*/connector.yaml: manifest data, not runtime code.
  const officialConnectors = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/kernel/connectors/official.rs",
  );
  for (const root of [
    "crates/runx-runtime/src",
    "crates/runx-core/src",
    "crates/runx-contracts/src",
  ]) {
    for (const filePath of rustFiles(root)) {
      if (
        filePath.includes(`${path.sep}tests${path.sep}`)
        || path.basename(filePath) === "tests.rs"
        || path.basename(filePath).endsWith("_tests.rs")
      ) continue;
      let source = productionRustSource(readFileSync(filePath, "utf8"));
      if (
        filePath === paidInvocationContractOwner
        || filePath === paidInvocationFingerprintOwner
        || filePath === paidSkillListingContractOwner
        || filePath === x402ContractOwner
      ) {
        const forbiddenRuntimeMarker = [
          /\bstd::fs\b/u,
          /\bstd::net\b/u,
          /\bstd::process\b/u,
          /\b(?:reqwest|hyper|axum|sqlx|diesel|aws_sdk|stripe|coinbase)\b/u,
        ].find((pattern) => pattern.test(source));
        if (forbiddenRuntimeMarker) {
          findings.push(
            `${relative(filePath)} crosses the inert public-contract boundary with ${forbiddenRuntimeMarker}`,
          );
        }
        continue;
      }
      if (
        filePath === paidInvocationFixtureProducer
        || filePath === x402FixtureProducer
        || filePath === officialConnectors
      ) continue;
      if (filePath === contractsLib) {
        source = source
          .replace(/pub mod paid_invocation;\s*/gu, "")
          .replace(/pub use paid_invocation::\{[\s\S]*?\};\s*/gu, "")
          .replace(/pub mod paid_skill_listing;\s*/gu, "")
          .replace(/pub use paid_skill_listing::\{[\s\S]*?\};\s*/gu, "")
          .replace(/pub mod x402;\s*/gu, "")
          .replace(/pub use x402::\{[\s\S]*?\};\s*/gu, "");
      }
      if (filePath === schemaArtifactsProjection) {
        source = source
          .replace(/X402[A-Za-z0-9_]*/gu, "ExternalContract")
          .replace(/x402[-_.A-Za-z0-9/]*/gu, "external-contract");
      }
      const lines = source.split(/\r?\n/u);
      lines.forEach((line, index) => {
        for (const token of line.matchAll(/[A-Za-z_][A-Za-z0-9_]*/gu)) {
          const banned = splitIdentifierParts(token[0]).find((part) => domainTokens.has(part));
          if (banned) {
            findings.push(`${relative(filePath)}:${index + 1} contains domain token '${banned}' in '${token[0]}'`);
          }
        }
      });
    }
  }

  for (const filePath of rustFiles("crates/runx-x402/src")) {
    const source = readFileSync(filePath, "utf8");
    const forbiddenRuntimeMarker = [
      /\bstd::fs\b/u,
      /\bstd::net\b/u,
      /\bstd::process\b/u,
      /\b(?:reqwest|hyper|axum|sqlx|diesel|aws_sdk|stripe|coinbase|tokio)\b/u,
    ].find((pattern) => pattern.test(source));
    if (forbiddenRuntimeMarker) {
      findings.push(
        `${relative(filePath)} crosses the inert x402 presentation boundary with ${forbiddenRuntimeMarker}`,
      );
    }
  }

  for (const relPath of [
    "crates/runx-runtime/src/execution/target_runner.rs",
    "crates/runx-runtime/src/execution/target_runner",
    "crates/runx-runtime/src/post_merge_observer.rs",
    "crates/runx-runtime/src/post_merge_observer",
    "crates/runx-contracts/src/target_runner.rs",
    "crates/runx-contracts/src/target_runner",
    "crates/runx-contracts/src/post_merge_observer.rs",
    "crates/runx-contracts/src/post_merge_observer",
  ]) {
    if (existsSync(path.join(workspaceRoot, relPath))) {
      findings.push(`${relPath} reintroduces retired provider orchestration`);
    }
  }

  const providerClientMarkers = [/\breqwest\b/u, /\bapi\.github\.com\b/u, /\bGITHUB_TOKEN\b/u, /\bbearer_auth\b/u];
  for (const root of ["crates/runx-runtime/src/adapters"]) {
    for (const filePath of rustFiles(root)) {
      const source = readFileSync(filePath, "utf8");
      const marker = providerClientMarkers.find((pattern) => pattern.test(source));
      if (marker) {
        findings.push(`${relative(filePath)} contains outbound GitHub provider client marker ${marker}`);
      }
    }
  }

  const retiredWirePatterns = [
    /PaymentAuthorityBounds/u,
    /PaymentCredentialForm/u,
    /\bbounds\.payment\b/u,
    /max_spend_usd/u,
    /max_per_call_minor/u,
    /max_per_run_minor/u,
    /max_per_period_minor/u,
    /payment_single_use_spend/u,
    /single_use_spend_capability/u,
    /ProofKind::PaymentRail/u,
    /"payment_rail"/u,
    /\bpayment_rail\b/u,
    /EffectSettlementReceipt/u,
    /\beffect_settlement\b/u,
    /\beffect-settlement\b/u,
    /\bpayment_required\b/u,
    /payment_rail_packet/u,
    /runx\.payment\.rail\.v1/u,
    /\bquote_required\b/u,
    /\breservation_required\b/u,
    /\bcredential_form\b/u,
    /\bsingle_use_spend\b/u,
    /resource_family:\s*payment/u,
    /"resource_family"\s*:\s*"payment"/u,
  ];
  const roots = ["crates/runx-contracts", "packages/contracts/src", "schemas", "fixtures", "skills", "examples", "scripts", "docs"];
  const extensions = new Set([".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".json", ".yaml", ".yml", ".md"]);
  for (const root of roots) {
    const absoluteRoot = path.join(workspaceRoot, root);
    for (const filePath of existsSync(absoluteRoot) ? walk(absoluteRoot) : []) {
      if (!extensions.has(path.extname(filePath)) || isArchitectureCheckFile(filePath)) continue;
      const source = readFileSync(filePath, "utf8");
      const pattern = retiredWirePatterns.find((candidate) => candidate.test(source));
      if (pattern) {
        findings.push(`${relative(filePath)} contains retired generic-contract wire name ${pattern}`);
      }
    }
  }
}
