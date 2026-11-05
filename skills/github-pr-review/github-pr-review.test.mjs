import test from "node:test";
import assert from "node:assert/strict";
import {
  pinPull,
  bindComparison,
  selectFilePage,
  finalizeReview,
  patchAnchors,
  validateContext,
} from "./github-pr-review.mjs";
globalThis.Runx = { parseData: (source) => JSON.parse(source) };
const head = "a".repeat(40),
  base = "b".repeat(40),
  merge = "c".repeat(40),
  digest = "sha256:" + "d".repeat(64);
const artifact = {
  schema: "runx.project_artifact.v1",
  uri: "runx:project-artifact:" + digest,
  digest,
  bytes: 123,
  media_type: "application/json",
  path: "/synthetic/source",
};
function pin(count = 1, expected_head = "") {
  return pinPull({
    repository: "example/project",
    number: 42,
    expected_head,
    observed_at: "2026-11-04T00:00:00Z",
    pull_artifact: artifact,
    pull: {
      schema: "runx.provider.operation.v1",
      status: "success",
      provider: "github",
      operation: "pullrequest.read",
      target: "example/project",
      result: {
        pull: {
          number: 42,
          head: { sha: head },
          base: { sha: base, repo: { full_name: "example/project" } },
          title: "Use the selected account token",
          body: "Do not substitute another account.",
          changed_files: count,
        },
      },
    },
  }).snapshot;
}
function snapshot(count = 1) {
  return bindComparison({
    snapshot: pin(count),
    comparison_artifact: artifact,
    files_artifact: artifact,
    comparison_provider: "github",
    comparison_operation: "commits.compare",
    comparison_status: "success",
    comparison_target: "example/project",
    comparison_base: base,
    comparison_merge_base: merge,
    comparison_url: `https://api.github.com/repos/example/project/compare/${base}...${head}`,
  }).snapshot;
}
function file(path = "src/client.rs", length = 8) {
  return {
    filename: path,
    status: "modified",
    sha: "e".repeat(40),
    additions: 1,
    deletions: 1,
    patch: "@@ -12 +12 @@\n-" + "x".repeat(length) + "\n+" + "y".repeat(length),
  };
}
function page(records, eof = true, state = null) {
  return {
    records: records.map((record) => JSON.stringify(record)),
    whole_digest: digest,
    eof,
    state,
  };
}
function select(files, extra = {}) {
  return selectFilePage({
    snapshot: snapshot(files.length),
    patch_char_limit: 200000,
    runx_page: page(files),
    ...extra,
  }).review_context;
}
test("ordinary PR selection pins the actual head; an optional stale head holds before comparison", () => {
  assert.equal(pin().head_sha, head);
  assert.equal(pin().decision, "ready");
  const stale = pin(1, "f".repeat(40));
  assert.equal(stale.decision, "needs_more_evidence");
  assert.equal(stale.head_sha, head);
  assert.ok(stale.blockers[0].includes("no comparison"));
});
test("comparison binds its exact immutable head URL and merge base", () => {
  const wrong = bindComparison({
    snapshot: pin(),
    comparison_base: base,
    comparison_merge_base: merge,
    comparison_url:
      "https://api.github.com/repos/example/project/compare/main...main",
  }).snapshot;
  assert.equal(wrong.decision, "needs_more_evidence");
  const changed = snapshot();
  changed.files_artifact = { ...artifact, digest: "sha256:" + "0".repeat(64) };
  assert.throws(
    () => selectFilePage({ snapshot: changed, runx_page: page([file()]) }),
    /does not belong/u,
  );
});
test("more than 100 valid files remain useful under the declared context budget", () => {
  const context = select(
    Array.from({ length: 150 }, (_, i) => file(`src/file-${i}.rs`)),
  );
  assert.equal(context.decision, "ready");
  assert.equal(context.files.length, 150);
  assert.equal(context.selection.observed_files, 150);
  assert.equal(context.selection.scan_complete, true);
});
test("missing and truncated patches do not erase independent valid files", () => {
  const context = select([
    file(),
    { ...file("image.png"), patch: undefined },
    { ...file("truncated.rs"), patch: "@@ -1,2 +1,2 @@\n-x\n+y" },
  ]);
  assert.equal(context.decision, "ready");
  assert.equal(context.files.length, 1);
  assert.equal(context.selection.gap_count, 2);
});
test("native page partitioning does not change selection or source identity", () => {
  const files = Array.from({ length: 150 }, (_, i) => file(`src/file-${i}.rs`)),
    snap = snapshot(files.length);
  const first = selectFilePage({
    snapshot: snap,
    patch_char_limit: 200000,
    runx_page: page(files.slice(0, 50), false),
  });
  const second = selectFilePage({
    snapshot: snap,
    patch_char_limit: 200000,
    runx_page: page(files.slice(50), true, first.runx_page.state),
  });
  assert.deepEqual(second.review_context, select(files));
});
test("a later source change replaces lower-priority documentation without exceeding the budget", () => {
  const context = select(
    [file("docs/long.md", 10000), file("src/auth.rs", 10000)],
    { patch_char_limit: 30000 },
  );
  assert.deepEqual(
    context.files.map((row) => row.path),
    ["src/auth.rs"],
  );
  assert.ok(context.selection.admitted_characters <= 30000);
  assert.equal(context.selection.gap_count, 1);
});
test("explicit paths preserve intentional selection and do not trigger provider rediscovery", () => {
  const context = select([file("docs/long.md"), file("src/auth.rs")], {
    paths: ["docs/"],
  });
  assert.deepEqual(
    context.files.map((row) => row.path),
    ["docs/long.md"],
  );
  assert.equal(context.selection.excluded_files, 1);
});
test("duplicates cannot leave a misleading admitted source; empty admission is held", () => {
  const context = select([file(), file()]);
  assert.equal(context.files.length, 0);
  assert.equal(context.decision, "needs_more_evidence");
  const duplicate = file("src/" + "a".repeat(300));
  const bounded = select(
    [duplicate, file("docs/" + "b".repeat(600)), duplicate],
    { patch_char_limit: 1000 },
  );
  assert.ok(!bounded.files.some((row) => row.path === duplicate.filename));
});
test("provider comparison truncation is a visible scope limit, not a false all-file clearance", () => {
  const context = select([file()], { snapshot: snapshot(500) });
  assert.equal(context.decision, "ready");
  assert.ok(
    context.limitations.some((value) => value.includes("500 changed files")),
  );
});
test("changed-line validation preserves valid findings while withholding invented anchors", () => {
  const context = select([file()]),
    finding = {
      path: "src/client.rs",
      side: "RIGHT",
      line: 12,
      severity: "high",
      title: "Use the selected token",
      trigger: "A non-default account makes a request.",
      impact: "The request authenticates as another account.",
      suggested_change: "Use the selected account token.",
    };
  const result = finalizeReview({
    review_context: context,
    context_digest: digest,
    review_draft: {
      summary: "The changed authentication path has a concrete risk.",
      findings: [finding, { ...finding, line: 13 }],
      limitations: [],
    },
  }).pr_review;
  assert.equal(result.decision, "needs_more_evidence");
  assert.deepEqual(result.findings, [finding]);
  assert.equal(result.head_sha, head);
  assert.equal(result.diff_base_sha, merge);
  assert.equal(result.publication_status, "not_posted");
  assert.equal(result.execution_status, "not_tested");
});
test("check completeness requires actual source/head/count bindings and excludes legacy CI claims", () => {
  const context = select([file()]);
  assert.equal(context.checks_complete, false);
  assert.throws(
    () =>
      validateContext({
        review_context: { ...context, checks_complete: true },
      }),
    /lacks an exact head/u,
  );
  context.checks_source = { head_sha: head, total_count: 1, artifact };
  context.checks = [
    {
      id: 1,
      head_sha: head,
      name: "tests",
      status: "completed",
      conclusion: "success",
    },
  ];
  context.checks_complete = true;
  assert.equal(
    validateContext({ review_context: context }).review_context,
    context,
  );
  context.checks[0].head_sha = base;
  assert.throws(
    () => validateContext({ review_context: context }),
    /another head/u,
  );
});
test("patch coordinates reject truncated, overlapping and unprefixed hunks", () => {
  assert.equal(patchAnchors("@@ -1,2 +1,2 @@\n-x\n+y"), null);
  assert.equal(patchAnchors("@@ -1 +1 @@\n-x\n+y\n@@ -1 +1 @@\n-y\n+z"), null);
  assert.equal(patchAnchors("@@ -1 +1 @@\n\n-x\n+y"), null);
  assert.deepEqual(
    patchAnchors("@@ -0,0 +1 @@\n+new\n\\ No newline at end of file"),
    { left: [], right: [1], additions: 1, deletions: 0 },
  );
});
