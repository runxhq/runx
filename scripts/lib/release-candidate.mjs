// Publication promotes one completed rehearsal of this exact source revision.
// GitHub's immutable artifact ids are the selection boundary; names alone are not.
export const RELEASE_CANDIDATE_ARTIFACTS = Object.freeze({
  release_assets: "release-assets",
  npm_tarballs: "npm-tarballs",
  channel_manifests: "channel-manifests",
  channel_input: "channel-input",
});

export async function readReleaseCandidate({
  repository, commit, runId, attempt, githubToken, fetchImpl = globalThis.fetch,
}) {
  if (!/^[\w.-]+\/[\w.-]+$/u.test(repository)
    || !/^[0-9a-f]{40}$/u.test(commit)
    || !/^[1-9]\d*$/u.test(String(runId))
    || !/^[1-9]\d*$/u.test(String(attempt))) {
    throw new Error("candidate requires a repository, exact commit, positive run id and attempt");
  }
  const base = `https://api.github.com/repos/${repository}/actions/runs/${runId}`;
  const headers = {
    accept: "application/vnd.github+json",
    "x-github-api-version": "2022-11-28",
    ...(githubToken ? { authorization: `Bearer ${githubToken}` } : {}),
  };
  async function get(url) {
    const response = await fetchImpl(url, { headers, redirect: "error", signal: AbortSignal.timeout(30_000) });
    if (!response.ok) throw new Error(`release rehearsal lookup returned HTTP ${response.status}`);
    return response.json();
  }
  const run = await get(base);
  if (String(run.id) !== String(runId) || String(run.run_attempt) !== String(attempt)
    || run.head_sha !== commit || run.event !== "workflow_dispatch"
    || run.path !== ".github/workflows/release.yml"
    || run.repository?.full_name !== repository || run.head_repository?.full_name !== repository
    || run.status !== "completed" || run.conclusion !== "success") {
    throw new Error("release rehearsal must be the successful exact-commit release workflow and attempt in this repository");
  }
  const listing = await get(`${base}/artifacts?per_page=100`);
  if (!Array.isArray(listing.artifacts) || listing.total_count !== listing.artifacts.length) {
    throw new Error("release rehearsal artifact listing is incomplete");
  }
  const artifacts = {};
  for (const [key, name] of Object.entries(RELEASE_CANDIDATE_ARTIFACTS)) {
    const matches = listing.artifacts.filter((artifact) => artifact.name === name);
    const artifact = matches[0];
    if (matches.length !== 1 || !Number.isSafeInteger(artifact?.id) || artifact.id < 1
      || artifact.expired !== false || !/^sha256:[0-9a-f]{64}$/u.test(artifact.digest ?? "")
      || String(artifact.workflow_run?.id) !== String(runId)
      || artifact.workflow_run?.head_sha !== commit) {
      throw new Error(`release rehearsal needs one unexpired, digest-bound ${name} artifact`);
    }
    artifacts[key] = { id: artifact.id, digest: artifact.digest };
  }
  return { run_id: String(runId), attempt: String(attempt), commit, artifacts };
}

export function candidateFromTag(annotation) {
  const lines = annotation.split("\n").filter(line => line.startsWith("release-candidate:"));
  const match = lines.length === 1 && /^release-candidate: ([1-9]\d*)\/([1-9]\d*)$/u.exec(lines[0]);
  if (!match) {
    throw new Error("release tag must name exactly one release-candidate: <run-id>/<attempt>");
  }
  return { runId: match[1], attempt: match[2] };
}
