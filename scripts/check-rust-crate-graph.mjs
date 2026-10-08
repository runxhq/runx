import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const cratesRoot = path.join(workspaceRoot, "crates");

const expectedMembers = [
  "runx-assistant",
  "runx-cli",
  "runx-contracts",
  "runx-contracts-derive",
  "runx-core",
  "runx-js-worker",
  "runx-parser",
  "runx-receipts",
  "runx-runtime",
  "runx-sdk",
  "runx-x402",
];

const apiBearingPublishedCrates = new Set([
  "runx-contracts",
  "runx-core",
  "runx-parser",
  "runx-receipts",
  "runx-runtime",
  "runx-x402",
]);

const reservationVersionCrates = new Set([
  "runx-sdk",
]);

const publishableLibraryCrates = new Set([
  ...apiBearingPublishedCrates,
  ...reservationVersionCrates,
]);

const allowedRunxDeps = new Map([
  // The assistant is a host: it drives the runtime and never sits beneath it.
  ["runx-assistant", new Set(["runx-runtime", "runx-contracts"])],
  [
    "runx-cli",
    new Set([
      "runx-assistant",
      "runx-runtime",
      "runx-contracts",
      "runx-parser",
      "runx-receipts",
      "runx-x402",
    ]),
  ],
  ["runx-contracts", new Set(["runx-contracts-derive"])],
  ["runx-contracts-derive", new Set()],
  ["runx-core", new Set(["runx-contracts"])],
  ["runx-js-worker", new Set(["runx-contracts", "runx-parser"])],
  ["runx-parser", new Set(["runx-contracts", "runx-core"])],
  ["runx-receipts", new Set(["runx-contracts"])],
  ["runx-runtime", new Set(["runx-contracts", "runx-core", "runx-parser", "runx-receipts"])],
  ["runx-sdk", new Set(["runx-contracts"])],
  ["runx-x402", new Set(["runx-contracts"])],
]);

const requiredRunxDeps = new Map([
  ["runx-assistant", new Set(["runx-runtime", "runx-contracts"])],
  ["runx-core", new Set(["runx-contracts"])],
  ["runx-js-worker", new Set(["runx-contracts", "runx-parser"])],
  ["runx-parser", new Set(["runx-contracts", "runx-core"])],
  ["runx-receipts", new Set(["runx-contracts"])],
  ["runx-runtime", new Set(["runx-contracts", "runx-core", "runx-parser", "runx-receipts"])],
  ["runx-sdk", new Set(["runx-contracts"])],
  ["runx-x402", new Set(["runx-contracts"])],
]);

const pureCrateNames = new Set([
  "runx-contracts",
  "runx-core",
  "runx-parser",
  "runx-receipts",
  "runx-sdk",
  "runx-x402",
]);

const workspaceDisallowedDeps = [
  "async-std",
  "axum",
  "clap",
  "hyper",
  "reqwest",
  "rmcp",
  "serde_yaml",
  "serde_yml",
  "tokio",
  "ureq",
];

const pureCrateDisallowedDeps = [
  "async-std",
  "axum",
  "clap",
  "hyper",
  "reqwest",
  "rmcp",
  "serde_yaml",
  "serde_yml",
  "tokio",
  "ureq",
];

const runtimeDisallowedDeps = [
  "async-std",
  "axum",
  "clap",
  "serde_yaml",
  "serde_yml",
  "ureq",
];

const cliDisallowedDeps = [
  "async-std",
  "axum",
  "hyper",
  "reqwest",
  "rmcp",
  "serde_yaml",
  "serde_yml",
  "tokio",
  "ureq",
];

const findings = [];
const workspaceManifest = await readManifest("Cargo.toml");
const actualMembers = parseWorkspaceMembers(workspaceManifest);
const workspaceRunxVersions = parseWorkspaceRunxDependencyVersions(workspaceManifest);
const crateManifests = new Map();

checkMembers(actualMembers);
checkDisallowedDependencies("workspace", workspaceManifest);

for (const crateName of expectedMembers) {
  crateManifests.set(crateName, await readManifest(`${crateName}/Cargo.toml`));
}

checkNoCliVersionBleed(crateManifests, workspaceRunxVersions);

for (const crateName of expectedMembers) {
  const manifest = crateManifests.get(crateName);
  const packageName = parsePackageName(manifest);
  if (packageName !== crateName) {
    findings.push(`${crateName}/Cargo.toml package name is ${packageName ?? "missing"}, expected ${crateName}`);
  }
  checkWorkspaceDependencyVersion(crateName, manifest);
  checkPublishingReadiness(crateName, manifest);
  checkRunxDependencies(crateName, manifest);
  await checkRunxDependencyUsage(crateName, manifest);
  checkDisallowedDependencies(crateName, manifest);
  checkRuntimeAsyncHttpContract(crateName, manifest);
}

