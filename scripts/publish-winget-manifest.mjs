import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";

const options = parseArgs(process.argv.slice(2));
const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN;
if (!token && !options.dryRun) {
  throw new Error("GH_TOKEN or GITHUB_TOKEN is required");
}

const manifestDir = wingetManifestDir(options.identifier, options.version);
const manifestFiles = loadManifestFiles(options.manifest, manifestDir);
const branchName = `${options.branchPrefix}-${options.version}`;

if (options.dryRun) {
  console.log(JSON.stringify({
    status: "dry-run",
    source: options.manifest,
    targetRepo: options.upstream,
    identifier: options.identifier,
    version: options.version,
    branch: branchName,
    manifestPaths: manifestFiles.map((file) => file.path),
  }, null, 2));
  process.exit(0);
}

const published = await readVersionFiles(options.upstream, options.base, manifestDir);
if (published.length > 0) {
  await assertExactVersion(published, manifestFiles, `${options.upstream} ${options.version}`);
  console.log(JSON.stringify({ status: "already-published", version: options.version }));
  process.exit(0);
}

const user = await api("GET", "/user");
const forkOwner = user.login;
await ensureFork(forkOwner);

const baseRef = await api("GET", `/repos/${options.upstream}/git/ref/heads/${options.base}`);
await ensureBranch(forkOwner, branchName, baseRef.object.sha);
const staged = await readVersionFiles(`${forkOwner}/winget-pkgs`, branchName, manifestDir);
if (staged.some((entry) => !manifestFiles.some((file) => file.path === entry.path))) {
  throw new Error(`winget branch ${branchName} contains unexpected files for ${options.version}`);
}
for (const file of manifestFiles) {
  await putFile(forkOwner, branchName, file.path, file.contents);
}
const pullRequest = await ensurePullRequest(forkOwner, branchName);

console.log(JSON.stringify({
  status: "published",
  fork: `${forkOwner}/winget-pkgs`,
  branch: branchName,
  manifestPaths: manifestFiles.map((file) => file.path),
  pullRequest: pullRequest.html_url,
}, null, 2));

async function ensureFork(owner) {
  const fork = await apiMaybe("GET", `/repos/${owner}/winget-pkgs`);
  if (fork?.full_name === `${owner}/winget-pkgs`) {
    return;
  }
  await api("POST", `/repos/${options.upstream}/forks`, {});
  for (let attempt = 1; attempt <= 30; attempt += 1) {
    const created = await apiMaybe("GET", `/repos/${owner}/winget-pkgs`);
    if (created?.full_name === `${owner}/winget-pkgs`) {
      return;
    }
    await sleep(2000);
  }
  throw new Error(`fork ${owner}/winget-pkgs was not ready after creation`);
}

async function ensureBranch(owner, branch, sha) {
  const encodedBranch = encodeURIComponent(branch);
  const ref = await apiMaybe("GET", `/repos/${owner}/winget-pkgs/git/ref/heads/${encodedBranch}`);
  if (ref) {
    return;
  }
  await api("POST", `/repos/${owner}/winget-pkgs/git/refs`, {
    ref: `refs/heads/${branch}`,
    sha,
  });
}

async function readVersionFiles(repository, branch, dir) {
  const entries = await apiMaybe(
    "GET",
    `/repos/${repository}/contents/${encodePath(dir)}?ref=${encodeURIComponent(branch)}`,
  );
  if (entries === null) return [];
  if (!Array.isArray(entries) || entries.some((entry) => entry.type !== "file")) {
    throw new Error(`${repository} ${dir} is not a flat manifest directory`);
  }
  return entries;
}

async function assertExactVersion(existing, expected, label) {
  if (existing.length !== expected.length) {
    throw new Error(`${label} has a different set of manifest files`);
  }
  for (const file of expected) {
    const entry = existing.find((candidate) => candidate.path === file.path);
    if (!entry || await readFileContents(entry.url) !== file.contents) {
      throw new Error(`${label} differs at ${file.path}; never replace a published version`);
    }
  }
}

async function readFileContents(url) {
  const file = await api("GET", new URL(url).pathname);
  if (file.encoding !== "base64" || typeof file.content !== "string") {
    throw new Error(`winget manifest response lacks base64 content: ${url}`);
  }
  return Buffer.from(file.content.replace(/\s/gu, ""), "base64").toString("utf8");
}

