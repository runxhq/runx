import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import {
  isArchitectureCheckFile,
  productionRustSource,
  relative,
  rustFiles,
  skillProductionFiles,
  walk,
  workspaceRoot,
} from "./context.mjs";

export function checkCanonicalParserOwnership(findings) {
  const cliManifestPath = path.join(workspaceRoot, "crates/runx-cli/Cargo.toml");
  const cliManifest = existsSync(cliManifestPath) ? readFileSync(cliManifestPath, "utf8") : "";
  if (/^serde_(?:norway|yaml|yml)\s*=/mu.test(cliManifest)) {
    findings.push(`${relative(cliManifestPath)} depends on a YAML backend instead of runx-parser`);
  }

  const parserRoot = path.join(workspaceRoot, "crates/runx-parser");
  for (const filePath of rustFiles("crates")) {
    if (filePath.startsWith(`${parserRoot}${path.sep}`) || filePath.includes(`${path.sep}tests${path.sep}`)) {
      continue;
    }
    const source = readFileSync(filePath, "utf8");
    if (/\b(?:serde_norway|serde_yaml|serde_yml|yaml_rust)::/u.test(source)) {
      findings.push(`${relative(filePath)} parses YAML outside the canonical runx-parser crate`);
    }
  }

  const runtimeHarnessFixturePath = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/execution/harness/fixtures.rs",
  );
  const runtimeHarnessFixture = existsSync(runtimeHarnessFixturePath)
    ? productionRustSource(readFileSync(runtimeHarnessFixturePath, "utf8"))
    : "";
  if (/pub\s+fn\s+parse_harness_fixture|\bproject_parser_error\b|HarnessFixtureError::(?:Required|Empty|Invalid|UnknownReceiptField)/u.test(runtimeHarnessFixture)) {
    findings.push(`${relative(runtimeHarnessFixturePath)} mirrors parser-owned harness parsing or diagnostics`);
  }
  const runtimeDevLoopPath = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/dev/loop.rs",
  );
  const runtimeDevLoop = existsSync(runtimeDevLoopPath)
    ? productionRustSource(readFileSync(runtimeDevLoopPath, "utf8"))
    : "";
  if (/\bparse_yaml_document\b|\bParsedDevFixture\b|fixture\.document|json_(?:object|string)_field\([^\n]*fixture/u.test(runtimeDevLoop)) {
    findings.push(`${relative(runtimeDevLoopPath)} reparses parser-owned dev fixture contracts`);
  }
  const parserLibPath = path.join(workspaceRoot, "crates/runx-parser/src/lib.rs");
  const parserLib = existsSync(parserLibPath) ? readFileSync(parserLibPath, "utf8") : "";
  if (!parserLib.includes("parse_dev_fixture") || !parserLib.includes("DevFixture")) {
    findings.push(`${relative(parserLibPath)} must own the typed runx dev fixture contract`);
  }
  const runtimeConfigPath = path.join(workspaceRoot, "crates/runx-runtime/src/config.rs");
  const runtimeConfig = existsSync(runtimeConfigPath)
    ? productionRustSource(readFileSync(runtimeConfigPath, "utf8"))
    : "";
  if (/parse_yaml_document[\s\S]{0,240}manifest|manifest_text[\s\S]{0,240}JsonValue::Object/u.test(runtimeConfig)) {
    findings.push(`${relative(runtimeConfigPath)} reparses runner profile manifests outside runx-parser`);
  }
  const runtimeLib = path.join(workspaceRoot, "crates/runx-runtime/src/lib.rs");
  const runtimeSource = existsSync(runtimeLib) ? readFileSync(runtimeLib, "utf8") : "";
  if (/\b(?:ParserEvalError|ParserEvalOutput|evaluate_parser_document_str|parse_yaml_document)\b/u.test(runtimeSource)) {
    findings.push(`${relative(runtimeLib)} re-exports parser ownership through the runtime`);
  }

  for (const root of ["scripts", "tests"]) {
    const absoluteRoot = path.join(workspaceRoot, root);
    for (const filePath of existsSync(absoluteRoot) ? walk(absoluteRoot) : []) {
      if (!/\.(?:js|mjs|cjs|ts)$/u.test(filePath)) continue;
      const source = readFileSync(filePath, "utf8");
      if (/from\s+["']yaml["']|require\(["']yaml["']\)/u.test(source)) {
        findings.push(`${relative(filePath)} parses YAML outside the canonical native parser`);
      }
      if (
        !isArchitectureCheckFile(filePath)
        && /\b(?:assertExecutionProfileYamlSubset|parseSkillFrontmatter)\b/u.test(source)
      ) {
        findings.push(`${relative(filePath)} reimplements a canonical parser contract`);
      }
    }
  }
  for (const filePath of skillProductionFiles()) {
    const source = readFileSync(filePath, "utf8");
    if (/\bparse(?:Skill)?Frontmatter\b|\bparse_frontmatter\b/u.test(source)) {
      findings.push(`${relative(filePath)} reimplements package frontmatter parsing outside runx-parser`);
    }
  }
  const packageManifest = JSON.parse(readFileSync(path.join(workspaceRoot, "package.json"), "utf8"));
  if (packageManifest.dependencies?.yaml || packageManifest.devDependencies?.yaml) {
    findings.push("package.json retains the parallel JavaScript YAML parser dependency");
  }
  const parserBridgePath = path.join(workspaceRoot, "scripts/lib/native-parser.mjs");
  const parserBridge = existsSync(parserBridgePath) ? readFileSync(parserBridgePath, "utf8") : "";
  for (const token of [
    "parser\", \"eval",
    "validateRunnerManifestYamlBatch",
    "validateHarnessFixtureYamlBatch",
    "parsePacketSchemaDocumentsBatch",
  ]) {
    if (!parserBridge.includes(token)) {
      findings.push(`${relative(parserBridgePath)} lacks canonical native-parser bridge token ${token}`);
    }
  }

  const packetGeneratorPath = path.join(workspaceRoot, "scripts/generate-packet-schemas.ts");
  const packetGenerator = existsSync(packetGeneratorPath)
    ? readFileSync(packetGeneratorPath, "utf8")
    : "";
  for (const token of [
    "ownedPacketContracts",
    'schema["x-runx-schema"]',
    'path.join(workspaceRoot, "schemas")',
    "parsePacketSchemaDocumentsBatch",
    "collectManifestContracts",
  ]) {
    if (!packetGenerator.includes(token)) {
      findings.push(`${relative(packetGeneratorPath)} must discover Rust-owned packet identities from root schemas`);
      break;
    }
  }
  if (/\.raw\.document|\bcollectContracts\s*\(/u.test(packetGenerator)) {
    findings.push(`${relative(packetGeneratorPath)} reparses raw runner manifests instead of using typed parser output`);
  }

  for (const filePath of walk(path.join(workspaceRoot, "scripts"))) {
    if (!/\.(?:js|mjs|cjs|ts)$/u.test(filePath) || isArchitectureCheckFile(filePath)) continue;
    if (/\.raw\.document/u.test(readFileSync(filePath, "utf8"))) {
      findings.push(`${relative(filePath)} interprets raw parser output instead of typed parser IR`);
    }
  }
  const skillFixtureGeneratorPath = path.join(
    workspaceRoot,
    "scripts/generate-rust-skill-fixtures.ts",
  );
  const skillFixtureGenerator = existsSync(skillFixtureGeneratorPath)
    ? readFileSync(skillFixtureGeneratorPath, "utf8")
    : "";
  if (!skillFixtureGenerator.includes("validateRunnerManifestYamlBatch")) {
    findings.push(`${relative(skillFixtureGeneratorPath)} must consume parser-owned typed runner structure`);
  }

  const packetParserPath = path.join(workspaceRoot, "crates/runx-parser/src/packet.rs");
  const packetParser = existsSync(packetParserPath) ? readFileSync(packetParserPath, "utf8") : "";
  for (const token of ["PACKET_ID_FIELD", "parse_packet_schema_document", "ValidatedPacketSchema"]) {
    if (!packetParser.includes(token)) {
      findings.push(`${relative(packetParserPath)} lacks canonical packet parser token ${token}`);
    }
  }
  const packetCatalogPath = path.join(workspaceRoot, "crates/runx-runtime/src/packet_schemas.rs");
  const packetCatalog = existsSync(packetCatalogPath) ? readFileSync(packetCatalogPath, "utf8") : "";
  for (const token of [
    "PacketSchemaCatalog",
    "parse_packet_schema_document",
    "packet_schema_directories",
    "discover_loaded_package",
  ]) {
    if (!packetCatalog.includes(token)) {
      findings.push(`${relative(packetCatalogPath)} lacks canonical packet catalog token ${token}`);
    }
  }
  const graphTypesPath = path.join(workspaceRoot, "crates/runx-parser/src/graph/types.rs");
  const graphTypes = existsSync(graphTypesPath) ? readFileSync(graphTypesPath, "utf8") : "";
  if (!/pub\s+artifacts:\s+Option<SkillArtifactContract>/u.test(graphTypes)) {
    findings.push(`${relative(graphTypesPath)} must expose parser-validated graph artifact contracts`);
  }
  if (!/pub\s+kind:\s+StepKind/u.test(graphTypes)) {
    findings.push(`${relative(graphTypesPath)} must expose parser-validated step kinds`);
  }
  for (const filePath of rustFiles("crates")) {
    if (filePath.startsWith(`${parserRoot}${path.sep}`)) continue;
    const source = readFileSync(filePath, "utf8");
    if (/runner\.raw\.get\(\s*"scopes"\s*\)|\bcollect_declared_scopes\b/u.test(source)) {
      findings.push(`${relative(filePath)} reparses runner scopes instead of using parser-owned declared_scopes`);
    }
    if (/source\.raw\.get\(\s*"allowed_tools"\s*\)/u.test(source)) {
      findings.push(`${relative(filePath)} reparses allowed_tools instead of using the typed invocation contract`);
    }
    if (/validate_skill_source\(\s*run\b|\brun\.get\(\s*"(?:type|outputs)"/u.test(source)) {
      findings.push(`${relative(filePath)} reparses a step target instead of using its parser-owned StepKind`);
    }
  }
  const operatorContextPath = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/execution/operator_context.rs",
  );
  const operatorContext = existsSync(operatorContextPath)
    ? productionRustSource(readFileSync(operatorContextPath, "utf8"))
    : "";
  if (!/struct\s+SkillOperatorContextStep[\s\S]*?pub\s+definition:\s+GraphStep/u.test(operatorContext)) {
    findings.push(`${relative(operatorContextPath)} must retain GraphStep as the typed graph-step owner`);
  }
  const preparedSkillPath = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/execution/prepared_skill.rs",
  );
  const preparedSkill = existsSync(preparedSkillPath)
    ? productionRustSource(readFileSync(preparedSkillPath, "utf8"))
    : "";
  if (/step\.raw|json_field\s*\(\s*&step\.|collect_string_values\s*\(\s*&step\./u.test(preparedSkill)) {
    findings.push(`${relative(preparedSkillPath)} reparses serialized graph steps instead of using GraphStep`);
  }
}
