const TERMINAL = new Set(["complete", "failed", "pending"]);

/**
 * The read of one payment, targeting the resource it paid for, so the grant
 * that authorized the payment authorizes reading it back.
 */
export function prepareReadback(inputs) {
  const paymentRef = inputs.payment_ref;
  if (typeof paymentRef !== "string" || !paymentRef.trim()
    || paymentRef !== paymentRef.trim() || paymentRef.length > 300) {
    throw new Error("x402 payment reference is invalid.");
  }
  const resourceUrl = inputs.resource_url;
  if (typeof resourceUrl !== "string" || !/^https:\/\/[^\s/?#]+[^\s]*$/u.test(resourceUrl)
    || resourceUrl.length > 2048) {
    throw new Error("x402 resource URL must be the exact HTTPS URL that was paid.");
  }
  return {
    readback_request: Object.freeze({ payment_ref: paymentRef, resource_url: resourceUrl }),
    expected_result: Object.freeze({ payment_ref: paymentRef }),
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
  if (typeof readback.inner_receipt_ref !== "string"
    || !readback.inner_receipt_ref.startsWith("runx:receipt:")
    || readback.finality !== "confirmed") {
    throw new Error("x402 completed readback lacks confirmed receipt evidence.");
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