async function putFile(owner, branch, filePath, contents) {
  const existing = await apiMaybe(
    "GET",
    `/repos/${owner}/winget-pkgs/contents/${encodePath(filePath)}?ref=${encodeURIComponent(branch)}`,
  );
  if (existing) {
    if (await readFileContents(existing.url) !== contents) {
      throw new Error(`winget branch ${branch} has different content for ${filePath}`);
    }
    return;
  }
  await api("PUT", `/repos/${owner}/winget-pkgs/contents/${encodePath(filePath)}`, {
    message: `Update ${options.identifier} ${options.version}`,
    content: Buffer.from(contents, "utf8").toString("base64"),
    branch,
  });
}

async function ensurePullRequest(owner, branch) {
  const head = `${owner}:${branch}`;
  const existing = await api(
    "GET",
    `/repos/${options.upstream}/pulls?head=${encodeURIComponent(head)}&state=all`,
  );
  if (existing.length > 0) {
    if (existing[0].state !== "open") {
      throw new Error(`winget submission for ${options.version} was closed; do not silently recreate it`);
    }
    return existing[0];
  }
  return api("POST", `/repos/${options.upstream}/pulls`, {
    title: `${options.identifier} ${options.version}`,
    head,
    base: options.base,
    body: [
      `Updates ${options.identifier} to ${options.version}.`,
      "",
      "Generated by the runx release workflow from the validated channel manifest.",
    ].join("\n"),
  });
}

function wingetManifestDir(identifier, version) {
  const parts = identifier.split(".");
  if (parts.length < 2 || parts.some((part) => !part)) {
    throw new Error(`invalid winget package identifier: ${identifier}`);
  }
  const [publisher, packageName] = parts;
  return [
    "manifests",
    publisher[0].toLowerCase(),
    publisher,
    packageName,
    version,
  ].join("/");
}

function loadManifestFiles(source, targetDir) {
  const sourceStat = statSync(source);
  if (sourceStat.isDirectory()) {
    return readdirSync(source)
      .filter((entry) => entry.endsWith(".yaml"))
      .sort()
      .map((entry) => ({
        path: `${targetDir}/${entry}`,
        contents: readFileSync(path.join(source, entry), "utf8"),
      }));
  }
  if (sourceStat.isFile()) {
    return [{
      path: `${targetDir}/${path.basename(source)}`,
      contents: readFileSync(source, "utf8"),
    }];
  }
  throw new Error(`--manifest must be a YAML file or directory: ${source}`);
}

async function api(method, apiPath, body) {
  const response = await fetch(`https://api.github.com${apiPath}`, {
    method,
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      "Content-Type": "application/json",
      "X-GitHub-Api-Version": "2022-11-28",
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok) {
    const text = await response.text();
    throw new Error(`${method} ${apiPath} failed (${response.status}): ${text}`);
  }
  if (response.status === 204) {
    return null;
  }
  return response.json();
}

async function apiMaybe(method, apiPath) {
  try {
    return await api(method, apiPath);
  } catch (error) {
    if (String(error).includes("failed (404):")) {
      return null;
    }
    throw error;
  }
}

function encodePath(value) {
  return value.split("/").map(encodeURIComponent).join("/");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function parseArgs(argv) {
  const parsed = {
    manifest: "",
    version: "",
    identifier: "runxhq.runx",
    upstream: "microsoft/winget-pkgs",
    base: "master",
    branchPrefix: "runxhq-runx",
    dryRun: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--manifest") {
      parsed.manifest = argv[++index] ?? "";
      continue;
    }
    if (arg === "--version") {
      parsed.version = argv[++index] ?? "";
      continue;
    }
    if (arg === "--identifier") {
      parsed.identifier = argv[++index] ?? "";
      continue;
    }
    if (arg === "--upstream") {
      parsed.upstream = argv[++index] ?? "";
      continue;
    }
    if (arg === "--base") {
      parsed.base = argv[++index] ?? "";
      continue;
    }
    if (arg === "--branch-prefix") {
      parsed.branchPrefix = argv[++index] ?? "";
      continue;
    }
    if (arg === "--dry-run") {
      parsed.dryRun = true;
      continue;
    }
    throw new Error(`unknown argument: ${arg}`);
  }
  for (const key of ["manifest", "version", "identifier", "upstream", "base", "branchPrefix"]) {
    if (!parsed[key]) {
      throw new Error(`--${key.replace(/[A-Z]/gu, (letter) => `-${letter.toLowerCase()}`)} is required`);
    }
  }
  return parsed;
}
