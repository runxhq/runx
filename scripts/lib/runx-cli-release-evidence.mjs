import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { loadRustCliPlatforms } from "../rust-cli-topology.mjs";

const REPOSITORY = "runxhq/runx";
const GHCR_IMAGE = "ghcr.io/runxhq/runx";
const HOMEBREW_FORMULA_URL =
  "https://raw.githubusercontent.com/runxhq/homebrew-tap/main/Formula/runx.rb";
const SCOOP_MANIFEST_URL =
  "https://raw.githubusercontent.com/runxhq/scoop-bucket/main/bucket/runx.json";

const ossRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const cliManifest = JSON.parse(readFileSync(path.join(ossRoot, "packages/cli/package.json"), "utf8"));
export const RUNX_CLI_NPM_PACKAGES = Object.freeze([
  cliManifest.name,
  ...loadRustCliPlatforms(ossRoot).map((platform) => platform.package),
]);

export const RUNX_CLI_REQUIRED_RELEASE_CHECKS = Object.freeze([
  "github_release",
  "github_tag",
  "release_workflow",
  "npm",
  "ghcr",
  "homebrew",
  "scoop",
]);

export const RUNX_CLI_REQUIRED_RELEASE_CHANNELS = Object.freeze([
  "github_release",
  "npm",
  "ghcr",
  "homebrew",
  "scoop",
]);

export const RUNX_CLI_RELEASE_NOTE_SECTIONS = Object.freeze([
  "Highlights",
  "Added",
  "Changed",
  "Fixed",
  "Removed",
  "Security",
  "Breaking changes",
  "Upgrade guidance",
  "Contributors",
]);

export const RUNX_CLI_REQUIRED_CANDIDATE_CHECKS = Object.freeze([
  // CI owns the affected platform checks and secret scan behind this aggregate.
  "checks",
]);

const GITHUB_ACTIONS_SKIP_MARKERS = Object.freeze([
  "[skip ci]",
  "[ci skip]",
  "[no ci]",
  "[skip actions]",
  "[actions skip]",
]);

export function githubActionsSkipDirective(commitMessage) {
  if (typeof commitMessage !== "string") {
    throw new Error("commit message must be a string");
  }
  const normalized = commitMessage.toLowerCase();
  const marker = GITHUB_ACTIONS_SKIP_MARKERS.find((candidate) =>
    normalized.includes(candidate)
  );
  if (marker) return marker;
  return /^skip-checks:\s*true\s*$/imu.test(commitMessage) ? "skip-checks: true" : "";
}

/**
 * @param {{ body: string; version: string; previousTag: string }} options
 */
