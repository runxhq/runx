import { test } from "node:test";
import assert from "node:assert/strict";
import { prepareHistory, finalizeHistory } from "./run-history.mjs";
test("replay cannot silently relabel rows with an unapplied time window", () => {
  assert.throws(
    () =>
      prepareHistory({
        objective: "Review",
        limit: 1000,
        period: "7d",
        runs: [],
        catalog_items: [],
      }),
    /already-windowed/,
  );
});
test("scope filtering precedes the replay bound and coverage remains explicit", () => {
  const context = prepareHistory({
    objective: "Review",
    scope: "target",
    limit: 1,
    runs: [],
    catalog_items: [],
  }).history_context;
  const r = finalizeHistory({
    history_context: context,
    runs: [
      { skill: "other", state: "sealed", outcome: "failed" },
      { skill: "target", state: "sealed", outcome: "completed" },
    ],
    catalog_items: [],
  }).history_report;
  assert.equal(r.runs.terminal_count, 1);
  assert.deepEqual(r.runs.outcomes, { completed: 1 });
  assert.ok(r.limitations.some((l) => l.includes("bound was reached")));
  assert.ok(r.limitations.some((l) => l.includes("no temporal filtering")));
});
