import assert from "node:assert/strict";
import { test } from "node:test";
import { finalizeConversation } from "./conversation-review.mjs";
const source = {
  source_locator: "host://qa/conversation/catalog-status/message-2",
  source_digest: "sha256:72f89585f9e2a3001e88ad62245daa0dcab37d60670634f1a9b22a7ae55b852b",
  thread_locator: "host://qa/conversation/catalog-status",
  title: "Catalog completion status",
  body: "Can you confirm the catalog work is finished?",
  thread: {
    source_locator: "host://qa/conversation/catalog-status/message-2",
    source_digest: "sha256:72f89585f9e2a3001e88ad62245daa0dcab37d60670634f1a9b22a7ae55b852b",
    thread_locator: "host://qa/conversation/catalog-status",
    coverage_incomplete: false,
    messages: [
      {
        message_locator: "host://qa/conversation/catalog-status/message-1",
        author_id: "operator",
        is_self: true,
        text: "Current checkpoint: native fixes passed full local acceptance and independent review. Other exported skill work and final registry parity are still unfinished. No CI or release was run.",
      },
      {
        message_locator: "host://qa/conversation/catalog-status/message-2",
        author_id: "requester",
        is_self: false,
        text: "Can you confirm the catalog work is finished?",
      },
    ],
  },
  source_complete: true,
};
const draft = {
  kind: "reply",
  summary: "A status reply is requested.",
  reason: "Use the admitted checkpoint only.",
  draft_message: "  Exact reply text.\n",
  request_quote: "",
};
test("preserves the exact admitted reply and source binding", () => {
  const result = finalizeConversation({ ...source, draft }).conversation_packet;
  assert.equal(result.status, "reply");
  assert.equal(result.draft_message, draft.draft_message);
  assert.equal(result.source_locator, source.source_locator);
  assert.equal(result.delivery_status, "not_sent");
});
test("a coding quote must occur within one admitted source, not bridge separate messages", () => {
  const input = structuredClone(source);
  input.title = "General discussion";
  input.body = "Please fix";
  input.thread.messages = [{ text: "the parser" }];
  const result = finalizeConversation({
    ...input,
    draft: {
      kind: "coding",
      summary: "An alleged request.",
      reason: "Test exact source fidelity.",
      draft_message: "",
      request_quote: "Please fix\nthe parser",
    },
  }).conversation_packet;
  assert.equal(result.status, "needs_context");
  assert.equal(result.request_quote, null);
  assert.equal(result.validation.status, "fail");
});
