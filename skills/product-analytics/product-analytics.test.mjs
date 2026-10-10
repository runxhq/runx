import assert from "node:assert/strict";
import test from "node:test";
import { calculateMetrics } from "./product-analytics.mjs";

const definition = {
  product: "Example", identity: "account", time_zone: "UTC", cohort: "new accounts",
  filters: "exclude test accounts", funnel_rule: "ordered, within seven days of signup",
  retention_rule: "active during the seventh day after signup",
};
function report(name, activated = 75, returned = 40) {
  return {
    name, complete: true, source_digest: `sha256:${name === "current" ? "a" : "b"}`.padEnd(71, name === "current" ? "a" : "b"),
    observed_at: "2026-10-09T00:00:00Z",
    window: name === "current"
      ? { start_at: "2026-09-08T00:00:00Z", end_at: "2026-09-15T00:00:00Z" }
      : { start_at: "2026-09-01T00:00:00Z", end_at: "2026-09-08T00:00:00Z" },
    definition: { ...definition },
    funnel: { steps: [{ event: "signup", count: 100 }, { event: "activated", count: activated }] },
    retention: { period: "day 7", eligible: 80, returned },
  };
}
const calculate = (reports) => calculateMetrics({ objective: "Find activation gaps", reports }).metric_context;

test("compares rates in percentage points and preserves actual report observation time", () => {
  const context = calculate([report("current"), report("baseline", 50, 20)]);
  assert.equal(context.decision, "ready");
  assert.equal(context.reports[0].funnel[1].conversion_from_entry, 0.75);
  assert.equal(context.reports[0].funnel[1].lost_from_previous, 25);
  assert.equal(context.comparison.funnel[1].conversion_change_percentage_points, 25);
  assert.equal(context.comparison.retention_change_percentage_points, 25);
  assert.equal(context.reports[0].observed_at, "2026-10-09T00:00:00Z");
});

test("each report keeps the definition its source was computed under", () => {
  const baseline = report("baseline", 50, 20);
  baseline.definition = Object.fromEntries(Object.entries(definition).reverse());
  const context = calculate([report("current"), baseline]);
  assert.equal(context.decision, "ready");
  assert.deepEqual(context.blockers, []);
  assert.deepEqual(context.reports.map((item) => item.definition), [definition, definition]);
  assert.equal(context.comparison.funnel[1].conversion_change_percentage_points, 25);
});

test("refuses comparison when any definition field differs", () => {
  const changed = {
    product: "Other", identity: "person", time_zone: "America/New_York", cohort: "all accounts",
    filters: "include test accounts", funnel_rule: "unordered, within seven days of signup",
    retention_rule: "active during the thirtieth day after signup",
  };
  assert.deepEqual(Object.keys(changed).sort(), Object.keys(definition).sort());
  for (const [field, value] of Object.entries(changed)) {
    const items = [report("current"), report("baseline", 50, 20)];
    items[1].definition[field] = value;
    const context = calculate(items);
    assert.equal(context.decision, "needs_more_evidence");
    assert.equal(context.comparison, null);
    assert.deepEqual(context.reports, []);
    assert.deepEqual(context.blockers, [`baseline uses a different measurement definition: ${field}`]);
  }
});

test("account and person counts with the same events and window are not comparable", () => {
  const items = [report("current"), report("baseline", 400, 270)];
  items[1].definition.identity = "person";
  items[1].funnel.steps[0].count = 1000;
  items[1].retention.eligible = 900;
  const context = calculate(items);
  assert.equal(context.decision, "needs_more_evidence");
  assert.equal(context.comparison, null);
  assert.deepEqual(context.blockers, ["baseline uses a different measurement definition: identity"]);
});

test("empty cohorts have no measured rate or rate change", () => {
  const reports = [report("current", 0, 0), report("baseline", 0, 0)];
  for (const item of reports) { item.funnel.steps[0].count = 0; item.retention.eligible = 0; }
  const context = calculate(reports);
  assert.equal(context.decision, "ready");
  assert.equal(context.reports[0].funnel[1].conversion_from_previous, null);
  assert.equal(context.reports[0].retention.rate, null);
  assert.equal(context.comparison.retention_change_percentage_points, null);
});

test("withholds invalid counts and incompatible comparison arithmetic", () => {
  const changes = [
    (items) => { items[0].complete = false; },
    (items) => { items[0].funnel.steps[1].count = 101; },
    (items) => { items[0].retention.returned = 81; },
    (items) => { items[1].funnel.steps[1].event = "different activation"; },
    (items) => { items[1].window.end_at = "2026-09-09T00:00:00Z"; },
    (items) => { items[1].retention.period = "day 30"; },
    (items) => { items[1].name = "current"; },
  ];
  for (const change of changes) {
    const items = [report("current"), report("baseline")];
    change(items);
    const context = calculate(items);
    assert.equal(context.decision, "needs_more_evidence");
    assert.equal(context.comparison, null);
    assert.deepEqual(context.reports, []);
    assert.ok(context.blockers.length);
  }
});
