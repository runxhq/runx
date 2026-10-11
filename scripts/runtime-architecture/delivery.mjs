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
  for (const required of [
    'spawnSync(runx, ["--help", "--json"]',
    "bindNativeCommands(readNativeCommandCatalog(runx), commandAnnotations)",
  ]) {
    if (!parity.includes(required)) {
      findings.push(`${relative(parityPath)} must consume the native JSON command catalog (${required})`);
    }
  }
}

export function checkRegistryOwnership(findings) {
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
    if (/\b(?:load_validated_skill_package|parse_harness_fixture)\b/u.test(source)) {
      findings.push(`${relative(filePath)} prepares registry packages in the CLI instead of using the runtime publish service`);
    }
  }
  const runtimeRegistryPath = path.join(workspaceRoot, "crates/runx-runtime/src/registry.rs");
  const runtimeRegistry = existsSync(runtimeRegistryPath)
    ? readFileSync(runtimeRegistryPath, "utf8")
    : "";
  if (!runtimeRegistry.includes("canonical_registry_url")) {
    findings.push(`${relative(runtimeRegistryPath)} must expose the canonical registry source URL owner`);
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
  const publishFilesPath = path.join(
    workspaceRoot,
    "crates/runx-runtime/src/registry/publish_package/files.rs",
  );
  const publishFiles = existsSync(publishFilesPath) ? readFileSync(publishFilesPath, "utf8") : "";
  if (!publishFiles.includes("loaded.package.consumed_files")) {
    findings.push(`${relative(publishFilesPath)} must project parser-owned package material`);
  }
}

export function checkHttpTransportOwnership(findings) {
  const runtimeHttpRoot = path.join(workspaceRoot, "crates/runx-runtime/src/http");
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
      "connectors/nitrosend/tools/nitrosend/bulk_import/run.mjs",
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