if (findings.length > 0) {
  console.error("Rust crate graph check failed:");
  for (const finding of findings) {
    console.error(`- ${finding}`);
  }
  process.exit(1);
}

console.log("Rust crate graph check passed.");

async function readManifest(relativePath) {
  return readFile(path.join(cratesRoot, relativePath), "utf8");
}

function parseWorkspaceMembers(manifest) {
  const body = sectionBody(manifest, "workspace");
  if (!body) {
    findings.push("crates/Cargo.toml is missing [workspace]");
    return [];
  }
  const membersMatch = /members\s*=\s*\[(?<body>.*?)\]/msu.exec(body);
  if (!membersMatch?.groups) {
    findings.push("crates/Cargo.toml is missing workspace members");
    return [];
  }
  return [...membersMatch.groups.body.matchAll(/"([^"]+)"/gu)].map((entry) => entry[1]).sort();
}

function parsePackageName(manifest) {
  const packageBody = sectionBody(manifest, "package");
  const match = /^name\s*=\s*"([^"]+)"/mu.exec(packageBody);
  return match?.[1];
}

function parsePackageVersion(manifest) {
  const packageBody = sectionBody(manifest, "package");
  const match = /^version\s*=\s*"([^"]+)"/mu.exec(packageBody);
  return match?.[1];
}

function parseWorkspaceRunxDependencyVersions(manifest) {
  const body = sectionBody(manifest, "workspace.dependencies");
  const versions = new Map();
  for (const match of body.matchAll(/^(runx-[A-Za-z0-9_-]+)\s*=\s*\{[^}]*version\s*=\s*"([^"]+)"/gmu)) {
    versions.set(match[1], match[2]);
  }
  return versions;
}

function checkMembers(actualMembers) {
  const expected = [...expectedMembers].sort();
  if (actualMembers.join("\n") !== expected.join("\n")) {
    findings.push(`workspace members are ${actualMembers.join(", ")}, expected ${expected.join(", ")}`);
  }
  if (actualMembers.includes("runx-authoring")) {
    findings.push("runx-authoring must not be an initial Rust crate");
  }
}

function checkWorkspaceDependencyVersion(crateName, manifest) {
  const workspaceVersion = workspaceRunxVersions.get(crateName);
  if (!workspaceVersion) {
    return;
  }
  const packageVersion = parsePackageVersion(manifest);
  if (workspaceVersion !== packageVersion) {
    findings.push(
      `workspace dependency ${crateName} version ${workspaceVersion} must match ${crateName}/Cargo.toml version ${packageVersion ?? "missing"}`,
    );
  }
}

function checkNoCliVersionBleed(manifests, workspaceVersions) {
  const cliVersion = parsePackageVersion(manifests.get("runx-cli") ?? "");
  if (!cliVersion) {
    return;
  }

  const stampedCrates = expectedMembers
    .filter((crateName) => crateName !== "runx-cli")
    .filter((crateName) => parsePackageVersion(manifests.get(crateName) ?? "") === cliVersion);
  const nonCliWorkspaceDeps = [...workspaceVersions.entries()].filter(([crateName]) => crateName !== "runx-cli");
  const stampedWorkspaceDeps = nonCliWorkspaceDeps
    .filter(([, version]) => version === cliVersion)
    .map(([crateName]) => crateName);

  if (stampedCrates.length === expectedMembers.length - 1 && stampedWorkspaceDeps.length === nonCliWorkspaceDeps.length) {
    findings.push(
      "CLI releases must not stamp the internal Rust crate graph. Only runx-cli may match the cli-v tag unless an explicit library-crate release is requested.",
    );
  }
}

function checkPublishingReadiness(crateName, manifest) {
  const packageBody = sectionBody(manifest, "package");
  const hasPublishFalse = /^publish\s*=\s*false\s*$/mu.test(packageBody);
  const version = parsePackageVersion(manifest);
  if (apiBearingPublishedCrates.has(crateName) && version === "0.0.1") {
    findings.push(`${crateName}/Cargo.toml must not reuse the published reservation version 0.0.1 for API-bearing publishability`);
  }
  if (reservationVersionCrates.has(crateName)) {
    if (version !== "0.0.1") {
      findings.push(`${crateName}/Cargo.toml must use placeholder reservation version 0.0.1, found ${version ?? "missing"}`);
    }
  }
  if (publishableLibraryCrates.has(crateName)) {
    if (hasPublishFalse) {
      findings.push(`${crateName}/Cargo.toml must remain publishable so the crates.io package can be reserved or updated`);
    }
  }
  if (crateName === "runx-cli" && !hasPublishFalse) {
    findings.push("runx-cli must remain unpublished until its internal Rust dependencies have a coordinated library release");
  }
  if (crateName === "runx-assistant" && !hasPublishFalse) {
    findings.push("runx-assistant is an application crate and must remain unpublished");
  }
}

