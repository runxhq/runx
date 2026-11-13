import { test } from "node:test";
import assert from "node:assert/strict";
import { prepareHistory, finalizeHistory } from "./run-history.mjs";
test("live history retains unreadable records and counts them toward its bound", () => {
  const unreadable = [{ run_id: "run_retired", updated_at: "2026-11-13T00:00:00Z", reason: "unsupported record" }];
  const history_context = prepareHistory({ objective: "Review", limit: 2 }).history_context;
  const report = finalizeHistory({
    history_context,
    run_list: {
      runs: [{ skill: "constructor", state: "sealed", outcome: "completed" }],
      unreadable,
    },
    authoring_context: { catalog_skills: [] },
  }).history_report;
  assert.equal(report.decision, "needs_more_evidence");
  assert.deepEqual(report.runs.unreadable, unreadable);
  assert.equal(report.runs.terminal_count, 1);
  assert.equal(report.runs.completed_rate, 1);
  assert.deepEqual(report.runs.top_subjects, [{ subject: "constructor", count: 1 }]);
  assert.ok(report.limitations.some((line) => line.includes("bound was reached")));
  assert.ok(report.recommendations.some((item) => item.action.includes("compatible runtime")));
  assert.ok(report.recommendations.every((item) => !item.action.includes("empty ledger")));
});
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
