import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

import {
  escapeRegExp,
  productionRustSource,
  relative,
  rustFiles,
  skillProductionFiles,
  walk,
  workspaceRoot,
} from "./context.mjs";

export function checkCliCommandOwnership(findings) {
  const commandSpecPath = path.join(workspaceRoot, "crates/runx-cli/src/command_spec.rs");
  const commandSpec = existsSync(commandSpecPath) ? readFileSync(commandSpecPath, "utf8") : "";
  if (!/pub fn catalog_json\(\)/u.test(commandSpec) || !/CommandCatalog/u.test(commandSpec)) {
    findings.push(`${relative(commandSpecPath)} must project the native help catalog as JSON`);
  }

  const catalogPath = path.join(workspaceRoot, "crates/runx-cli/src/command_spec/catalog.rs");
  const catalog = existsSync(catalogPath) ? readFileSync(catalogPath, "utf8") : "";
  if (!/ROOT_COMMAND_SPEC/u.test(catalog) || !/--bearer-stdin/u.test(catalog)) {
    findings.push(`${relative(catalogPath)} must own root help and the complete native option catalog`);
  }

  const parityPath = path.join(workspaceRoot, "tests/cli-feature-parity-contract.ts");
  const parity = existsSync(parityPath) ? readFileSync(parityPath, "utf8") : "";
  for (const token of [
    "command(\"",
    "requiredPositionals",
    "conditionalPositionals",
    "checkHelpCoverage",
    "checkUsageCoverage",
    "command_spec/catalog.rs",
  ]) {
    if (parity.includes(token)) {
      findings.push(`${relative(parityPath)} duplicates native CLI syntax through '${token}'`);
    }
  }
  for (const required of [
    'spawnSync(runx, ["--help", "--json"]',
    "bindNativeCommands(readNativeCommandCatalog(runx), commandAnnotations)",
  ]) {
    if (!parity.includes(required)) {
      findings.push(`${relative(parityPath)} must consume the native JSON command catalog (${required})`);
    }
  }
  for (const relPath of [
    "scripts/generate-cli-feature-parity.ts",
    "fixtures/cli-parity/commands.json",
    "fixtures/cli-parity/runtime-surfaces.json",
    "fixtures/cli-parity/cases/oracle.json",
  ]) {
    if (existsSync(path.join(workspaceRoot, relPath))) {
      findings.push(`${relPath} is a generated CLI parity projection over the native command catalog`);
    }
  }

  const driftPath = path.join(workspaceRoot, "scripts/check-command-drift.mjs");
  const drift = existsSync(driftPath) ? readFileSync(driftPath, "utf8") : "";
  for (const token of ["command_spec/catalog.rs", "fixtures/cli-parity/commands.json", "matchAll(/CommandSpec"]) {
    if (drift.includes(token)) {
      findings.push(`${relative(driftPath)} must not parse or mirror the native command registry`);
    }
  }

  const packagePath = path.join(workspaceRoot, "package.json");
  const packageSource = existsSync(packagePath) ? readFileSync(packagePath, "utf8") : "";
  if (/fixtures:cli-help:check|fixtures:cli-parity:check|check-help-coverage|canonical-only/u.test(packageSource)) {
    findings.push(`${relative(packagePath)} retains a redundant CLI help/parity validation path`);
  }

  const cutoverPath = path.join(workspaceRoot, "scripts/check-rust-cli-cutover.ts");
  const cutover = existsSync(cutoverPath) ? readFileSync(cutoverPath, "utf8") : "";
  if (/noAliases|no-aliases|inspectCanonicalMatrix/u.test(cutover)) {
    findings.push(`${relative(cutoverPath)} retains a second alias registry over native command help`);
  }
}