export function validateRunxCliReleaseNotes({ body, version, previousTag }) {
  assertStableVersion(version);
  if (typeof body !== "string") {
    throw new Error("release notes body must be a string");
  }
  if (!/^cli-v\d+\.\d+\.\d+$/u.test(previousTag)) {
    throw new Error(`invalid previous CLI tag: ${previousTag}`);
  }

  const title = `# Runx CLI ${version}`;
  const summary = body
    .slice(title.length)
    .split(/^## /mu, 1)[0]
    .trim();
  const checks = [
    check("release_notes_title", body.startsWith(`${title}\n`), `expected title '${title}'`),
    check(
      "release_notes_summary",
      summary.length > 0,
      summary.length > 0 ? "release summary is present" : "release summary is missing",
    ),
    ...RUNX_CLI_RELEASE_NOTE_SECTIONS.map((section) => {
      const content = releaseNoteSection(body, section);
      return check(
        `release_notes_${section.toLowerCase().replaceAll(" ", "_")}`,
        content.length > 0,
        content.length > 0 ? `${section} is present` : `${section} is missing or empty`,
      );
    }),
    check(
      "release_notes_compare_link",
      body.includes(
        `https://github.com/${REPOSITORY}/compare/${previousTag}...cli-v${version}`,
      ),
      `expected full changelog link from ${previousTag} to cli-v${version}`,
    ),
    check(
      "release_notes_no_placeholders",
      !/\b(?:TBD|TODO|PLACEHOLDER|COMING SOON)\b/iu.test(body),
      "release notes contain no placeholder text",
    ),
  ];

  return {
    ready: checks.every((entry) => entry.status === "passed"),
    version,
    previousTag,
    checks,
  };
}

/**
 * @param {{
 *   version: string;
 *   expectedCommit?: string;
 *   fetchImpl?: typeof fetch;
 *   githubToken?: string;
 *   ghcrTag?: string;
 *   ghcrDigest?: string;
 * }} options
 */
export async function observeRunxCliRelease(options) {
  const {
    version,
    expectedCommit,
    fetchImpl = globalThis.fetch,
    githubToken = process.env.GH_TOKEN ?? process.env.GITHUB_TOKEN,
    ghcrTag = version,
    ghcrDigest,
  } = options;
  assertStableVersion(version);
  if (typeof fetchImpl !== "function") {
    throw new Error("release evidence requires a fetch implementation");
  }

  const tag = `cli-v${version}`;
  const githubHeaders = {
    accept: "application/vnd.github+json",
    ...(githubToken ? { authorization: `Bearer ${githubToken}` } : {}),
  };
  const [release, gitTag, workflow, npm, ghcr, homebrew, scoop] = await Promise.all([
    observeGitHubRelease({ version, tag, fetchImpl, githubHeaders }),
    observeGitTag({ tag, expectedCommit, fetchImpl, githubHeaders }),
    observeReleaseWorkflow({ tag, expectedCommit, fetchImpl, githubHeaders }),
    observeNpm({ version, fetchImpl }),
    observeGhcr({ version, tag: ghcrTag, expectedDigest: ghcrDigest, fetchImpl }),
    observeHomebrew({ version, tag, fetchImpl }),
    observeScoop({ version, tag, fetchImpl }),
  ]);
  if (
    gitTag.check.status === "passed"
    && workflow.check.status === "passed"
    && gitTag.commitRef !== workflow.commitRef
  ) {
    workflow.check = check(
      "release_workflow",
      false,
      `${tag} workflow ran at ${workflow.commitRef}, but the tag resolves to ${gitTag.commitRef}`,
    );
  }
  const observations = [release, gitTag, workflow, npm, ghcr, homebrew, scoop];
  const checks = observations.map(({ check }) => check);

  return {
    ready: checks.every((check) => check.status === "passed"),
    version,
    tag,
    commitRef: gitTag.commitRef,
    publishedAt: release.publishedAt,
    releaseUrl: release.url,
    workflowUrl: workflow.url,
    checks,
    missing: checks
      .filter((check) => check.status === "failed")
      .map((check) => `${check.id}: ${check.detail}`),
    locators: [
      release.url,
      workflow.url,
      `https://www.npmjs.com/package/%40runxhq%2Fcli/v/${version}`,
      "https://github.com/orgs/runxhq/packages/container/package/runx",
      HOMEBREW_FORMULA_URL,
      SCOOP_MANIFEST_URL,
    ].filter(Boolean),
  };
}

/**
 * Prove that the exact candidate commit passed the repository's required
 * pre-release checks. A green branch is not sufficient: releases bind to one
 * immutable commit, so the evidence must do the same.
 *
 * @param {{
 *   commit: string;
 *   fetchImpl?: typeof fetch;
 *   githubToken?: string;
 * }} options
 */
export async function observeRunxCliCandidateChecks({
  commit,
  fetchImpl = globalThis.fetch,
  githubToken = process.env.GH_TOKEN ?? process.env.GITHUB_TOKEN,
}) {
  if (!/^[0-9a-f]{40}$/u.test(commit ?? "")) {
    throw new Error(`invalid candidate commit: ${commit ?? "<unset>"}`);
  }
  if (typeof fetchImpl !== "function") {
    throw new Error("candidate evidence requires a fetch implementation");
  }

  const endpoint =
    `https://api.github.com/repos/${REPOSITORY}/commits/${commit}/check-runs`
    + "?filter=latest&per_page=100";
  const headers = {
    accept: "application/vnd.github+json",
    ...(githubToken ? { authorization: `Bearer ${githubToken}` } : {}),
  };

  try {
    const response = await fetchImpl(endpoint, { headers });
    if (!response.ok) {
      const detail = `candidate check lookup returned HTTP ${response.status}`;
      return candidateCheckEvidence(commit, [], detail);
    }
    const body = await response.json();
    const runs = Array.isArray(body.check_runs) ? body.check_runs : [];
    return candidateCheckEvidence(commit, runs);
  } catch (error) {
    return candidateCheckEvidence(commit, [], errorMessage(error));
  }
}

/**
 * @param {{ fetchImpl?: typeof fetch; version?: string; tag?: string; expectedDigest?: string }} [options]
 */
export async function checkRunxGhcrAnonymousAccess({
  fetchImpl = globalThis.fetch,
  version,
  tag = version,
  expectedDigest,
} = {}) {
  if (typeof fetchImpl !== "function") {
    throw new Error("GHCR access check requires a fetch implementation");
  }
  if (version) assertStableVersion(version);
  if (tag && !/^[a-zA-Z0-9_][a-zA-Z0-9_.-]{0,127}$/u.test(tag)) {
    throw new Error("GHCR tag is invalid");
  }
  if (expectedDigest && !/^sha256:[0-9a-f]{64}$/u.test(expectedDigest)) {
    throw new Error("GHCR expected digest is invalid");
  }
  const access = await anonymousGhcrToken(fetchImpl);
  const id = tag ? "ghcr" : "ghcr_anonymous_access";
  if (!access.token) return check(id, false, access.detail);
  if (!tag) return check(id, true, "GHCR grants anonymous pull access");
  try {
    const manifestResponse = await fetchImpl(
      `https://ghcr.io/v2/runxhq/runx/manifests/${tag}`,
      {
        headers: {
          accept: [
            "application/vnd.oci.image.index.v1+json",
            "application/vnd.docker.distribution.manifest.list.v2+json",
            "application/vnd.oci.image.manifest.v1+json",
          ].join(", "),
          authorization: `Bearer ${access.token}`,
        },
      },
    );
    const observedDigest = manifestResponse.headers.get("docker-content-digest");
    const passed = manifestResponse.ok && (!expectedDigest || observedDigest === expectedDigest);
    return check(id, passed, manifestResponse.ok
      ? expectedDigest
        ? `${GHCR_IMAGE}:${tag} digest ${observedDigest ?? "<missing>"}, expected ${expectedDigest}`
        : `${GHCR_IMAGE}:${tag} is anonymously pullable`
      : `${GHCR_IMAGE}:${tag} manifest returned HTTP ${manifestResponse.status}`);
  } catch (error) {
    return check(id, false, errorMessage(error));
  }
}

function releaseNoteSection(body, heading) {
  const marker = `## ${heading}\n`;
  const headingIndex = body.indexOf(marker);
  if (headingIndex < 0) return "";
  const contentIndex = headingIndex + marker.length;
  const nextHeadingIndex = body.indexOf("\n## ", contentIndex);
  return body
    .slice(contentIndex, nextHeadingIndex < 0 ? body.length : nextHeadingIndex)
    .trim();
}

function candidateCheckEvidence(commit, runs, lookupFailure = "") {
  const checks = RUNX_CLI_REQUIRED_CANDIDATE_CHECKS.map((name) => {
    const matching = runs.filter((run) => stringField(run?.name) === name);
    const passed = matching.length > 0 && matching.every((run) =>
      stringField(run?.status) === "completed"
      && stringField(run?.conclusion) === "success"
    );
    const observed = matching
      .map((run) =>
        `${stringField(run?.status) || "unknown"}/${stringField(run?.conclusion) || "unknown"}`
      )
      .join(", ");
    return check(
      `candidate_${name}`,
      passed,
      passed
        ? `${name} passed for ${commit}`
        : `${name} ${lookupFailure || observed || "is missing"} for ${commit}`,
    );
  });
  return {
    ready: checks.every((entry) => entry.status === "passed"),
    commitRef: commit,
    checks,
    missing: checks
      .filter((entry) => entry.status === "failed")
      .map((entry) => `${entry.id}: ${entry.detail}`),
  };
}

async function observeGitHubRelease({ version, tag, fetchImpl, githubHeaders }) {
  const url = `https://api.github.com/repos/${REPOSITORY}/releases/tags/${tag}`;
  try {
    const response = await fetchImpl(url, { headers: githubHeaders });
    if (!response.ok) {
      return failedObservation("github_release", `${tag} returned HTTP ${response.status}`);
    }
    const body = await response.json();
    const assets = Array.isArray(body.assets)
      ? body.assets.map((asset) => stringField(asset?.name)).filter(Boolean)
      : [];
    const missingAssets = requiredReleaseAssets(version).filter((asset) => !assets.includes(asset));
    const releaseUrl = stringField(body.html_url);
    const publishedAt = stringField(body.published_at);
    const valid = stringField(body.tag_name) === tag
      && body.draft === false
      && body.prerelease === false
      && Boolean(releaseUrl)
      && Boolean(publishedAt)
      && missingAssets.length === 0;
    return {
      check: check(
        "github_release",
        valid,
        valid
          ? `${tag} is public with every required artifact`
          : `invalid metadata or missing artifacts: ${missingAssets.join(", ") || "release metadata"}`,
      ),
      url: releaseUrl,
      publishedAt,
    };
  } catch (error) {
    return failedObservation("github_release", errorMessage(error));
  }
}

async function observeGitTag({ tag, expectedCommit, fetchImpl, githubHeaders }) {
  const endpoint = `https://api.github.com/repos/${REPOSITORY}/git/ref/tags/${tag}`;
  try {
    const response = await fetchImpl(endpoint, { headers: githubHeaders });
    if (!response.ok) {
      return failedObservation("github_tag", `${tag} returned HTTP ${response.status}`);
    }
    const body = await response.json();
    const commitRef = await peelGitObject({
      object: body?.object,
      fetchImpl,
      githubHeaders,
    });
    const valid = Boolean(commitRef) && (!expectedCommit || commitRef === expectedCommit);
    return {
      check: check(
        "github_tag",
        valid,
        valid
          ? `${tag} resolves to ${commitRef}`
          : `${tag} resolves to ${commitRef || "<unset>"}`
            + `${expectedCommit ? `, expected ${expectedCommit}` : ""}`,
      ),
      commitRef,
    };
  } catch (error) {
    return failedObservation("github_tag", errorMessage(error));
  }
}

async function peelGitObject({ object, fetchImpl, githubHeaders }) {
  let type = stringField(object?.type);
  let sha = stringField(object?.sha);
  for (let depth = 0; depth < 5 && type === "tag" && sha; depth += 1) {
    const response = await fetchImpl(
      `https://api.github.com/repos/${REPOSITORY}/git/tags/${sha}`,
      { headers: githubHeaders },
    );
    if (!response.ok) return "";
    const body = await response.json();
    type = stringField(body?.object?.type);
    sha = stringField(body?.object?.sha);
  }
  return type === "commit" && /^[0-9a-f]{40}$/u.test(sha) ? sha : "";
}

async function observeReleaseWorkflow({ tag, expectedCommit, fetchImpl, githubHeaders }) {
  const query = new URLSearchParams({
    event: "push",
    branch: tag,
    per_page: "10",
  });
  const endpoint =
    `https://api.github.com/repos/${REPOSITORY}/actions/workflows/release.yml/runs?${query}`;
  try {
    const response = await fetchImpl(endpoint, { headers: githubHeaders });
    if (!response.ok) {
      return failedObservation(
        "release_workflow",
        `${tag} workflow lookup returned HTTP ${response.status}`,
      );
    }
    const body = await response.json();
    const runs = Array.isArray(body.workflow_runs) ? body.workflow_runs : [];
    const run = runs.find((candidate) => stringField(candidate?.head_branch) === tag);
    const commitRef = stringField(run?.head_sha);
    const url = stringField(run?.html_url);
    const conclusion = stringField(run?.conclusion);
    const status = stringField(run?.status);
    const valid = status === "completed"
      && conclusion === "success"
      && Boolean(commitRef)
      && Boolean(url)
      && (!expectedCommit || commitRef === expectedCommit);
    return {
      check: check(
        "release_workflow",
        valid,
        valid
          ? `${tag} release workflow passed for ${commitRef}`
          : `${tag} workflow is ${status || "missing"}/${conclusion || "unknown"}`
            + `${expectedCommit && commitRef !== expectedCommit
              ? ` at ${commitRef || "<unset>"}, expected ${expectedCommit}`
              : ""}`,
      ),
      commitRef,
      url,
    };
  } catch (error) {
    return failedObservation("release_workflow", errorMessage(error));
  }
}

async function observeNpm({ version, fetchImpl }) {
  const mismatches = [];
  await Promise.all(RUNX_CLI_NPM_PACKAGES.map(async (packageName) => {
    try {
      const response = await fetchImpl(
        `https://registry.npmjs.org/${encodeURIComponent(packageName)}`,
        { headers: { accept: "application/json" } },
      );
      if (!response.ok) {
        mismatches.push(`${packageName} HTTP ${response.status}`);
        return;
      }
      const body = await response.json();
      const published = body?.versions?.[version]?.version;
      const latest = body?.["dist-tags"]?.latest;
      if (published !== version || latest !== version) {
        mismatches.push(
          `${packageName} published=${published ?? "<unset>"} latest=${latest ?? "<unset>"}`,
        );
      }
    } catch (error) {
      mismatches.push(`${packageName} ${errorMessage(error)}`);
    }
  }));
  mismatches.sort();
  return {
    check: check(
      "npm",
      mismatches.length === 0,
      mismatches.length === 0
        ? `all ${RUNX_CLI_NPM_PACKAGES.length} CLI packages publish and select ${version}`
        : mismatches.join("; "),
    ),
  };
}

async function observeGhcr({ version, tag, expectedDigest, fetchImpl }) {
  return {
    check: await checkRunxGhcrAnonymousAccess({ version, tag, expectedDigest, fetchImpl }),
  };
}

async function anonymousGhcrToken(fetchImpl) {
  try {
    const tokenUrl = new URL("https://ghcr.io/token");
    tokenUrl.searchParams.set("service", "ghcr.io");
    tokenUrl.searchParams.set("scope", "repository:runxhq/runx:pull");
    const response = await fetchImpl(tokenUrl, {
      headers: { accept: "application/json" },
    });
    if (!response.ok) {
      return {
        token: "",
        detail: `anonymous pull token returned HTTP ${response.status}`,
      };
    }
    const body = await response.json();
    const token = stringField(body.token) || stringField(body.access_token);
    return {
      token,
      detail: token ? "" : "anonymous pull token was missing",
    };
  } catch (error) {
    return { token: "", detail: errorMessage(error) };
  }
}

async function observeHomebrew({ version, tag, fetchImpl }) {
  try {
    const response = await fetchImpl(HOMEBREW_FORMULA_URL);
    const source = response.ok ? await response.text() : "";
    const formulaVersion = source.match(/^\s*version\s+"([^"]+)"\s*$/mu)?.[1];
    const valid = response.ok
      && formulaVersion === version
      && source.includes(`/releases/download/${tag}/runx-${version}-`);
    return {
      check: check(
        "homebrew",
        valid,
        valid
          ? `Homebrew formula selects ${version}`
          : `Homebrew formula version=${formulaVersion || "<unset>"} HTTP ${response.status}`,
      ),
    };
  } catch (error) {
    return failedObservation("homebrew", errorMessage(error));
  }
}

