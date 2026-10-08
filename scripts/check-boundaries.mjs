#!/usr/bin/env node
import { readFile, readdir, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(
  process.env.RUNX_BOUNDARY_WORKSPACE_ROOT ?? fileURLToPath(new URL("..", import.meta.url)),
);
const boundaryGuardPath = path.resolve(fileURLToPath(import.meta.url));

const sourceExtensions = new Set([".ts", ".tsx", ".mts", ".cts"]);
const activeTypeScriptJavaScriptExtensions = new Set([
  ".ts",
  ".tsx",
  ".mts",
  ".cts",
  ".js",
  ".jsx",
  ".mjs",
  ".cjs",
]);
const activeCredentialContractExtensions = new Set([
  ".ts",
  ".tsx",
  ".mts",
  ".cts",
  ".js",
  ".jsx",
  ".mjs",
  ".cjs",
  ".rs",
  ".json",
]);
const ignoredDirectoryNames = new Set([
  ".git",
  ".turbo",
  "node_modules",
  "dist",
  ".build",
  "coverage",
  "target",
]);
const hostedConnectBrokerageScanRoots = ["packages", "plugins", "scripts", "tests"];
const hostedCredentialContractScanRoots = [
  "packages",
  "plugins",
  "scripts",
  "tests",
  "fixtures/contracts",
  "schemas",
  "crates/runx-contracts/src",
  "crates/runx-contracts/tests",
  "crates/runx-runtime/src",
  "crates/runx-core/src",
];
const literalName = (...parts) => parts.join("");
const literalPattern = (...parts) => new RegExp(literalName(...parts));
const privateProviderGatewayUpstreamPattern = new RegExp("nan" + "go", "i");
const hostedOAuthAuthModePattern = /["']?auth_mode["']?\s*[:=]\s*["']oauth(?:_bearer)?["']/;
const gatewayConnectionIdPattern = new RegExp("\\bco" + "nn_[A-Za-z0-9_:-]+");
const forbiddenHostedConnectBrokerageTerms = [
  { name: "private provider gateway upstream", pattern: privateProviderGatewayUpstreamPattern },
  { name: literalName("oauth", "_required"), pattern: literalPattern("oauth", "_required") },
  { name: literalName("authorize", "_url"), pattern: literalPattern("authorize", "_url") },
  { name: literalName("Connect", "Session"), pattern: literalPattern("Connect", "Session") },
  { name: literalName("Hosted", "Provider", "Reference"), pattern: literalPattern("Hosted", "Provider", "Reference") },
  { name: literalName("connect", "-http"), pattern: literalPattern("connect", "-http") },
  { name: literalName("create", "Http", "Connect", "Service"), pattern: literalPattern("create", "Http", "Connect", "Service") },
  {
    name: literalName("RUNX_CONNECT_PROVIDER", "_GATEWAY"),
    pattern: literalPattern("RUNX_CONNECT_PROVIDER", "_GATEWAY"),
  },
];
const forbiddenHostedCredentialContractTerms = [
  { name: "hosted OAuth auth_mode", pattern: hostedOAuthAuthModePattern },
  { name: "provider gateway connection id", pattern: gatewayConnectionIdPattern },
  { name: literalName("opaque", "_connection"), pattern: literalPattern("opaque", "_connection") },
  { name: literalName("redact", "_connect", "_text"), pattern: literalPattern("redact", "_connect", "_text") },
  { name: literalName("credential_delivery", ".broker", "_response"), pattern: literalPattern("credential_delivery", "\\.broker", "_response") },
  { name: literalName("credential_delivery", "_broker", "_response"), pattern: literalPattern("credential_delivery", "_broker", "_response") },
  { name: literalName("credential-delivery", "-broker", "-response"), pattern: literalPattern("credential-delivery", "-broker", "-response") },
  { name: literalName("CredentialDelivery", "Broker", "Response"), pattern: literalPattern("CredentialDelivery", "Broker", "Response") },
];
const packageDependencyFields = ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"];
const forbiddenPackageImports = {
  "client": {
    prefixes: ["@runxhq/cli", "@runxhq/langchain", "@langchain", "@anthropic-ai", "@ai-sdk", "ai"],
    reason: "@runxhq/client must not depend on CLI or framework packages.",
  },
  langchain: {
    prefixes: ["@runxhq/cli"],
    reason: "@runxhq/langchain must not depend on CLI or host packages.",
  },
};
const staticSpecifierPattern =
  /\b(?:import|export)\s+(?:type\s+)?(?:[^'";]*?\s+from\s+)?["']([^"']+)["']|import\s*\(\s*["']([^"']+)["']\s*\)/g;
const findings = [];
const packageManifestCache = new Map();
const workspacePackageNames = await readWorkspacePackageNames();

await checkForbiddenHostedConnectBrokerage();
await checkForbiddenHostedCredentialContracts();
for (const filePath of await findSourceFiles(workspaceRoot)) {
  await checkSourceFile(filePath);
}

if (findings.length > 0) {
  console.error("Boundary check failed:");
  for (const finding of findings) {
    console.error(`- ${finding}`);
  }
  process.exit(1);
}

console.log("Boundary check passed.");

async function checkForbiddenHostedConnectBrokerage() {
  for (const rootName of hostedConnectBrokerageScanRoots) {
    const rootPath = path.join(workspaceRoot, rootName);
    const entry = await statIfExists(rootPath);
    if (!entry?.isDirectory()) {
      continue;
    }

    for (const filePath of await findActiveTypeScriptJavaScriptFiles(rootPath)) {
      const rel = toPosix(path.relative(workspaceRoot, filePath));
      checkForbiddenHostedConnectBrokerageInText(rel, rel, "path");
      const source = await readFile(filePath, "utf8");
      checkForbiddenHostedConnectBrokerageInSource(rel, source);
    }
  }
}

function checkForbiddenHostedConnectBrokerageInSource(rel, source) {
  const lines = source.split(/\r?\n/);
  for (const [index, line] of lines.entries()) {
    checkForbiddenHostedConnectBrokerageInText(rel, line, `line ${index + 1}`);
  }
}

function checkForbiddenHostedConnectBrokerageInText(rel, text, location) {
  for (const term of forbiddenHostedConnectBrokerageTerms) {
    if (term.pattern.test(text)) {
      findings.push(`${rel} contains forbidden hosted connect/OAuth brokerage term ${term.name} in ${location}.`);
    }
  }
}

async function checkForbiddenHostedCredentialContracts() {
  for (const rootName of hostedCredentialContractScanRoots) {
    const rootPath = path.join(workspaceRoot, rootName);
    const entry = await statIfExists(rootPath);
    if (!entry?.isDirectory()) {
      continue;
    }

    for (const filePath of await findActiveCredentialContractFiles(rootPath)) {
      const rel = toPosix(path.relative(workspaceRoot, filePath));
      const source = await readFile(filePath, "utf8");
      const lines = source.split(/\r?\n/);
      for (const [index, line] of lines.entries()) {
        for (const term of forbiddenHostedCredentialContractTerms) {
          if (term.pattern.test(line)) {
            findings.push(`${rel} contains forbidden hosted OAuth credential contract term ${term.name} in line ${index + 1}.`);
          }
        }
      }
    }
  }
}

async function checkSourceFile(filePath) {
  const source = await readFile(filePath, "utf8");
  const specifiers = extractSpecifiers(source);
  const rel = toPosix(path.relative(workspaceRoot, filePath));
  const packageSource = getPackageSource(rel);

  for (const specifier of specifiers) {
    if (rel.startsWith("packages/client/src/adapters/") && !rel.endsWith(".test.ts")) {
      if (specifier.startsWith("node:") || specifier === "@runxhq/client" || specifier.startsWith("../") || (specifier.startsWith("@runxhq/client/") && specifier !== "@runxhq/client/adapters")) {
        findings.push(`${rel} imports ${specifier}; portable client adapters cannot import Node or the native client root.`);
      }
    }
    if (packageSource) {
      checkForbiddenPackageImport(rel, packageSource.packageName, specifier);
      await checkDeclaredWorkspaceImport(rel, packageSource.packageName, specifier);
    }

    if (rel.startsWith("packages/") && isCloudSpecifier(specifier)) {
      findings.push(`${rel} imports cloud code; oss must not depend on cloud.`);
    }
  }

}

function checkForbiddenPackageImport(rel, packageName, specifier) {
  const rule = forbiddenPackageImports[packageName];
  if (!rule) {
    return;
  }
  if (rule.prefixes.some((prefix) => specifierMatchesPackageName(specifier, prefix))) {
    findings.push(`${rel} imports ${specifier}; ${rule.reason}`);
  }
}

async function checkDeclaredWorkspaceImport(rel, packageName, specifier) {
  const dependencyName = workspaceDependencyName(specifier);
  if (!dependencyName || !workspacePackageNames.has(dependencyName)) {
    return;
  }

  const manifest = await readPackageManifest(packageName);
  if (!manifest || manifest.name === dependencyName) {
    return;
  }
  if (packageName === "cli" && isNativeCliArtifactManifest(manifest)) {
    return;
  }

  const declared = {
    ...manifest.dependencies,
    ...manifest.devDependencies,
    ...manifest.peerDependencies,
    ...manifest.optionalDependencies,
  };
  if (!Object.hasOwn(declared, dependencyName)) {
    findings.push(`${rel} imports ${specifier}; ${manifest.name} must declare ${dependencyName} in package.json.`);
  }
}

function isNativeCliArtifactManifest(manifest) {
  const bin = typeof manifest.bin === "string" ? manifest.bin : manifest.bin?.runx;
  const files = Array.isArray(manifest.files) ? manifest.files : [];
  const includesFileOrDirectory = (entry) => files.includes(entry) || files.some((file) => file.startsWith(`${entry}/`));
  return manifest.name === "@runxhq/cli"
    && bin === "./bin/runx"
    && includesFileOrDirectory("bin")
    && includesFileOrDirectory("native")
    && !files.includes("src")
    && !files.includes("dist")
    && !files.includes("tools");
}

function extractSpecifiers(source) {
  const specifiers = [];
  let match;
  while ((match = staticSpecifierPattern.exec(source)) !== null) {
    specifiers.push(match[1] ?? match[2]);
  }
  return specifiers;
}

function getPackageSource(rel) {
  const parts = rel.split("/");
  if (parts[0] !== "packages" || parts[2] !== "src") {
    return undefined;
  }
  return {
    packageName: parts[1],
    domain: parts[3] ?? "",
  };
}

function workspaceDependencyName(specifier) {
  const match = /^(@runxhq\/[^/]+)/.exec(specifier);
  return match?.[1];
}

function specifierMatchesPackageName(specifier, packageName) {
  return specifier === packageName || specifier.startsWith(`${packageName}/`);
}

async function readWorkspacePackageNames() {
  const packagesDir = path.join(workspaceRoot, "packages");
  const names = new Set();
  for (const entry of await readdir(packagesDir, { withFileTypes: true })) {
    if (!entry.isDirectory()) {
      continue;
    }
    const manifest = await readPackageManifest(entry.name);
    if (manifest?.name) {
      names.add(manifest.name);
    }
  }
  return names;
}

async function readPackageManifest(packageName) {
  if (packageManifestCache.has(packageName)) {
    return packageManifestCache.get(packageName);
  }
  const manifestPath = path.join(workspaceRoot, "packages", packageName, "package.json");
  const manifest = await readJsonIfExists(manifestPath);
  packageManifestCache.set(packageName, manifest);
  return manifest;
}

function isCloudSpecifier(specifier) {
  return specifier === "cloud" || specifier.startsWith("cloud/") || specifier.includes("/cloud/");
}

async function findSourceFiles(root) {
  const files = [];
  await walk(root, files);
  return files;
}

async function findActiveTypeScriptJavaScriptFiles(root) {
  const files = [];
  await walkActiveTypeScriptJavaScript(root, files);
  return files;
}

async function findActiveCredentialContractFiles(root) {
  const files = [];
  await walkActiveCredentialContract(root, files);
  return files;
}

async function walkActiveTypeScriptJavaScript(directory, files) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (!isIgnoredDirectoryName(entry.name)) {
        await walkActiveTypeScriptJavaScript(path.join(directory, entry.name), files);
      }
      continue;
    }
    if (!entry.isFile() || !activeTypeScriptJavaScriptExtensions.has(path.extname(entry.name))) {
      continue;
    }
    const filePath = path.join(directory, entry.name);
    if (path.resolve(filePath) === boundaryGuardPath) {
      continue;
    }
    files.push(filePath);
  }
}

async function walkActiveCredentialContract(directory, files) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (!isIgnoredDirectoryName(entry.name)) {
        await walkActiveCredentialContract(path.join(directory, entry.name), files);
      }
      continue;
    }
    if (!entry.isFile() || !activeCredentialContractExtensions.has(path.extname(entry.name))) {
      continue;
    }
    const filePath = path.join(directory, entry.name);
    if (path.resolve(filePath) === boundaryGuardPath) {
      continue;
    }
    files.push(filePath);
  }
}

async function walk(directory, files) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (!isIgnoredDirectoryName(entry.name)) {
        await walk(path.join(directory, entry.name), files);
      }
      continue;
    }
    if (!entry.isFile() || !sourceExtensions.has(path.extname(entry.name)) || isTestFile(entry.name)) {
      continue;
    }
    files.push(path.join(directory, entry.name));
  }
}

async function statIfExists(filePath) {
  try {
    return await stat(filePath);
  } catch (error) {
    if (isNotFound(error)) {
      return undefined;
    }
    throw error;
  }
}

async function readJsonIfExists(filePath) {
  let contents;
  try {
    contents = await readFile(filePath, "utf8");
  } catch (error) {
    if (isNotFound(error)) {
      return undefined;
    }
    throw error;
  }
  return JSON.parse(contents);
}

function isNotFound(error) {
  return Boolean(error && typeof error === "object" && "code" in error && error.code === "ENOENT");
}

function isTestFile(fileName) {
  return /\.(test|spec)\.(ts|tsx|mts|cts)$/.test(fileName);
}

function isIgnoredDirectoryName(name) {
  return ignoredDirectoryNames.has(name) || name.startsWith("target-");
}

function toPosix(input) {
  return input.split(path.sep).join("/");
}