export function checkRegistryOwnership(findings) {
  const retiredRegistryPaths = [
    "crates/runx-cli/src/registry/remote_publish/payloads.rs",
    "crates/runx-cli/src/registry/package.rs",
  ];
  for (const relPath of retiredRegistryPaths) {
    if (existsSync(path.join(workspaceRoot, relPath))) {
      findings.push(`${relPath} duplicates runtime-owned registry publish contracts`);
    }
  }
  const cliRegistryPaths = [
    path.join(workspaceRoot, "crates/runx-cli/src/registry.rs"),
    path.join(workspaceRoot, "crates/runx-cli/src/registry"),
  ];
  const files = cliRegistryPaths.flatMap((candidate) => {
    if (!existsSync(candidate)) return [];
    return path.extname(candidate) ? [candidate] : walk(candidate).filter((filePath) => filePath.endsWith(".rs"));
  });
  for (const filePath of files) {
    const source = readFileSync(filePath, "utf8");
    if (/\b(?:reqwest|ureq|isahc|attohttpc)::/u.test(source)) {
      findings.push(`${relative(filePath)} owns registry transport instead of using runx-runtime registry services`);
    }
    if (/\bstruct\s+RegistryClient\b/u.test(source)) {
      findings.push(`${relative(filePath)} declares a parallel registry client`);
    }
    if (/\bstruct\s+HostedSkillPackageFile\b/u.test(source)) {
      findings.push(`${relative(filePath)} duplicates runtime-owned RegistryPackageFile`);
    }
    if (/\b(?:load_validated_skill_package|parse_harness_fixture)\b/u.test(source)) {
      findings.push(`${relative(filePath)} prepares registry packages in the CLI instead of using the runtime publish service`);
    }
    if (/\b(?:canonical_remote_registry_url|PublishPackageView|publish_skill_package|publish_admin_package)\b/u.test(source)) {
      findings.push(`${relative(filePath)} retains registry identity or publish wrappers owned by runx-runtime`);
    }
  }
  const runtimeRegistryPath = path.join(workspaceRoot, "crates/runx-runtime/src/registry.rs");
  const runtimeRegistry = existsSync(runtimeRegistryPath)
    ? readFileSync(runtimeRegistryPath, "utf8")
    : "";
  if (/pub\s+use[^;]*\b(?:HttpRequest|HttpResponse|HttpTransport|DefaultRuntimeHttpTransport)\b/su.test(runtimeRegistry)) {
    findings.push(`${relative(runtimeRegistryPath)} re-exports canonical HTTP transport types through the registry`);
  }
  if (!runtimeRegistry.includes("canonical_registry_url")) {
    findings.push(`${relative(runtimeRegistryPath)} must expose the canonical registry source URL owner`);
  }
  const localRegistryPath = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/registry/local.rs",
  );
  const localRegistry = existsSync(localRegistryPath)
    ? productionRustSource(readFileSync(localRegistryPath, "utf8"))
    : "";
  if (/\bLocalRegistryClient\b|\bcreate_(?:file_registry_store|local_registry_client)\b|pub\s+fn\s+search_registry\s*\(/u.test(localRegistry)) {
    findings.push(`${relative(localRegistryPath)} retains aliases over the canonical FileRegistryStore`);
  }

  const registryHttpPath = path.join(workspaceRoot, "crates/runx-runtime/src/registry/http.rs");
  const registryHttp = existsSync(registryHttpPath) ? readFileSync(registryHttpPath, "utf8") : "";
  if (/\bfn\s+split_skill_id\s*\(/u.test(registryHttp)) {
    findings.push(`${relative(registryHttpPath)} retains a second registry skill-id parser`);
  }

  const publishPackagePath = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/registry/publish_package.rs",
  );
  const publishPackage = existsSync(publishPackagePath)
    ? readFileSync(publishPackagePath, "utf8")
    : "";
  for (const token of [
    "prepare_registry_publish_package",
    "RegistryPublishPackageRequest",
    "run_harness",
  ]) {
    if (!publishPackage.includes(token)) {
      findings.push(`${relative(publishPackagePath)} lacks canonical registry publish owner token ${token}`);
    }
  }
  const harnessInferencePath = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/registry/publish_package/files/harness_dependencies.rs",
  );
  if (existsSync(harnessInferencePath)) {
    findings.push(`${relative(harnessInferencePath)} infers harness dependencies from arbitrary values`);
  }
  const publishFilesPath = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/registry/publish_package/files.rs",
  );
  const publishFiles = existsSync(publishFilesPath) ? readFileSync(publishFilesPath, "utf8") : "";
  if (!publishFiles.includes("loaded.package.consumed_files")) {
    findings.push(`${relative(publishFilesPath)} must project parser-owned package material`);
  }
  if (/\b(?:collect_publish_harness_files|is_publishable_package_file)\b/u.test(publishFiles)
      || publishFiles.includes("loaded.package.harness_files")) {
    findings.push(`${relative(publishFilesPath)} retains a second package-membership policy`);
  }
}