function checkRunxDependencies(crateName, manifest) {
  const deps = parseDependencyNames(manifest).filter((dep) => dep.startsWith("runx-"));
  const allowed = allowedRunxDeps.get(crateName) ?? new Set();
  const required = requiredRunxDeps.get(crateName) ?? new Set();

  for (const dep of deps) {
    if (!allowed.has(dep)) {
      findings.push(`${crateName} must not depend on ${dep}`);
    }
  }
  for (const dep of required) {
    if (!deps.includes(dep)) {
      findings.push(`${crateName} must depend on ${dep}`);
    }
  }
}

async function checkRunxDependencyUsage(crateName, manifest) {
  if (crateName !== "runx-parser") {
    return;
  }
  const deps = parseDependencyNames(manifest).filter((dep) => dep.startsWith("runx-"));
  const source = await readCrateSource(crateName);
  for (const dep of deps) {
    const importName = dep.replaceAll("-", "_");
    if (!source.includes(importName)) {
      findings.push(`${crateName} declares ${dep} but does not use ${importName} in src/`);
    }
  }
}

async function readCrateSource(crateName) {
  const files = await collectRustFiles(path.join(cratesRoot, crateName, "src"));
  const contents = await Promise.all(files.map((filePath) => readFile(filePath, "utf8")));
  return contents.join("\n");
}

async function collectRustFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...await collectRustFiles(entryPath));
    } else if (entry.isFile() && entry.name.endsWith(".rs")) {
      files.push(entryPath);
    }
  }
  return files.sort();
}

function checkDisallowedDependencies(crateName, manifest) {
  const dependencyNames = parseDependencyNames(manifest);
  const disallowedDeps = disallowedDependenciesFor(crateName);
  for (const dep of disallowedDeps) {
    if (dependencyNames.includes(dep)) {
      findings.push(`${crateName} must not depend on ${dep}`);
    }
  }
}

function disallowedDependenciesFor(crateName) {
  if (crateName === "workspace") {
    return workspaceDisallowedDeps;
  }
  if (pureCrateNames.has(crateName)) {
    return pureCrateDisallowedDeps;
  }
  if (crateName === "runx-runtime") {
    return runtimeDisallowedDeps;
  }
  if (crateName === "runx-cli" || crateName === "runx-assistant") {
    return cliDisallowedDeps;
  }
  return pureCrateDisallowedDeps;
}

