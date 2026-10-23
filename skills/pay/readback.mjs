const TERMINAL = new Set(["complete", "failed", "pending"]);

/**
 * The read of one payment, targeting the resource it paid for, so the grant
 * that authorized the payment authorizes reading it back.
 */
export function prepareReadback(inputs) {
  const paymentId = inputs.payment_id;
  if (typeof paymentId !== "string" || !paymentId.trim()
    || paymentId !== paymentId.trim() || paymentId.length > 300) {
    throw new Error("x402 payment reference is invalid.");
  }
  const resourceUrl = inputs.resource_url;
  if (typeof resourceUrl !== "string" || !/^https:\/\/[^\s/?#]+[^\s]*$/u.test(resourceUrl)
    || resourceUrl.length > 2048) {
    throw new Error("x402 resource URL must be the exact HTTPS URL that was paid.");
  }
  return {
    readback_request: Object.freeze({ payment_id: paymentId, resource_url: resourceUrl }),
    expected_result: Object.freeze({ payment_id: paymentId }),
  };
}

export function requireCompleteReadback(inputs) {
  const readback = record(inputs.readback, "x402 readback");
  const status = readback.readback_status;
  if (typeof status !== "string" || !TERMINAL.has(status)) {
    throw new Error("x402 readback status is invalid.");
  }
  if (status === "pending") {
    throw new Error("x402 paid resource is still pending; continue through durable readback.");
  }
  if (status === "failed") {
    throw new Error(`x402 paid resource completed as ${boundedState(readback.resource_state)}.`);
  }
  // Completion rests on payment evidence alone: a confirmed transaction and the
  // resource's 2xx answer. It never waits on the resource's own product state.
  if (typeof readback.transaction !== "string"
    || !/^0x[0-9a-fA-F]{64}$/u.test(readback.transaction)
    || readback.finality !== "confirmed") {
    throw new Error("x402 completed readback lacks a confirmed transaction.");
  }
  if (!Number.isInteger(readback.resource_status)
    || readback.resource_status < 200 || readback.resource_status > 299) {
    throw new Error("x402 completed readback lacks a 2xx resource answer.");
  }
  return { verified: Object.freeze({ readback_status: "complete" }) };
}

function boundedState(value) {
  if (typeof value !== "string" || !value.trim() || value.length > 96) {
    throw new Error("x402 failed readback state is invalid.");
  }
  return value;
}

function record(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object.`);
  }
  return value;
}