export function checkHttpTransportOwnership(findings) {
  const cliHostedFacade = path.join(workspaceRoot, "crates/runx-cli/src/public_api.rs");
  if (existsSync(cliHostedFacade)) {
    findings.push(`${relative(cliHostedFacade)} duplicates runtime hosted-API ownership`);
  }
  const runtimeHttpRoot = path.join(workspaceRoot, "crates/runx-runtime/src/http");
  const runtimeHttpModule = path.join(runtimeHttpRoot, "mod.rs");
  const runtimeHttpSource = existsSync(runtimeHttpModule) ? readFileSync(runtimeHttpModule, "utf8") : "";
  if (/\bstruct\s+RuntimeHttpClient\b/u.test(runtimeHttpSource)) {
    findings.push(`${relative(runtimeHttpModule)} retains the unused generic RuntimeHttpClient wrapper`);
  }
  for (const filePath of rustFiles("crates/runx-cli/src")) {
    if (filePath.endsWith("_tests.rs")) continue;
    const source = readFileSync(filePath, "utf8");
    const production = source.split(/\n#\[cfg\(test\)\]\nmod\s+tests\b/u, 1)[0] ?? source;
    if (/\bRuntimeHttp(?:Request|Header)\b|\.send\(\s*(?:HttpRequest|RuntimeHttpRequest)\b/u.test(production)) {
      findings.push(`${relative(filePath)} constructs hosted HTTP requests instead of calling a runtime service`);
    }
  }
  for (const filePath of rustFiles("crates")) {
    if (filePath.startsWith(`${runtimeHttpRoot}${path.sep}`)) continue;
    const source = readFileSync(filePath, "utf8");
    if (/\breqwest::/u.test(source)) {
      findings.push(`${relative(filePath)} bypasses the canonical runtime HTTP transport`);
    }
  }

  const requestOwners = [
    "crates/runx-runtime/src/hosted_api/environment.rs",
    "crates/runx-runtime/src/hosted_api/request.rs",
    "crates/runx-runtime/src/hosted_api/skill_endpoint.rs",
    "crates/runx-runtime/src/registry/http.rs",
    "crates/runx-runtime/src/adapters/agent_anthropic_messages.rs",
    "crates/runx-runtime/src/adapters/agent_chat_completions.rs",
    "crates/runx-runtime/src/kernel/session.rs",
  ];
  const requestOwnerRoots = [
    "crates/runx-runtime/src/http/",
    "crates/runx-runtime/src/drivers/",
    "crates/runx-runtime/src/tool_catalogs/native/http/",
  ];
  for (const filePath of rustFiles("crates")) {
    const rel = relative(filePath);
    if (rel.includes("/tests/") || rel.endsWith("_tests.rs")) continue;
    const source = productionRustSource(readFileSync(filePath, "utf8"));
    const alias = source.match(/RuntimeHttpRequest\s+as\s+([A-Za-z_][A-Za-z0-9_]*)/u)?.[1];
    const constructsRequest = /\bRuntimeHttpRequest\s*\{/u.test(source)
      || (alias !== undefined && new RegExp(`\\b${escapeRegExp(alias)}\\s*\\{`, "u").test(source));
    if (!constructsRequest) continue;
    const allowed = requestOwners.includes(rel)
      || requestOwnerRoots.some((root) => rel.startsWith(root));
    if (!allowed) {
      findings.push(`${rel} constructs RuntimeHttpRequest outside a transport or named protocol owner`);
    }
  }

  const networkPattern = /\bfetch\s*\(|\bXMLHttpRequest\b|\b(?:https?|axios|undici|got)\.(?:get|request)\s*\(|from\s+["'](?:node:)?https?["']|require\(["'](?:node:)?https?["']\)/u;
  const allowed = new Map([
    [
      "skills/nitrosend/tools/nitrosend/bulk_import/run.mjs",
      "runx-architecture-allow: transient-signed-upload",
    ],
  ]);
  for (const filePath of skillProductionFiles()) {
    const source = readFileSync(filePath, "utf8");
    if (!networkPattern.test(source)) continue;
    const rel = relative(filePath);
    const marker = allowed.get(rel);
    if (!marker || !source.includes(marker)) {
      findings.push(`${rel} implements skill-owned HTTP; call a connector operation`);
    }
  }
}
