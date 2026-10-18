import { describe, expect, it } from "vitest";

import {
  RUNX_CLI_NPM_PACKAGES,
  RUNX_CLI_RELEASE_NOTE_SECTIONS,
  RUNX_CLI_REQUIRED_RELEASE_CHANNELS,
  RUNX_CLI_REQUIRED_RELEASE_CHECKS,
  githubActionsSkipDirective,
  observeRunxCliCandidateChecks,
  observeRunxCliRelease,
  validateRunxCliReleaseNotes,
} from "../scripts/lib/runx-cli-release-evidence.mjs";

const version = "9.5.0";
const tag = `cli-v${version}`;
const commit = "a".repeat(40);

describe("Runx CLI release evidence", () => {
  it("recognizes every GitHub Actions skip directive that suppresses tag pushes", () => {
    expect(githubActionsSkipDirective("docs: update [skip ci]")).toBe("[skip ci]");
    expect(githubActionsSkipDirective("docs: update [CI SKIP]")).toBe("[ci skip]");
    expect(githubActionsSkipDirective("docs: update [no ci]")).toBe("[no ci]");
    expect(githubActionsSkipDirective("docs: update [skip actions]")).toBe("[skip actions]");
    expect(githubActionsSkipDirective("docs: update [actions skip]")).toBe("[actions skip]");
    expect(githubActionsSkipDirective("docs: update\n\nskip-checks:true")).toBe(
      "skip-checks: true",
    );
    expect(githubActionsSkipDirective("docs: update\n\nskip-checks: true")).toBe(
      "skip-checks: true",
    );
    expect(githubActionsSkipDirective("docs: update\n\nskip-checks: false")).toBe("");
    expect(githubActionsSkipDirective("fix(release): publish 0.8.1")).toBe("");
  });

  it("requires independent evidence for every public release channel", async () => {
    const evidence = await observeRunxCliRelease({
      version,
      expectedCommit: commit,
      fetchImpl: releaseFetch(),
      githubToken: "",
    });

    expect(evidence.ready).toBe(true);
    expect(evidence.commitRef).toBe(commit);
    expect(evidence.checks.map((check: { id: string }) => check.id))
      .toEqual(RUNX_CLI_REQUIRED_RELEASE_CHECKS);
    expect(RUNX_CLI_REQUIRED_RELEASE_CHANNELS).toEqual([
      "github_release",
      "npm",
      "ghcr",
      "homebrew",
      "scoop",
      "aur",
    ]);
    expect(evidence.checks.every((check: { status: string }) => check.status === "passed"))
      .toBe(true);
  });

  it("scopes authenticated readback to GitHub API requests", async () => {
    const requests: Array<{ url: string; authorization: string | null }> = [];
    const providerFetch = releaseFetch();
    const fetchImpl: typeof fetch = async (input, init) => {
      requests.push({
        url: String(input),
        authorization: new Headers(init?.headers).get("authorization"),
      });
      return providerFetch(input, init);
    };

    const evidence = await observeRunxCliRelease({
      version,
      expectedCommit: commit,
      fetchImpl,
      githubToken: "github-readback-token",
    });

    expect(evidence.ready).toBe(true);
    const githubRequests = requests.filter(({ url }) => url.startsWith("https://api.github.com/"));
    expect(githubRequests).toHaveLength(3);
    expect(githubRequests.every(({ authorization }) =>
      authorization === "Bearer github-readback-token"
    )).toBe(true);
    expect(requests
      .filter(({ url }) => !url.startsWith("https://api.github.com/"))
      .every(({ authorization }) => authorization !== "Bearer github-readback-token")).toBe(true);
  });

  it("binds release readiness to the CI aggregate on the exact commit", async () => {
    const requests: Array<{ url: string; authorization: string | null }> = [];
    const evidence = await observeRunxCliCandidateChecks({
      commit,
      githubToken: "candidate-readback-token",
      fetchImpl: async (input, init) => {
        requests.push({
          url: String(input),
          authorization: new Headers(init?.headers).get("authorization"),
        });
        return jsonResponse({
          check_runs: [
            { name: "checks", status: "completed", conclusion: "success" },
            { name: "Dependabot", status: "completed", conclusion: "failure" },
          ],
        });
      },
    });

    expect(evidence.ready).toBe(true);
    expect(evidence.commitRef).toBe(commit);
    expect(evidence.checks.map((check: { id: string }) => check.id))
      .toEqual(["candidate_checks"]);
    expect(requests).toEqual([{
      url:
        `https://api.github.com/repos/runxhq/runx/commits/${commit}/check-runs`
        + "?filter=latest&per_page=100",
      authorization: "Bearer candidate-readback-token",
    }]);
  });

  it("refuses missing, pending, or failed exact-commit checks", async () => {
    for (const checkRuns of [
      [],
      [{ name: "classify", status: "completed", conclusion: "success" }],
      [{ name: "checks", status: "completed", conclusion: "failure" }],
      [{ name: "checks", status: "completed", conclusion: "skipped" }],
      [
        { name: "checks", status: "completed", conclusion: "success" },
        { name: "checks", status: "in_progress", conclusion: null },
      ],
    ]) {
      const evidence = await observeRunxCliCandidateChecks({
        commit,
        githubToken: "",
        fetchImpl: async () => jsonResponse({ check_runs: checkRuns }),
      });

      expect(evidence.ready).toBe(false);
      expect(failedIds(evidence)).toEqual(["candidate_checks"]);
    }
  });

  it("refuses workflow, npm-latest, anonymous-container, and AUR drift", async () => {
    const evidence = await observeRunxCliRelease({
      version,
      expectedCommit: commit,
      fetchImpl: releaseFetch({
        workflowConclusion: "failure",
        npmLatest: "9.4.0",
        ghcrTokenStatus: 401,
        aurVersion: "9.4.0-1",
      }),
      githubToken: "",
    });

    expect(evidence.ready).toBe(false);
    expect(failedIds(evidence)).toEqual(["release_workflow", "npm", "ghcr", "aur"]);
  });

  it("binds a repaired image tag to its exact public digest", async () => {
    const digest = `sha256:${"f".repeat(64)}`;
    const providerFetch = releaseFetch();
    const fetchImpl: typeof fetch = (input, init) => String(input).includes(`/manifests/${version}-r1`)
      ? Promise.resolve(new Response(JSON.stringify({ schemaVersion: 2 }), {
        headers: { "docker-content-digest": digest },
      }))
      : providerFetch(input, init);
    const verified = await observeRunxCliRelease({
      version,
      ghcrTag: `${version}-r1`,
      ghcrDigest: digest,
      fetchImpl,
      githubToken: "",
    });
    expect(verified.ready).toBe(true);

    const drifted = await observeRunxCliRelease({
      version,
      ghcrTag: `${version}-r1`,
      ghcrDigest: `sha256:${"e".repeat(64)}`,
      fetchImpl,
      githubToken: "",
    });
    expect(failedIds(drifted)).toEqual(["ghcr"]);
  });

  it("binds a successful workflow to the exact prepared commit", async () => {
    const evidence = await observeRunxCliRelease({
      version,
      expectedCommit: "b".repeat(40),
      fetchImpl: releaseFetch(),
      githubToken: "",
    });

    expect(evidence.ready).toBe(false);
    expect(failedIds(evidence)).toEqual(["github_tag", "release_workflow"]);
    expect(evidence.checks.find((check: { id: string }) => check.id === "release_workflow")?.detail)
      .toContain("expected");
  });

  it("refuses a workflow that did not run from the published tag commit", async () => {
    const evidence = await observeRunxCliRelease({
      version,
      fetchImpl: releaseFetch({ workflowCommit: "b".repeat(40) }),
      githubToken: "",
    });

    expect(evidence.ready).toBe(false);
    expect(failedIds(evidence)).toEqual(["release_workflow"]);
    expect(evidence.commitRef).toBe(commit);
  });

  it("requires complete version-bound release notes without placeholders", () => {
    const previousTag = "cli-v9.4.0";
    const body = [
      `# Runx CLI ${version}`,
      "",
      "A complete release summary.",
      "",
      ...RUNX_CLI_RELEASE_NOTE_SECTIONS.flatMap((section) => [
        `## ${section}`,
        "",
        `Complete ${section.toLowerCase()} evidence.`,
        "",
      ]),
      `**Full changelog**: https://github.com/runxhq/runx/compare/${previousTag}...${tag}`,
      "",
    ].join("\n");

    expect(validateRunxCliReleaseNotes({ body, version, previousTag }).ready).toBe(true);
    const incomplete = validateRunxCliReleaseNotes({
      body: body
        .replace("## Security\n\nComplete security evidence.\n\n", "")
        .replace("Complete fixed evidence.", "TBD"),
      version,
      previousTag,
    });
    expect(incomplete.ready).toBe(false);
    expect(failedIds(incomplete)).toEqual([
      "release_notes_security",
      "release_notes_no_placeholders",
    ]);
  });
});

