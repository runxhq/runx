import assert from "node:assert/strict";
import test from "node:test";

import { prepareReadback, requireCompleteReadback } from "./readback.mjs";

test("reads one payment against the resource it paid for", () => {
  assert.deepEqual(prepareReadback({
    payment_ref: "runx:x402-payment:test",
    resource_url: "https://vendor.example/v1/invocations",
  }), {
    readback_request: {
      payment_ref: "runx:x402-payment:test",
      resource_url: "https://vendor.example/v1/invocations",
    },
    expected_result: { payment_ref: "runx:x402-payment:test" },
  });
  assert.throws(() => prepareReadback({
    payment_ref: "runx:x402-payment:test",
    resource_url: "http://vendor.example/v1/invocations",
  }), /exact HTTPS URL/);
});

test("requires terminal confirmed receipt for synchronous pay", () => {
  assert.deepEqual(requireCompleteReadback({
    readback: {
      readback_status: "complete",
      finality: "confirmed",
      inner_receipt_ref: `runx:receipt:sha256:${"a".repeat(64)}`,
    },
  }), { verified: { readback_status: "complete" } });
  assert.throws(
    () => requireCompleteReadback({ readback: { readback_status: "pending" } }),
    /durable readback/u,
  );
});
