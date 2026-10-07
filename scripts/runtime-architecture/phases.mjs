import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import {
  countMatches,
  productionRustSource,
  relative,
  rustFiles,
  workspaceRoot,
} from "./context.mjs";

export function checkServiceBoundary(findings) {
  const roots = [
    "crates/runx-runtime/src/adapters",
    "crates/runx-runtime/src/execution",
  ];
  const forbidden = [
    /\bRuntimeReceiptSignatureConfig::from_env\b/u,
    /\bLocalReceiptStore::new\b/u,
    /\bresolve_receipt_path\s*\(/u,
    /\bstd::env::(?:var|vars)\s*\(/u,
  ];
  for (const root of roots) {
    for (const filePath of rustFiles(root)) {
      const source = productionRustSource(readFileSync(filePath, "utf8"));
      for (const pattern of forbidden) {
        if (pattern.test(source)) {
          findings.push(`${relative(filePath)} still constructs environment or receipt state outside runtime services`);
        }
      }
    }
  }
}

export function checkExecutionSplit(findings, root = workspaceRoot) {
  const runner = "crates/runx-runtime/src/execution/runner";
  for (const owner of ["step_handlers.rs", "step_handlers/inputs.rs", "step_handlers/output.rs", "step_handlers/host_resolution.rs"]) {
    const relativePath = `${runner}/${owner}`;
    const filePath = path.join(root, relativePath);
    if (!existsSync(filePath)) {
      findings.push(`Missing current execution owner ${relativePath}.`);
      continue;
    }
    findings.push(...checkExecutionSource(owner, productionRustSource(readFileSync(filePath, "utf8"))));
  }
  if (existsSync(path.join(root, runner, "steps.rs"))) {
    findings.push(`${runner}/steps.rs is retired; step_handlers owns concrete step execution.`);
  }
}

export function checkExecutionSource(owner, source) {
  const findings = [];
  // Check responsibility crossings, not file size. The concrete handler retains
  // the ordered admit/invoke/seal sequence; its data helpers must not perform it.
  const forbidden = owner === "step_handlers.rs"
    ? /\brequest_approval\b/u
    : owner === "step_handlers/host_resolution.rs"
      ? /\b(?:SkillAdapter|BorrowedSkillAdapter|seal_step|prepare_effect_execution|resolve_inputs)\b/u
      : /\b(?:SkillAdapter|BorrowedSkillAdapter|Host|request_approval|seal_step|prepare_effect_execution|LocalReceiptStore)\b|\bcrate::(?:host|receipts|effects)::|\bstd::(?:fs|net|process)::/u;
  if (forbidden.test(source)) {
    findings.push(`${owner} crosses its execution responsibility boundary (${forbidden}).`);
  }
  if (owner === "step_handlers.rs") {
    for (const child of ["inputs", "output", "host_resolution"]) {
      if (!new RegExp(`\\bmod\\s+${child}\\s*;`, "u").test(source)) {
        findings.push(`step_handlers.rs must declare its ${child} owner.`);
      }
    }
  }
  return findings;
}

export function checkProjectionHotPaths(findings) {
  const runtimeRoot = path.join(workspaceRoot, "crates/runx-runtime/src");
  const compactIndexFound = rustFiles("crates/runx-runtime/src").some((filePath) => {
    const source = readFileSync(filePath, "utf8");
    return /\bstruct\s+\w*(?:Id)?Interner\b/u.test(source)
      || /\bstruct\s+\w*(?:Step)?PositionIndex\b[\s\S]*?\bpositions:\s*BTreeMap<String,\s*usize>/u.test(source);
  });
  if (!compactIndexFound) {
    findings.push(`${relative(runtimeRoot)} has no runtime-local id interner or compact position index for hot execution/projection paths`);
  }

  const cloneBudget = new Map([
    ["crates/runx-runtime/src/execution/graph_index.rs", 8],
    ["crates/runx-runtime/src/execution/output_projection.rs", 8],
  ]);
  for (const [relPath, maxClones] of cloneBudget) {
    const filePath = path.join(workspaceRoot, relPath);
    if (!existsSync(filePath)) {
      continue;
    }
    const count = countMatches(readFileSync(filePath, "utf8"), /\.clone\s*\(/gu);
    if (count > maxClones) {
      findings.push(`${relPath} has ${count} .clone() calls, above budget ${maxClones}`);
    }
  }
}

export function checkSessionPooling(findings) {
  for (const filePath of rustFiles("crates/runx-runtime/src")) {
    const source = readFileSync(filePath, "utf8");
    if (/\b(?:cli.*pool|pool.*cli|user command pool|pooled.*Command|CommandPool)\b/iu.test(source)) {
      findings.push(`${relative(filePath)} appears to pool arbitrary CLI/user commands`);
    }
  }
}
