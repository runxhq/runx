import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import os from "node:os";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";
import { candidateFromTag, readReleaseCandidate, RELEASE_CANDIDATE_ARTIFACTS } from "./lib/release-candidate.mjs";
import { inspectProfile } from "../skills/release/release.mjs";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const repository = "runxhq/runx";
const commit = "a".repeat(40);
const options = { repository, commit, runId: "123", attempt: "2", githubToken: "test-read-token" };
function fixtures() {
  return {
    run: { id: 123, run_attempt: 2, head_sha: commit, event: "workflow_dispatch",
      path: ".github/workflows/release.yml", repository: { full_name: repository },
      head_repository: { full_name: repository }, status: "completed", conclusion: "success" },
    artifacts: { total_count: 4, artifacts: Object.values(RELEASE_CANDIDATE_ARTIFACTS).map((name, index) => ({
      id: index + 1, name, expired: false, digest: `sha256:${"b".repeat(64)}`,
      workflow_run: { id: 123, head_sha: commit },
    })) },
  };
}
function fetcher(data, requests = []) {
  return async (url, init) => {
    requests.push({ url, headers: init.headers });
    return Response.json(url.includes("/artifacts?") ? data.artifacts : data.run);
  };
}

test("promotion pins the successful source, workflow attempt and immutable artifact ids", async () => {
  const requests = [];
  const result = await readReleaseCandidate({ ...options, fetchImpl: fetcher(fixtures(), requests) });
  assert.equal(result.commit, commit);
  assert.equal(result.run_id, "123");
  assert.deepEqual(Object.values(result.artifacts).map(a => a.id), [1, 2, 3, 4]);
  assert.equal(requests.length, 2);
  assert.ok(requests.every(r => r.url.startsWith(`https://api.github.com/repos/${repository}/`)));
  assert.ok(requests.every(r => r.headers.authorization === "Bearer test-read-token"));
});

test("promotion refuses another commit, workflow, repository, attempt or unsuccessful run", async () => {
  for (const mutation of [
    run => { run.head_sha = "c".repeat(40); },
    run => { run.run_attempt = 3; },
    run => { run.event = "pull_request"; },
    run => { run.path = ".github/workflows/ci.yml"; },
    run => { run.repository.full_name = "someone/else"; },
    run => { run.head_repository.full_name = "someone/fork"; },
    run => { run.status = "in_progress"; },
    run => { run.conclusion = "failure"; },
  ]) {
    const data = fixtures(); mutation(data.run);
    await assert.rejects(readReleaseCandidate({ ...options, fetchImpl: fetcher(data) }), /successful exact-commit/u);
  }
});

test("promotion refuses missing, ambiguous, expired, unbound or partially listed artifacts", async () => {
  for (const mutation of [
    listing => { listing.artifacts.pop(); listing.total_count--; },
    listing => { listing.artifacts.push(listing.artifacts[0]); listing.total_count++; },
    listing => { listing.artifacts[0].expired = true; },
    listing => { delete listing.artifacts[0].digest; },
    listing => { listing.artifacts[0].workflow_run.id = 999; },
    listing => { listing.artifacts[0].workflow_run.head_sha = "d".repeat(40); },
    listing => { listing.total_count++; },
  ]) {
    const data = fixtures(); mutation(data.artifacts);
    await assert.rejects(readReleaseCandidate({ ...options, fetchImpl: fetcher(data) }), /artifact/u);
  }
  await assert.rejects(readReleaseCandidate({ ...options, fetchImpl: async () => new Response("", { status: 403 }) }), /HTTP 403/u);
});

test("a publication tag names exactly one concrete rehearsal attempt", () => {
  assert.deepEqual(candidateFromTag("Runx CLI 0.12.0\n\nrelease-candidate: 123/2\n"), { runId: "123", attempt: "2" });
  for (const annotation of ["", "release-candidate: latest", "release-candidate: 0/1", "release-candidate: 123/2\nrelease-candidate: 456/1", "release-candidate: 123/2\nrelease-candidate: latest"]) {
    assert.throws(() => candidateFromTag(annotation), /exactly one/u);
  }
});