function failedIds(evidence: {
  checks: readonly { id: string; status: string }[];
}): string[] {
  return evidence.checks
    .filter((check) => check.status === "failed")
    .map((check) => check.id);
}

function releaseFetch(options: {
  workflowConclusion?: string;
  workflowCommit?: string;
  npmLatest?: string;
  ghcrTokenStatus?: number;
  aurVersion?: string;
} = {}): typeof fetch {
  return (async (input: URL | RequestInfo) => {
    const url = String(input);
    if (url.includes("/releases/tags/")) {
      return jsonResponse({
        tag_name: tag,
        draft: false,
        prerelease: false,
        published_at: "2026-07-24T00:00:00Z",
        html_url: `https://github.com/runxhq/runx/releases/tag/${tag}`,
        assets: requiredAssets().map((name) => ({ name })),
      });
    }
    if (url.includes("/git/ref/tags/")) {
      return jsonResponse({ object: { type: "commit", sha: commit } });
    }
    if (url.includes("/actions/workflows/release.yml/runs")) {
      return jsonResponse({
        workflow_runs: [{
          head_branch: tag,
          head_sha: options.workflowCommit ?? commit,
          status: "completed",
          conclusion: options.workflowConclusion ?? "success",
          html_url: "https://github.com/runxhq/runx/actions/runs/1",
        }],
      });
    }
    if (url.startsWith("https://registry.npmjs.org/")) {
      const packageName = decodeURIComponent(url.slice("https://registry.npmjs.org/".length));
      if (!RUNX_CLI_NPM_PACKAGES.includes(packageName)) {
        return new Response("", { status: 404 });
      }
      return jsonResponse({
        versions: { [version]: { version } },
        "dist-tags": { latest: options.npmLatest ?? version },
      });
    }
    if (url.startsWith("https://ghcr.io/token")) {
      return options.ghcrTokenStatus
        ? new Response("", { status: options.ghcrTokenStatus })
        : jsonResponse({ token: "anonymous-pull-token" });
    }
    if (url.includes("/v2/runxhq/runx/manifests/")) {
      return jsonResponse({ schemaVersion: 2 });
    }
    if (url.includes("homebrew-tap")) {
      return new Response(
        `version "${version}"\nurl "https://github.com/runxhq/runx/releases/download/${tag}/runx-${version}-aarch64-apple-darwin.tar.gz"\n`,
      );
    }
    if (url.startsWith("https://aur.archlinux.org/rpc/v5/info")) {
      return jsonResponse({
        resultcount: 1,
        results: [{ Name: "runx", Version: options.aurVersion ?? `${version}-2` }],
      });
    }
    if (url.includes("scoop-bucket")) {
      return jsonResponse({
        version,
        architecture: {
          "64bit": {
            url: `https://github.com/runxhq/runx/releases/download/${tag}/runx-${version}-x86_64-pc-windows-msvc.zip`,
          },
        },
      });
    }
    return new Response("", { status: 404 });
  }) as typeof fetch;
}

function jsonResponse(value: unknown): Response {
  return new Response(JSON.stringify(value), {
    headers: { "content-type": "application/json" },
  });
}

function requiredAssets(): string[] {
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