function checkRuntimeAsyncHttpContract(crateName, manifest) {
  if (crateName !== "runx-runtime") {
    return;
  }

  const featuresBody = sectionBody(manifest, "features");
  // With mcp, the MCP client's streamable HTTP leg rides the guarded reqwest
  // client (D25).
  if (!/^async-http\s*=\s*\["dep:reqwest", "dep:tokio", "dep:rustls", "dep:webpki-root-certs", "tokio\/rt-multi-thread", "rmcp\?\/transport-streamable-http-client-reqwest"\]\s*$/mu.test(featuresBody)) {
    findings.push("runx-runtime async-http feature must be exactly [\"dep:reqwest\", \"dep:tokio\", \"dep:rustls\", \"dep:webpki-root-certs\", \"tokio/rt-multi-thread\", \"rmcp?/transport-streamable-http-client-reqwest\"]");
  }
  if (!/^process-tool\s*=\s*\["async-http"\]\s*$/mu.test(featuresBody)) {
    findings.push("runx-runtime process-tool feature must imply async-http so the cargo CLI exercises reviewed HTTP");
  }
  // The MCP client speaks over the guarded HTTP client, so mcp implies it.
  if (!/^mcp\s*=\s*\["async-http", "dep:rmcp", "dep:tokio", "tokio\/process", "tokio\/io-util", "tokio\/sync", "tokio\/rt-multi-thread"\]\s*$/mu.test(featuresBody)) {
    findings.push("runx-runtime mcp feature must be exactly [\"async-http\", \"dep:rmcp\", \"dep:tokio\", \"tokio/process\", \"tokio/io-util\", \"tokio/sync\", \"tokio/rt-multi-thread\"]");
  }
  if (!/^mcp-http-server\s*=\s*\[\s*"mcp",\s*"rmcp\/transport-streamable-http-server",\s*"dep:bytes",\s*"dep:http",\s*"dep:http-body-util",\s*"dep:hyper",\s*"dep:hyper-util",\s*"dep:tower-service",\s*\]\s*$/mu.test(featuresBody)) {
    findings.push("runx-runtime mcp-http-server feature must be exactly [\"mcp\", \"rmcp/transport-streamable-http-server\", \"dep:bytes\", \"dep:http\", \"dep:http-body-util\", \"dep:hyper\", \"dep:hyper-util\", \"dep:tower-service\"]");
  }

  const reqwest = dependencyInlineSpec(manifest, "dependencies", "reqwest");
  if (!reqwest) {
    findings.push("runx-runtime must declare optional reqwest for the approved async-http edge");
  } else {
    if (!/version\s*=\s*"=[^"]+"/u.test(reqwest)) {
      findings.push("runx-runtime reqwest dependency must use an exact version pin");
    }
    if (!/default-features\s*=\s*false/u.test(reqwest)) {
      findings.push("runx-runtime reqwest dependency must disable default features");
    }
    if (!/optional\s*=\s*true/u.test(reqwest)) {
      findings.push("runx-runtime reqwest dependency must stay optional");
    }
    // reqwest drives rustls without bundling a crypto provider; the ring
    // provider is supplied by the explicit `rustls` dependency and installed at
    // client construction, avoiding the vendored aws-lc-sys C blob.
    for (const feature of ["rustls-no-provider", "json"]) {
      if (!dependencyInlineFeatures(reqwest).includes(feature)) {
        findings.push(`runx-runtime reqwest dependency must enable the ${feature} feature`);
      }
    }
    for (const forbiddenFeature of ["blocking", "cookies", "stream", "native-tls", "default-tls"]) {
      if (dependencyInlineFeatures(reqwest).includes(forbiddenFeature)) {
        findings.push(`runx-runtime reqwest dependency must not enable the ${forbiddenFeature} feature`);
      }
    }
  }

  const tokio = dependencyInlineSpec(manifest, "dependencies", "tokio");
  if (!tokio) {
    findings.push("runx-runtime must declare optional tokio for the approved async-http edge");
  } else {
    if (!/version\s*=\s*"=[^"]+"/u.test(tokio)) {
      findings.push("runx-runtime tokio dependency must use an exact version pin");
    }
    if (!/default-features\s*=\s*false/u.test(tokio)) {
      findings.push("runx-runtime tokio dependency must disable default features");
    }
    if (!/optional\s*=\s*true/u.test(tokio)) {
      findings.push("runx-runtime tokio dependency must stay optional");
    }
    const tokioFeatures = dependencyInlineFeatures(tokio);
    for (const feature of ["rt", "net", "time"]) {
      if (!tokioFeatures.includes(feature)) {
        findings.push(`runx-runtime tokio dependency must enable the ${feature} feature`);
      }
    }
    for (const forbiddenFeature of ["full", "macros", "process"]) {
      if (tokioFeatures.includes(forbiddenFeature)) {
        findings.push(`runx-runtime tokio dependency must not enable the ${forbiddenFeature} feature`);
      }
    }
  }

  const rmcp = dependencyInlineSpec(manifest, "dependencies", "rmcp");
  if (!rmcp) {
    findings.push("runx-runtime must declare optional rmcp for the approved MCP server and connector edge");
  } else {
    if (!/version\s*=\s*"=[^"]+"/u.test(rmcp)) {
      findings.push("runx-runtime rmcp dependency must use an exact version pin");
    }
    if (!/default-features\s*=\s*false/u.test(rmcp)) {
      findings.push("runx-runtime rmcp dependency must disable default features");
    }
    if (!/optional\s*=\s*true/u.test(rmcp)) {
      findings.push("runx-runtime rmcp dependency must stay optional");
    }
    const rmcpFeatures = dependencyInlineFeatures(rmcp);
    if (rmcpFeatures.join(",") !== "client,server") {
      findings.push("runx-runtime rmcp dependency must enable only the client and server features for the canonical MCP path");
    }
  }

  const hyper = dependencyInlineSpec(manifest, "dependencies", "hyper");
  if (!hyper) {
    findings.push("runx-runtime must declare optional hyper for the approved MCP HTTP server edge");
  } else {
    if (!/version\s*=\s*"=[^"]+"/u.test(hyper)) {
      findings.push("runx-runtime hyper dependency must use an exact version pin");
    }
    if (!/default-features\s*=\s*false/u.test(hyper)) {
      findings.push("runx-runtime hyper dependency must disable default features");
    }
    if (!/optional\s*=\s*true/u.test(hyper)) {
      findings.push("runx-runtime hyper dependency must stay optional");
    }
    const hyperFeatures = dependencyInlineFeatures(hyper);
    if (hyperFeatures.join(",") !== "http1,server") {
      findings.push("runx-runtime hyper dependency must enable only the http1 and server features for MCP HTTP");
    }
  }

  const hyperUtil = dependencyInlineSpec(manifest, "dependencies", "hyper-util");
  if (!hyperUtil) {
    findings.push("runx-runtime must declare optional hyper-util for the approved MCP HTTP server edge");
  } else {
    if (!/version\s*=\s*"=[^"]+"/u.test(hyperUtil)) {
      findings.push("runx-runtime hyper-util dependency must use an exact version pin");
    }
    if (!/default-features\s*=\s*false/u.test(hyperUtil)) {
      findings.push("runx-runtime hyper-util dependency must disable default features");
    }
    if (!/optional\s*=\s*true/u.test(hyperUtil)) {
      findings.push("runx-runtime hyper-util dependency must stay optional");
    }
    const hyperUtilFeatures = dependencyInlineFeatures(hyperUtil);
    if (hyperUtilFeatures.join(",") !== "service,tokio") {
      findings.push("runx-runtime hyper-util dependency must enable only the service and tokio features for MCP HTTP");
    }
  }
}