test("the prepared native profile binds the rehearsal in exact argv and refuses replacement", () => {
  const directory = mkdtempSync(path.join(os.tmpdir(), "runx-pinned-release-"));
  try {
    mkdirSync(path.join(directory, "release"));
    writeFileSync(path.join(directory, "release/runx-cli.json"), readFileSync(path.join(root, "release/runx-cli.json")));
    const args = [path.join(root, "scripts/runx-cli-release-profile.mjs"), "profile", "--candidate-run", "123", "--candidate-attempt", "2"];
    const invocation = { cwd: directory, env: { HOME: directory, PATH: "/usr/bin:/bin" }, stdio: "pipe" };
    const generated = JSON.parse(execFileSync(process.execPath, args, invocation).toString());
    const filename = path.join(directory, generated.profile_path);
    const contents = readFileSync(filename, "utf8");
    const { release_context: context } = inspectProfile({
      project_root: ".", channel: "runx-cli", version: "0.12.0",
      profile_file: { path: generated.profile_path, contents,
        content_digest: `sha256:${createHash("sha256").update(contents).digest("hex")}` },
    });
    for (const phase of ["prepare", "publish", "verify"]) {
      assert.deepEqual(context.commands[phase].args.slice(-4), args.slice(-4));
      assert.equal(context.commands[phase].env.RUNX_RELEASE_PHASE, phase);
    }
    execFileSync(process.execPath, args, invocation);
    const other = JSON.parse(execFileSync(process.execPath, args.map(a => a === "123" ? "456" : a), invocation).toString());
    assert.notEqual(other.profile_path, generated.profile_path);
    assert.equal(readFileSync(filename, "utf8"), contents);
    writeFileSync(filename, "changed local profile");
    assert.throws(() => execFileSync(process.execPath, args, invocation), /EEXIST/u);
    assert.equal(readFileSync(filename, "utf8"), "changed local profile");
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("the real generated platform matrix satisfies the build and smoke workflow", () => {
  const { include } = JSON.parse(execFileSync(process.execPath, [path.join(root, "scripts/release-platform-matrix.mjs")], { encoding: "utf8" }));
  const requireContracts = createRequire(path.join(root, "packages/contracts/package.json"));
  const workflow = requireContracts("yaml").parse(readFileSync(path.join(root, ".github/workflows/release.yml"), "utf8"));
  for (const name of ["build", "smoke"]) {
    const job = workflow.jobs[name];
    const fields = [...JSON.stringify(job).matchAll(/matrix\.([a-z_]+)/gu)].map(match => match[1]);
    for (const platform of include) for (const field of fields) assert.ok(platform[field], `${platform.platform} must supply ${field}`);
    assert.equal(job.if, "github.event_name == 'workflow_dispatch'", "a tag never enters a native build or smoke");
  }
  const recorder = workflow.jobs.build.steps.find(step => step.name === "Record deterministic module platform evidence");
  assert.match(recorder.run, /--host "\$\{\{ matrix\.runner \}\}"/u);
  const directory = mkdtempSync(path.join(os.tmpdir(), "runx-platform-contract-"));
  try {
    for (const platform of include) {
      const worker = path.join(directory, platform.worker);
      writeFileSync(worker, "contract fixture only; no execution proof");
      const output = path.join(directory, `${platform.platform}.json`);
      execFileSync(process.execPath, [path.join(root, "scripts/record-deterministic-module-platform-evidence.mjs"),
        "--decision", "docs/architecture/deterministic-module-engine.json",
        "--platform", platform.platform, "--target", platform.target,
        "--host", platform.runner, "--worker", worker, "--out", output], { stdio: "pipe" });
      assert.equal(JSON.parse(readFileSync(output, "utf8")).host, platform.runner);
    }
  } finally { rmSync(directory, { recursive: true, force: true }); }
  for (const name of ["publish-npm", "package-managers"]) {
    const publication = workflow.jobs[name];
    assert.match(publication.if, /!cancelled\(\)/u, "publication survives skipped build jobs but respects cancellation");
  }
  const npm = workflow.jobs["publish-npm"].steps;
  assert.ok(!npm.some(step => /npm pack|pack-typescript/u.test(step.run ?? "")), "publication cannot repack the candidate");
  const downloads = npm.filter(step => step.uses?.startsWith("actions/download-artifact"));
  assert.equal(downloads.length, 1);
  assert.equal(downloads[0].with["artifact-ids"], "${{ needs.prepare.outputs.npm_tarballs_id }}");
  assert.equal(downloads[0].with["run-id"], "${{ needs.prepare.outputs.candidate_run_id }}");
});
