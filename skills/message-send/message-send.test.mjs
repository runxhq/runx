import { test } from "node:test";
import assert from "node:assert/strict";
import { prepareNotification } from "./post.mjs";
import { prepareReply, holdMessage } from "./reply.mjs";
test("message planning preserves exact leading spaces, line breaks and larger provider-valid text", () => {
  for (const message of ["  indented\n", "x".repeat(5000)]) {
    const r = prepareNotification({
      channel: "slack://T123/C456",
      principal: "runx:principal:operator",
      content: { message },
    });
    assert.equal(r.digest_input, message);
    assert.deepEqual(r.notification_admission.findings, []);
  }
});
test("a source-bound reply preserves formatting and does not impose an unrelated smaller limit", () => {
  const thread_locator = "slack://T123/C456/123.000001";
  const message = "  " + "x".repeat(5000) + "\n";
  const r = prepareReply({
    source_context: {
      schema: "runx.source_packet.v1",
      source_ref: { uri: thread_locator },
      workflow_inputs: {
        source_locator: thread_locator,
        source_complete: true,
        source_digest: "sha256:" + "a".repeat(64),
        thread_locator,
        thread: { thread_locator, coverage_incomplete: false },
      },
    },
    expected_provider: "slack",
    thread_locator,
    draft_message: message,
    digest: { digest: "sha256:" + "b".repeat(64) },
    turn_id: "sha256:" + "c".repeat(64),
  });
  assert.equal(r.reply_plan.ready, true);
  assert.equal(r.reply_plan.request.text, message);
});
test("delivery drift has an explicit held outcome without completion evidence", () => {
  assert.deepEqual(holdMessage({ reply_plan: { reason: "Content changed; reapprove it." } }), {
    work_result: {
      state: "held",
      summary: "Content changed; reapprove it.",
      completion_evidence: [],
      followups: [],
    },
  });
});
