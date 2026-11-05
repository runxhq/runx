import { test } from "node:test";
import assert from "node:assert/strict";
import { validateQuery } from "./web-search.mjs";
test("invalid or reversed publication windows stop before a potentially paid search", () => {
  for (const input of [
    { start_published_at: "not-a-date" },
    { end_published_at: "" },
    { start_published_at: "2026-11-05T00:00:00Z", end_published_at: "2026-11-01T00:00:00Z" },
  ])
    assert.throws(() => validateQuery(input), /no search was dispatched/);
  assert.deepEqual(validateQuery({}), { valid: true });
  assert.deepEqual(
    validateQuery({
      start_published_at: "2026-11-01T00:00:00Z",
      end_published_at: "2026-11-05T00:00:00Z",
    }),
    { valid: true },
  );
});
