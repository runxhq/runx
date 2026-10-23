import assert from "node:assert/strict";
import test from "node:test";

import { prepareReadback, requireCompleteReadback } from "./readback.mjs";

test("reads one payment against the resource it paid for", () => {
  assert.deepEqual(prepareReadback({
    payment_id: "runx:x402-payment:test",
    resource_url: "https://merchant.example/v1/report",
  }), {
    readback_request: {
      payment_id: "runx:x402-payment:test",
      resource_url: "https://merchant.example/v1/report",
    },
    expected_result: { payment_id: "runx:x402-payment:test" },
  });
  assert.throws(() => prepareReadback({
    payment_id: "runx:x402-payment:test",
    resource_url: "http://merchant.example/v1/report",
  }), /exact HTTPS URL/);
});

test("completes on a confirmed transaction and a 2xx resource answer", () => {
  const complete = {
    readback_status: "complete",
    finality: "confirmed",
    transaction: `0x${"a".repeat(64)}`,
    resource_status: 200,
  };
  assert.deepEqual(requireCompleteReadback({ readback: complete }), {
    verified: { readback_status: "complete" },
  });
  assert.throws(
    () => requireCompleteReadback({ readback: { ...complete, finality: "pending" } }),
    /confirmed transaction/u,
  );
  assert.throws(
    () => requireCompleteReadback({ readback: { ...complete, transaction: "0xabc" } }),
    /confirmed transaction/u,
  );
  assert.throws(
    () => requireCompleteReadback({ readback: { ...complete, resource_status: 502 } }),
    /2xx resource answer/u,
  );
  assert.throws(
    () => requireCompleteReadback({ readback: { readback_status: "pending" } }),
    /durable readback/u,
  );
});
