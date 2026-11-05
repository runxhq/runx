import assert from "node:assert/strict";
import test from "node:test";
import { calculateMetrics } from "./product-analytics.mjs";

const definition = {
  product: "Example",
  identity: "account",
  time_zone: "UTC",
  cohort: "new accounts",
  filters: "exclude test accounts",
  funnel_rule: "ordered, within seven days of signup",
  retention_rule: "active during the seventh day after signup",
};
function report(name, activated = 75, returned = 40) {
  return {
    name,
    complete: true,
    source_digest: `sha256:${name === "current" ? "a" : "b"}`.padEnd(
      71,
      name === "current" ? "a" : "b",
    ),
    observed_at: "2026-10-09T00:00:00Z",
    window:
      name === "current"
        ? { start_at: "2026-09-08T00:00:00Z", end_at: "2026-09-15T00:00:00Z" }
        : { start_at: "2026-09-01T00:00:00Z", end_at: "2026-09-08T00:00:00Z" },
    definition: { ...definition },
    funnel: {
      steps: [
        { event: "signup", count: 100 },
        { event: "activated", count: activated },
      ],
    },
    retention: { period: "day 7", eligible: 80, returned },
  };
}
const calculate = (reports) =>
  calculateMetrics({ objective: "Find activation gaps", reports })
    .metric_context;

test("compares rates in percentage points and preserves actual report observation time", () => {
  const context = calculate([report("current"), report("baseline", 50, 20)]);
  assert.equal(context.decision, "ready");
  assert.equal(context.reports[0].funnel[1].conversion_from_entry, 0.75);
  assert.equal(context.reports[0].funnel[1].lost_from_previous, 25);
  assert.equal(
    context.comparison.funnel[1].conversion_change_percentage_points,
    25,
  );
  assert.equal(context.comparison.retention_change_percentage_points, 25);
  assert.equal(context.reports[0].observed_at, "2026-10-09T00:00:00Z");
});

test("each report keeps the definition its source was computed under", () => {
  const baseline = report("baseline", 50, 20);
  baseline.definition = Object.fromEntries(
    Object.entries(definition).reverse(),
  );
  const context = calculate([report("current"), baseline]);
  assert.equal(context.decision, "ready");
  assert.deepEqual(context.blockers, []);
  assert.deepEqual(
    context.reports.map((item) => item.definition),
    [definition, definition],
  );
  assert.equal(
    context.comparison.funnel[1].conversion_change_percentage_points,
    25,
  );
});

test("withholds only comparison when a definition differs", () => {
  const changed = {
    product: "Other",
    identity: "person",
    time_zone: "America/New_York",
    cohort: "all accounts",
    filters: "include test accounts",
    funnel_rule: "unordered, within seven days of signup",
    retention_rule: "active during the thirtieth day after signup",
  };
  assert.deepEqual(Object.keys(changed).sort(), Object.keys(definition).sort());
  for (const [field, value] of Object.entries(changed)) {
    const items = [report("current"), report("baseline", 50, 20)];
    items[1].definition[field] = value;
    const context = calculate(items);
    assert.equal(context.decision, "ready");
    assert.equal(context.comparison, null);
    assert.equal(context.reports.length, 2);
    assert.deepEqual(context.blockers, []);
    assert.deepEqual(context.comparison_blockers, [
      `baseline uses a different measurement definition: ${field}`,
    ]);
  }
});

test("account and person counts with the same events and window are not comparable", () => {
  const items = [report("current"), report("baseline", 400, 270)];
  items[1].definition.identity = "person";
  items[1].funnel.steps[0].count = 1000;
  items[1].retention.eligible = 900;
  const context = calculate(items);
  assert.equal(context.decision, "ready");
  assert.equal(context.comparison, null);
  assert.deepEqual(context.comparison_blockers, [
    "baseline uses a different measurement definition: identity",
  ]);
});

test("empty cohorts have no measured rate or rate change", () => {
  const reports = [report("current", 0, 0), report("baseline", 0, 0)];
  for (const item of reports) {
    item.funnel.steps[0].count = 0;
    item.retention.eligible = 0;
  }
  const context = calculate(reports);
  assert.equal(context.decision, "ready");
  assert.equal(context.reports[0].funnel[1].conversion_from_previous, null);
  assert.equal(context.reports[0].retention.rate, null);
  assert.equal(context.comparison.retention_change_percentage_points, null);
});

test("invalid current evidence still withholds interpretation", () => {
  const changes = [
    (r) => {
      r[0].complete = false;
    },
    (r) => {
      r[0].funnel.steps[1].count = 101;
    },
    (r) => {
      r[0].retention.returned = 81;
    },
    (r) => {
      r[1].name = "current";
    },
  ];
  for (const change of changes) {
    const rows = [report("current"), report("baseline")];
    change(rows);
    const c = calculate(rows);
    assert.equal(c.decision, "needs_more_evidence");
    assert.equal(c.comparison, null);
    assert.deepEqual(c.reports, []);
    assert.ok(c.blockers.length);
  }
});
test("baseline faults do not erase independently valid current measurements", () => {
  const changes = [
    (r) => {
      r[1].funnel.steps[1].event = "different activation";
    },
    (r) => {
      r[1].window.end_at = "2026-09-09T00:00:00Z";
    },
    (r) => {
      r[1].retention.period = "day 30";
    },
    (r) => {
      r[1].complete = false;
    },
    (r) => {
      r[1].retention.returned = 1000;
    },
  ];
  for (const change of changes) {
    const rows = [report("current"), report("baseline")];
    change(rows);
    const c = calculate(rows);
    assert.equal(c.decision, "ready");
    assert.equal(c.comparison, null);
    assert.equal(c.reports[0].funnel[1].count, 75);
    assert.ok(c.comparison_blockers.length);
    assert.deepEqual(c.blockers, []);
  }
});
test("long funnels preserve repeated events by ordered position", () => {
  const r = report("current");
  r.funnel.steps = Array.from({ length: 64 }, (_, i) => ({
    event: i % 2 ? "confirm" : "view",
    count: 128 - i,
  }));
  const c = calculate([r]);
  assert.equal(c.decision, "ready");
  assert.equal(c.reports[0].funnel.length, 64);
  assert.equal(c.reports[0].funnel[63].count, 65);
  assert.equal(c.reports[0].funnel[63].lost_from_previous, 1);
});
