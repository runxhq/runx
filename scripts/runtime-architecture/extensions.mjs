import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import { relative, walk, workspaceRoot } from "./context.mjs";

export function checkAuthoringOwnership(findings) {
  const projectPath = path.join(workspaceRoot, "crates/runx-cli/src/project.rs");
  const projectSource = existsSync(projectPath) ? readFileSync(projectPath, "utf8") : "";
  for (const token of ["PathBuf::from(\"runx/skill-build\")", "runner: Some(\"build\".to_owned())"] ) {
    if (!projectSource.includes(token)) {
      findings.push(`${relative(projectPath)} must delegate runx new to skill-build build`);
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
}