function dependencyInlineSpec(manifest, sectionName, dependencyName) {
  const body = sectionBody(manifest, sectionName);
  const pattern = new RegExp(`^${escapeRegExp(dependencyName)}\\s*=\\s*(?<spec>.*)$`, "mu");
  const match = pattern.exec(body);
  return match?.groups?.spec.trim() ?? null;
}

function dependencyInlineFeatures(spec) {
  const match = /features\s*=\s*\[(?<features>[^\]]*)\]/u.exec(spec);
  if (!match?.groups) {
    return [];
  }
  return [...match.groups.features.matchAll(/"([^"]+)"/gu)].map((entry) => entry[1]).sort();
}

function parseDependencyNames(manifest) {
  const names = new Set();
  for (const sectionName of ["dependencies", "dev-dependencies", "build-dependencies"]) {
    for (const name of dependencyNamesFromSection(sectionBody(manifest, sectionName))) {
      names.add(name);
    }
    for (const name of dependencyNamesFromSubtables(manifest, sectionName)) {
      names.add(name);
    }
  }
  return [...names].sort();
}

function dependencyNamesFromSection(body) {
  const names = [];
  for (const line of body.split("\n")) {
    const match = /^([A-Za-z0-9_-]+)(?:\.[A-Za-z0-9_-]+)?\s*=/u.exec(line.trim());
    if (match) {
      names.push(match[1]);
    }
    const packageMatch = /^package\s*=\s*"([^"]+)"/u.exec(line.trim());
    if (packageMatch) {
      names.push(packageMatch[1]);
    }
  }
  return names;
}

function dependencyNamesFromSubtables(manifest, sectionName) {
  const names = [];
  const headerPattern = new RegExp(`^\\[${escapeRegExp(sectionName)}\\.([A-Za-z0-9_-]+)\\]\\s*$`, "gmu");
  for (const match of manifest.matchAll(headerPattern)) {
    names.push(match[1]);
    const bodyStart = match.index + match[0].length;
    const nextSection = /^\[/mu.exec(manifest.slice(bodyStart));
    const body = nextSection ? manifest.slice(bodyStart, bodyStart + nextSection.index) : manifest.slice(bodyStart);
    const packageName = dependencyPackageNameFromTable(body);
    if (packageName) {
      names.push(packageName);
    }
  }
  return names;
}

function dependencyPackageNameFromTable(body) {
  for (const line of body.split("\n")) {
    const packageMatch = /^package\s*=\s*"([^"]+)"/u.exec(line.trim());
    if (packageMatch) {
      return packageMatch[1];
    }
  }
  return null;
}

function sectionBody(manifest, sectionName) {
  const pattern = new RegExp(`^\\[${escapeRegExp(sectionName)}\\]\\s*$`, "mu");
  const match = pattern.exec(manifest);
  if (!match) {
    return "";
  }
  const bodyStart = match.index + match[0].length;
  const nextSection = /^\[/mu.exec(manifest.slice(bodyStart));
  return nextSection ? manifest.slice(bodyStart, bodyStart + nextSection.index) : manifest.slice(bodyStart);
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}
