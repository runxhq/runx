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

  for (const relPath of [
    "tools/spec/normalize_scafld_frontmatter",
    "tools/spec/read_declared_files",
  ]) {
    if (existsSync(path.join(workspaceRoot, relPath))) {
      findings.push(`${relPath} parses scafld-owned Markdown outside scafld`);
    }
  }
  for (const relPath of [
    "tests/http-cached-registry-store.test.ts",
    "tests/registry-fixtures.ts",
    "tests/util-split-skill-id.test.ts",
  ]) {
    if (existsSync(path.join(workspaceRoot, relPath))) {
      findings.push(`${relPath} retains a parallel TypeScript registry implementation`);
    }
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

  const runtimeFacade = path.join(workspaceRoot, "crates/runx-runtime/src/parser_eval.rs");
  if (existsSync(runtimeFacade)) {
    findings.push(`${relative(runtimeFacade)} is a redundant parser facade; callers must depend on runx-parser`);
  }
  const runtimeHarnessFixturePath = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/execution/harness/fixtures.rs",
  );
  const runtimeHarnessFixture = existsSync(runtimeHarnessFixturePath)
    ? productionRustSource(readFileSync(runtimeHarnessFixturePath, "utf8"))
    : "";
  if (/pub\s+fn\s+parse_harness_fixture|\bproject_parser_error\b|HarnessFixtureError::(?:Required|Empty|Invalid|RetiredReceiptField|UnknownReceiptField)/u.test(runtimeHarnessFixture)) {
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
  const readinessPath = path.join(workspaceRoot, "scripts/check-readiness-structural.mjs");
  const readinessSource = existsSync(readinessPath) ? readFileSync(readinessPath, "utf8") : "";
  if (/\bextractFrontmatterField\b/u.test(readinessSource)) {
    findings.push(`${relative(readinessPath)} must use scafld-owned path identity, not parse lifecycle front matter`);
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
  if (/\bcanonicalPacketContracts\b/u.test(packetGenerator)) {
    findings.push(`${relative(packetGeneratorPath)} retains a parallel packet-contract registry`);
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
  const versionDriftPath = path.join(workspaceRoot, "scripts/check-skill-version-drift.mjs");
  const versionDrift = existsSync(versionDriftPath) ? readFileSync(versionDriftPath, "utf8") : "";
  if (/\b(?:consumedScripts|visitValues|normalizeScriptPath)\b/u.test(versionDrift)) {
    findings.push(`${relative(versionDriftPath)} guesses package dependencies from arbitrary manifest values`);
  }
  const skillFixtureGeneratorPath = path.join(
    workspaceRoot,
    "scripts/generate-rust-skill-fixtures.ts",
  );
  const skillFixtureGenerator = existsSync(skillFixtureGeneratorPath)
    ? readFileSync(skillFixtureGeneratorPath, "utf8")
    : "";
  if (!skillFixtureGenerator.includes("source.graph?.steps")) {
    findings.push(`${relative(skillFixtureGeneratorPath)} must consume parser-owned typed graph structure`);
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
  const parallelPacketConsumers = [
    ["crates/runx-runtime/src/list.rs", /\bstruct\s+PacketSchema\b|\bfn\s+packet_id\s*\(/u],
    ["crates/runx-runtime/src/packet_validation.rs", /\bdiscover_packet_schemas\b|\bstruct\s+PacketSchema\b/u],
    ["crates/runx-runtime/src/registry/publish_package/files/packet.rs", /\bdiscover_packet_schemas\b|\bread_packet_schema\b|\bserde_json\b|\bstd::fs\b/u],
  ];
  for (const [relPath, pattern] of parallelPacketConsumers) {
    const filePath = path.join(workspaceRoot, relPath);
    const source = existsSync(filePath) ? productionRustSource(readFileSync(filePath, "utf8")) : "";
    if (pattern.test(source)) {
      findings.push(`${relPath} retains parallel packet parser or catalog ownership`);
    }
  }

  const graphTypesPath = path.join(workspaceRoot, "crates/runx-parser/src/graph/types.rs");
  const graphTypes = existsSync(graphTypesPath) ? readFileSync(graphTypesPath, "utf8") : "";
  if (!/pub\s+artifacts:\s+Option<SkillArtifactContract>/u.test(graphTypes)) {
    findings.push(`${relative(graphTypesPath)} must expose parser-validated graph artifact contracts`);
  }
  if (!/pub\s+run:\s+Option<GraphRunTarget>/u.test(graphTypes)) {
    findings.push(`${relative(graphTypesPath)} must expose parser-validated inline graph targets`);
  }
  const typedArtifactConsumers = [
    ["crates/runx-runtime/src/packet_validation.rs", /\binline_artifacts\b|artifacts\.get\s*\(/u],
    ["crates/runx-runtime/src/output_contract.rs", /\binline_artifacts\b/u],
    ["crates/runx-runtime/src/list.rs", /\bjson_artifact_emits\b/u],
    ["crates/runx-cli/src/registry/package.rs", /\bcollect_declared_packet_ids\b/u],
    ["crates/runx-runtime/src/adapters/agent.rs", /source\.raw[\s\S]{0,160}?artifacts/u],
  ];
  for (const [relPath, pattern] of typedArtifactConsumers) {
    const filePath = path.join(workspaceRoot, relPath);
    const source = existsSync(filePath) ? readFileSync(filePath, "utf8") : "";
    if (pattern.test(source)) {
      findings.push(`${relPath} reparses artifact contracts outside runx-parser`);
    }
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
      findings.push(`${relative(filePath)} reparses an inline graph target instead of using GraphRunTarget`);
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
  if (/SkillOperatorContextTarget|struct\s+SkillOperatorContextStep[\s\S]*?pub\s+raw:\s+JsonValue/u.test(operatorContext)) {
    findings.push(`${relative(operatorContextPath)} retains a parallel graph-step target or raw contract`);
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
  const registryPackagePath = path.join(workspaceRoot, "crates/runx-cli/src/registry/package.rs");
  const registryPackage = existsSync(registryPackagePath) ? readFileSync(registryPackagePath, "utf8") : "";
  if (/\bcollect_keyed_string_values\b|\bcollect_script_string_values\b/u.test(registryPackage)) {
    findings.push(`${relative(registryPackagePath)} recursively guesses execution sidecars instead of using typed contracts`);
  }
  const sourceTypesPath = path.join(workspaceRoot, "crates/runx-parser/src/skill/types.rs");
  const sourceTypes = existsSync(sourceTypesPath) ? readFileSync(sourceTypesPath, "utf8") : "";
  const sourceKind = sourceTypes.match(/pub enum SourceKind\s*\{([\s\S]*?)\n\}/u)?.[1] ?? "";
  for (const retired of ["Catalog", "HarnessHook", "Http"]) {
    if (new RegExp(`\\b${retired}\\b`, "u").test(sourceKind)) {
      findings.push(`${relative(sourceTypesPath)} retains retired ${retired} source ownership`);
    }
  }
  const skillSource = sourceTypes.match(/pub struct SkillSource\s*\{([\s\S]*?)\n\}/u)?.[1] ?? "";
  if (/\bpub\s+catalog_ref\s*:/u.test(skillSource)) {
    findings.push(`${relative(sourceTypesPath)} retains catalog_ref on SkillSource; graph tool steps own catalog dispatch`);
  }
  const sourceParserPath = path.join(workspaceRoot, "crates/runx-parser/src/skill/source.rs");
  const sourceParser = existsSync(sourceParserPath) ? readFileSync(sourceParserPath, "utf8") : "";
  for (const retired of ["http", "catalog"]) {
    if (!sourceParser.includes(`{field} ${retired} was removed`)) {
      findings.push(`${relative(sourceParserPath)} must fail explicitly for retired source.type ${retired}`);
    }
  }
  for (const relPath of [
    "crates/runx-runtime/src/tool_catalogs/build.rs",
    "crates/runx-runtime/src/tool_catalogs/inspect.rs",
    "crates/runx-runtime/src/dev/tool.rs",
  ]) {
    const filePath = path.join(workspaceRoot, relPath);
    const source = existsSync(filePath) ? readFileSync(filePath, "utf8") : "";
    if (/\bRawToolManifest\b|\bnormalize_tool_(?:manifest_shape|output)\b|\bruntime_from_source\b|artifacts\.get\s*\(/u.test(source)) {
      findings.push(`${relPath} reparses tool manifests instead of projecting parser-owned typed IR`);
    }
  }
  for (const filePath of rustFiles("crates/runx-cli/src")) {
    const source = readFileSync(filePath, "utf8");
    if (/\bcollect_process_script_files\b|\bprocess_script_files\b/u.test(source)) {
      findings.push(`${relative(filePath)} guesses execution sidecars instead of using parser-owned execution_files`);
    }
  }
  for (const [relPath, retired] of [
    ["crates/runx-runtime/src/execution/runner/steps.rs", /\bStepTypeRegistry\b|\bregistered_step_type\b|\brun_type_ref\b/u],
    ["crates/runx-runtime/src/execution/skill_front/graph.rs", /\bSourceAdapterRegistry\b|\bbuiltin_source_handlers\b/u],
  ]) {
    const filePath = path.join(workspaceRoot, relPath);
    const source = existsSync(filePath) ? readFileSync(filePath, "utf8") : "";
    if (retired.test(source)) {
      findings.push(`${relPath} retains a string registry beside typed source/step dispatch`);
    }
  }

  const retiredBinaryAliases = [
    `RUNX_${"KERNEL_EVAL_BIN"}`,
    `RUNX_${"PARSER_EVAL_BIN"}`,
    `RUNX_${"DEV_RUST_CLI_BIN"}`,
  ];
  for (const root of ["scripts", "tests"]) {
    const absoluteRoot = path.join(workspaceRoot, root);
    for (const filePath of existsSync(absoluteRoot) ? walk(absoluteRoot) : []) {
      if (!/\.(?:js|mjs|cjs|ts)$/u.test(filePath) || isArchitectureCheckFile(filePath)) continue;
      const source = readFileSync(filePath, "utf8");
      const alias = retiredBinaryAliases.find((candidate) => source.includes(candidate));
      if (alias) {
        findings.push(`${relative(filePath)} retains binary alias ${alias}; use RUNX_RUST_CLI_BIN`);
      }
    }
  }
}
