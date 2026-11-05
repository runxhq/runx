import assert from "node:assert/strict";
import { test } from "node:test";
import { admitIssue, indexIssues } from "./github-issue-triage.mjs";
const digest = "sha256:" + "a".repeat(64);
const snapshot = {
  repository: "runxhq/runx",
  number: "557",
  title: "Quickstart failure",
  state: "open",
  body:
    "    preserve indentation\n" + "x".repeat(30000) + "\n  trailing spaces  ",
};
test("prepared issue evidence preserves a long report byte for byte", () => {
  const result = admitIssue({
    issue_snapshot: snapshot,
    snapshot_digest: digest,
  }).issue_evidence;
  assert.equal(result.body, snapshot.body);
  assert.equal(result.digest, digest);
});
test("queue normalization preserves the same complete source text", () => {
  const result = indexIssues({
    issue_snapshots: [snapshot],
    query: "Find the blocking repro",
    snapshot_digest: digest,
  }).issue_index;
  assert.equal(result.issues[0].body, snapshot.body);
});
test("source character bounds agree for Unicode, and malformed bodies do not become empty", () => {
  const result = admitIssue({
    issue_snapshot: { ...snapshot, body: "🟠".repeat(40000) },
    snapshot_digest: digest,
  }).issue_evidence;
  assert.equal([...result.body].length, 40000);
  assert.throws(
    () =>
      admitIssue({
        issue_snapshot: { ...snapshot, body: 42 },
        snapshot_digest: digest,
      }),
    /must be text/,
  );
  assert.equal(
    admitIssue({
      issue_snapshot: { ...snapshot, body: null },
      snapshot_digest: digest,
    }).issue_evidence.body,
    "",
  );
});