async function observeScoop({ version, tag, fetchImpl }) {
  try {
    const response = await fetchImpl(SCOOP_MANIFEST_URL);
    const body = response.ok ? await response.json() : {};
    const serialized = JSON.stringify(body);
    const valid = response.ok
      && stringField(body.version) === version
      && serialized.includes(`/releases/download/${tag}/runx-${version}-`);
    return {
      check: check(
        "scoop",
        valid,
        valid
          ? `Scoop manifest selects ${version}`
          : `Scoop manifest version=${stringField(body.version) || "<unset>"} HTTP ${response.status}`,
      ),
    };
  } catch (error) {
    return failedObservation("scoop", errorMessage(error));
  }
}

function requiredReleaseAssets(version) {
  return [
    "channel-manifests.tar.gz",
    "checksums.txt",
    "install",
    "install.ps1",
    `runx-${version}-sbom.cdx.json`,
    `runx-${version}-aarch64-apple-darwin.tar.gz`,
    `runx-${version}-aarch64-unknown-linux-musl.tar.gz`,
    `runx-${version}-x86_64-apple-darwin.tar.gz`,
    `runx-${version}-x86_64-pc-windows-msvc.zip`,
    `runx-${version}-x86_64-unknown-linux-musl.tar.gz`,
  ];
}

function failedObservation(id, detail) {
  return { check: check(id, false, detail), url: "", publishedAt: "", commitRef: "" };
}

function check(id, passed, detail) {
  return { id, status: passed ? "passed" : "failed", detail };
}

function assertStableVersion(version) {
  if (!/^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/u.test(version ?? "")) {
    throw new Error(`invalid stable CLI version: ${version ?? "<unset>"}`);
  }
}

function stringField(value) {
  return typeof value === "string" ? value.trim() : "";
}

function errorMessage(error) {
  return error instanceof Error ? error.message : String(error);
}
