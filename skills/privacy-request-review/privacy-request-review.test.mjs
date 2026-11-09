import assert from "node:assert/strict";
import { test } from "node:test";
import { judgeRequest, finalizeVerdict } from "./privacy-request-review.mjs";
const source = {
  stream: "runx/privacy-request-review/requests",
  key: "request:one",
};
const input = {
  request_packet: {
    type: "erasure",
    subject_id: "subject:one",
    scope: { request_id: "request:one", data_classes: ["profile"] },
  },
  requestor_proof: {
    identity_provider: "idp:test",
    verified_at: "2026-11-05T00:00:00Z",
    assertion: { subject_id: "subject:one", assertion_digest: "sha256:" + "a".repeat(64) },
  },
  policy: {
    jurisdiction: "test-policy",
    trusted_identity_providers: ["idp:test"],
    lawful_bases: { erasure: "policy-erasure" },
    scope_bounds: {
      data_classes: ["profile", "billing"],
      erasure_allowed: ["profile"],
      export_allowed: ["profile"],
    },
  },
};
test("requested scopes cannot broaden the request or type-specific policy", () => {
  const good = judgeRequest(input).verdict_draft.verdict;
  assert.equal(good.decision.eligible, true);
  assert.equal(good.handoff.path, null);
  const wide = judgeRequest({
    ...input,
    request_packet: {
      ...input.request_packet,
      scope: { ...input.request_packet.scope, requested_scopes: ["billing"] },
    },
  }).verdict_draft.verdict;
  assert.equal(wide.decision.eligible, false);
  assert.equal(wide.handoff, null);
  assert.deepEqual(wide.request.requested_scopes, ["billing"]);
});
const append = {
  ...source,
  operation: "events.append",
  status: "committed",
  before_version: 0,
  version: 1,
  event: { id: "event:1", digest: "sha256:" + "b".repeat(64) },
};
const record = {
  version: 1,
  id: append.event.id,
  digest: append.event.digest,
  type: "subject_request.verdict_recorded",
};
const finalize = (change = {}) =>
  finalizeVerdict({
    key: source.key,
    expected_version: 0,
    append_result: append,
    recorded_readback: { ...source, operation: "events.read", version: 9, last_version: 1, events: [record] },
    verdict_draft: judgeRequest(input).verdict_draft,
    request_digest: "sha256:" + "c".repeat(64),
    policy_digest: "sha256:" + "d".repeat(64),
    ...change,
  });
test("only the exact committed event produces a durable verdict, even after later events", () => {
  assert.equal(finalize().subject_request_verdict.persistence.after_version, 1);
  assert.equal(
    finalize({
      // A replay answers as the first append did, whatever followed it.
      append_result: { ...append, status: "replayed" },
    }).subject_request_verdict.persistence.after_version,
    1,
  );
  for (const change of [
    { append_result: { ...append, status: "version_conflict" } },
    { append_result: { ...append, key: "other" } },
    { append_result: { ...append, version: 2 } },
    {
      recorded_readback: {
        ...source,
        operation: "events.read",
        events: [{ ...record, digest: "wrong" }],
      },
    },
  ])
    assert.throws(() => finalize(change));
});
test("many disallowed scopes retain a bounded actionable refusal and full original scope", () => {
  const values = Array.from({ length: 100 }, (_, i) => "outside-" + i + "x".repeat(180));
  const verdict = judgeRequest({
    ...input,
    request_packet: {
      ...input.request_packet,
      scope: { request_id: "request:one", data_classes: values },
    },
  }).verdict_draft.verdict;
  assert.equal(verdict.decision.eligible, false);
  assert.ok(verdict.decision.reason.length < 4000);
  assert.deepEqual(verdict.request.data_classes, values);
});
