// The store's answer, checked and returned as this skill's typed result.

const RESULT_SCHEMA = "runx.data.operation_result.v1";
const STATUSES = new Set(["committed", "idempotent_replay", "read", "conflict"]);

export function dataOperationResult(inputs) {
  const result = inputs.result;
  if (!result || typeof result !== "object" || Array.isArray(result)) {
    throw new Error("the store answered without a result");
  }
  if (result.schema !== RESULT_SCHEMA) {
    throw new Error(`the store's result is not ${RESULT_SCHEMA}`);
  }
  if (!STATUSES.has(result.status)) {
    throw new Error(`the store answered with unknown status ${String(result.status)}`);
  }
  return result;
}
