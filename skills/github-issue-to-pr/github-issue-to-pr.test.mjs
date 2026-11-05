import test from "node:test";
import assert from "node:assert/strict";
import {
  admitHostResult,
  verifyCandidate,
  finishVerification,
  planDelivery,
  settlePullRequest,
  normalizeIssueEvidence,
} from "./github-issue-to-pr.mjs";
const sha = "a".repeat(40);
const issue = { repository: "runxhq/runx", number: "442" };
const work = () => ({
  schema: "runx/github-issue-to-pr.host_result.v1",
  status: "completed",
  summary: "Correct the issue workflow.",
  ...issue,
  issue_number: "442",
  repo_root: ".",
  branch: "fix/workflow",
  commit: sha,
  files: ["file.mjs"],
  tests: [{ command: "node --test", status: "passed", evidence: "actual test output" }],
  governance: {
    kind: "repository",
    reason: "Routine correction; repository instructions require focused checks.",
  },
  publication: {
    decision: "hold",
    title: "",
    body: "",
    head: "",
    base: "",
    draft: false,
    idempotency_key: "",
  },
  errors: [],
});
const observe = (host = work(), overrides = {}) =>
  verifyCandidate({
    host_result: host,
    admission: admitHostResult({ host_result: host, issue_evidence: issue }).admission,
    status: { clean: true, repo_root: "/repo" },
    branch: { branch: host.branch, detached: false, repo_root: "/repo" },
    history: { commits: [{ commit: sha }], repo_root: "/repo" },
    ...overrides,
  }).candidate;
test("ordinary work verifies its real candidate without imposing finalization", () => {
  const host = work(),
    candidate = observe(host);
  const verification = finishVerification({ host_result: host, candidate }).verification;
  assert.equal(verification.decision, "ready");
  assert.equal(verification.governance.status, "not_required");
  assert.equal(verification.candidate.status, "verified");
  assert.equal(planDelivery({ host_result: host, verification }).delivery_plan.route, "local");
});
test("failed checks, incomplete host work, wrong issue and host errors cannot admit a candidate", () => {
  for (const mutate of [
    (h) => (h.tests[0].status = "failed"),
    (h) => (h.status = "blocked"),
    (h) => (h.issue_number = "99"),
    (h) => h.errors.push("Unresolved regression"),
    (h) => (h.commit = ""),
  ]) {
    const host = work();
    mutate(host);
    assert.equal(
      admitHostResult({ host_result: host, issue_evidence: issue }).admission.decision,
      "blocked",
    );
  }
});
test("dirty, wrong branch, detached, changed head and wrong checkout block publication", () => {
  for (const overrides of [
    { status: { clean: false, repo_root: "/repo" } },
    { branch: { branch: "main", detached: false, repo_root: "/repo" } },
    { branch: { branch: "fix/workflow", detached: true, repo_root: "/repo" } },
    { history: { commits: [{ commit: "b".repeat(40) }], repo_root: "/repo" } },
    { status: { clean: true, repo_root: "/different" } },
  ])
    assert.equal(observe(work(), overrides).decision, "blocked");
  const wrong = work();
  wrong.repo_root = "/other";
  assert.equal(observe(wrong).decision, "blocked");
});
test("required finalization is still exact-commit, exact-contract and independently verified", () => {
  const host = work();
  host.governance = {
    kind: "scafld",
    reason: "Essential execution boundary change.",
    receipt_path: "receipt.json",
    contract_digest: "sha256:" + "b".repeat(64),
  };
  const candidate = observe(host);
  const packet = {
    schema: "runx.provider.operation.v1",
    provider: "scafld",
    operation: "receipt.verify",
    status: "success",
    result: {
      passed: true,
      receipt_digest: "sha256:" + "c".repeat(64),
      receipt: {
        body: {
          task_id: "task",
          verdict: "pass",
          head_commit: sha,
          spec_fingerprint: "b".repeat(64),
          open_blockers: [],
        },
      },
    },
  };
  assert.equal(
    finishVerification({ host_result: host, candidate, receipt_verification: packet }).verification
      .governance.status,
    "verified",
  );
  packet.result.receipt.body.spec_fingerprint = "d".repeat(64);
  assert.equal(
    finishVerification({ host_result: host, candidate, receipt_verification: packet }).verification
      .decision,
    "blocked",
  );
});
test("local-only intent prevents publication even for a ready proposal", () => {
  const host = work();
  host.publication = {
    decision: "ready",
    title: "fix: workflow",
    body: "Fixes #442.",
    head: host.branch,
    base: "main",
    draft: false,
    idempotency_key: "one-intent",
  };
  const verification = finishVerification({
    host_result: host,
    candidate: observe(host),
  }).verification;
  assert.equal(
    planDelivery({ host_result: host, verification, publication_mode: "hold" }).delivery_plan.route,
    "local",
  );
  assert.equal(
    planDelivery({ host_result: host, verification, publication_mode: "publish" }).delivery_plan
      .route,
    "publish",
  );
  host.publication.base = host.branch;
  assert.equal(planDelivery({ host_result: host, verification }).delivery_plan.route, "blocked");
});
test("a publication request cannot invent a ready host proposal", () => {
  const host = work();
  const verification = finishVerification({
    host_result: host,
    candidate: observe(host),
  }).verification;
  assert.equal(
    planDelivery({ host_result: host, verification, publication_mode: "publish" }).delivery_plan
      .route,
    "blocked",
  );
});
test("PR reuse is bound to exact head, base repository and actual PR discovery readback", () => {
  const host = work();
  host.publication.head = host.branch;
  host.publication.base = "main";
  const op = (operation, result) => ({
    schema: "runx.provider.operation.v1",
    status: "success",
    provider: "github",
    operation,
    target: host.repository,
    result,
    readback_uri: "runx:evidence:" + operation,
  });
  const pull = {
    number: 77,
    html_url: "https://github.com/runxhq/runx/pull/77",
    state: "open",
    head: { ref: host.branch, sha },
    base: { ref: "main", repo: { full_name: host.repository } },
  };
  const inputs = {
    host_result: host,
    pushed: op("branch.push", { commit: sha, branch: host.branch }),
    choice: { action: "reuse", existing: pull },
    found: op("pullrequests.list", { pulls: [pull] }),
  };
  assert.equal(settlePullRequest(inputs).publication.status, "published");
  assert.equal(
    settlePullRequest(inputs).publication.readback_uri,
    "runx:evidence:pullrequests.list",
  );
  pull.head.sha = "b".repeat(40);
  assert.equal(settlePullRequest(inputs).publication.status, "failed");
});
test("a different issue or a pull request cannot masquerade as selected issue evidence", () => {
  const packet = {
    schema: "runx.provider.operation.v1",
    provider: "github",
    operation: "issue.read",
    status: "success",
    target: issue.repository,
    result: { issue: { number: 442, html_url: "https://github.com/runxhq/runx/issues/442" } },
  };
  const input = {
    requested_repository: issue.repository,
    requested_issue_number: "442",
    provider_operation: packet,
  };
  assert.equal(normalizeIssueEvidence(input).issue_evidence.number, "442");
  packet.result.issue.number = 3;
  assert.throws(() => normalizeIssueEvidence(input), /does not identify/);
  packet.result.issue.number = 442;
  packet.result.issue.pull_request = {};
  assert.throws(() => normalizeIssueEvidence(input), /does not identify/);
});
