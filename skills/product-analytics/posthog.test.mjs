import assert from "node:assert/strict";
import test from "node:test";
import { preparePosthog, bindPosthog, mergePosthog } from "./posthog.mjs";
// Domain tests use the same JSON value; the package test separately proves native parseData.
globalThis.Runx = { parseData: (source) => JSON.parse(source) };
const example = {
  schema: "runx.provider.operation.v1",
  status: "success",
  provider: "posthog",
  operation: "insight.read",
  target: "https://us.posthog.com/project/123/insights/launch01",
  transport: "test",
  readback_uri: "https://us.posthog.com/project/123/insights/launch01",
  principal_uri: "test:analytics",
  account_id: "test",
  result: {
    insight: {
      id: 42,
      short_id: "launch01",
      name: "Activation",
      query: {
        kind: "InsightVizNode",
        source: {
          kind: "FunnelsQuery",
          dateRange: {
            date_from: "-7d",
          },
          series: [
            {
              kind: "EventsNode",
              event: "signed_up",
            },
            {
              kind: "EventsNode",
              event: "invited_teammate",
            },
          ],
          funnelsFilter: {
            funnelOrderType: "ordered",
            funnelVizType: "steps",
            funnelWindowInterval: 7,
            funnelWindowIntervalUnit: "day",
          },
        },
      },
      result: [
        {
          order: 0,
          name: "signed_up",
          count: 100,
        },
        {
          order: 1,
          name: "invited_teammate",
          count: 65,
        },
      ],
      last_refresh: "2026-11-01T00:00:00Z",
      timezone: "UTC",
      resolved_date_range: {
        date_from: "2026-10-24T00:00:00Z",
        date_to: "2026-10-31T00:00:00Z",
      },
      hasMore: false,
      refreshing: false,
    },
  },
};
const clone = () => structuredClone(example);
const prepare = (packet, extra = {}) =>
  preparePosthog({
    name: "current",
    artifact: {
      schema: "runx.project_artifact.v1",
      uri: "runx:project-artifact:sha256:" + "a".repeat(64),
      digest: "sha256:" + "a".repeat(64),
      bytes: 100,
      media_type: "application/json",
      path: "/synthetic/insight.json",
    },
    page: {
      offset: 0,
      eof: true,
      data: JSON.stringify(packet),
      whole_digest: "sha256:" + "a".repeat(64),
    },
    ...extra,
  });
test("normalizes exact ordered counts and provider-resolved cache identity", () => {
  const out = prepare(clone());
  assert.deepEqual(out.source_admission.report.funnel.steps, [
    { event: "signed_up", count: 100 },
    { event: "invited_teammate", count: 65 },
  ]);
  assert.equal(out.source_admission.report.observed_at, "2026-11-01T00:00:00Z");
  assert.equal(out.query_semantics.query.dateRange, undefined);
  const bound = bindPosthog({
    source_admission: out.source_admission,
    semantic_digest: "sha256:" + "b".repeat(64),
  });
  assert.match(bound.source_admission.report.definition.filters, /bbbb/);
});
test("does not replace missing freshness or relative-date resolution with read time", () => {
  for (const field of ["last_refresh", "resolved_date_range", "timezone"]) {
    const packet = clone();
    delete packet.result.insight[field];
    const out = prepare(packet);
    assert.equal(out.source_admission.report, null);
    assert.ok(out.source_admission.blockers.length);
  }
});
test("retains fractional provider estimates and actual group identity", () => {
  const packet = clone(),
    q = packet.result.insight.query.source;
  q.samplingFactor = 0.3;
  q.aggregation_group_type_index = 2;
  packet.result.insight.result[0].count = 100.25;
  const out = prepare(packet).source_admission;
  assert.equal(out.report.funnel.steps[0].count, 100.25);
  assert.equal(out.report.estimated, true);
  assert.equal(out.report.definition.identity, "group:2");
});
test("does not flatten breakdowns or query comparison series", () => {
  for (const modifier of [
    { breakdownFilter: { breakdown: "plan" } },
    { compareFilter: { compare: true } },
  ]) {
    const packet = clone();
    Object.assign(packet.result.insight.query.source, modifier);
    assert.equal(prepare(packet).source_admission.report, null);
  }
});
test("dynamic action definitions withhold comparison without erasing the current measurement", () => {
  const packet = clone();
  packet.result.insight.query.source.series[1] = {
    kind: "ActionsNode",
    id: 7,
    name: "Activated",
  };
  const report = prepare(packet).source_admission.report;
  assert.ok(report);
  assert.equal(report.comparison_eligible, false);
});
function retention() {
  const packet = clone(),
    insight = packet.result.insight;
  insight.query = {
    kind: "RetentionQuery",
    retentionFilter: {
      period: "Day",
      targetEntity: { id: "signed_up", type: "events" },
      returningEntity: { id: "active", type: "events" },
    },
  };
  insight.resolved_date_range = {
    date_from: "2026-10-01T00:00:00Z",
    date_to: "2026-10-10T00:00:00Z",
  };
  insight.result = Array.from({ length: 10 }, (_, i) => ({
    date: "2026-10-" + String(i + 1).padStart(2, "0") + "T00:00:00Z",
    values: [
      { count: 100, label: "Day 0" },
      { count: i < 8 ? 50 : 0, label: "Day 1" },
      { count: i < 7 ? 40 : 0, label: "Day 2" },
    ],
  }));
  return packet;
}
test("retention excludes immature future zeroes and weights cohort counts", () => {
  const out = prepare(retention()).source_admission;
  assert.deepEqual(out.report.retention, {
    period: "Day 1",
    eligible: 800,
    returned: 400,
  });
  assert.match(out.notes.join(" "), /2 recent/);
});
test("empty eligible maturity is unknown rather than zero-percent retention", () => {
  const packet = retention();
  packet.result.insight.result = packet.result.insight.result.slice(-2);
  const out = prepare(packet).source_admission;
  assert.equal(out.report.retention.eligible, 0);
  assert.equal(out.report.retention.returned, 0);
});
test("uses actual timezone/calendar boundaries across DST rather than fixed calendar days", () => {
  const packet = retention();
  packet.result.insight.timezone = "America/New_York";
  packet.result.insight.result = [
    {
      date: "2026-10-31T00:00:00-04:00",
      values: [
        { count: 10, label: "Day 0" },
        { count: 5, label: "Day 1" },
      ],
    },
    {
      date: "2026-11-01T00:00:00-04:00",
      values: [
        { count: 10, label: "Day 0" },
        { count: 5, label: "Day 1" },
      ],
    },
    {
      date: "2026-11-02T00:00:00-05:00",
      values: [
        { count: 10, label: "Day 0" },
        { count: 0, label: "Day 1" },
      ],
    },
  ];
  packet.result.insight.resolved_date_range = {
    date_from: "2026-10-31T00:00:00-04:00",
    date_to: "2026-11-02T00:00:00-05:00",
  };
  packet.result.insight.last_refresh = "2026-11-03T00:00:00Z";
  assert.equal(prepare(packet).source_admission.report.retention.eligible, 10);
});
test("daily rolling maturity waits for the latest cohort entry full follow-up", () => {
  const packet = retention();
  packet.result.insight.query.retentionFilter.timeWindowMode =
    "24_hour_windows";
  packet.result.insight.result[2].date = "2026-10-03T01:00:00Z";
  packet.result.insight.resolved_date_range.date_to = "2026-10-03T00:30:00Z";
  assert.equal(prepare(packet).source_admission.report.retention.eligible, 100);
});
test("missing mature cells and cumulative/property aggregates are explicit gaps", () => {
  for (const change of [
    (p) => delete p.result.insight.result[0].values[1],
    (p) => (p.result.insight.query.retentionFilter.cumulative = true),
    (p) => (p.result.insight.query.retentionFilter.aggregationType = "sum"),
  ]) {
    const packet = retention();
    change(packet);
    assert.equal(prepare(packet).source_admission.report, null);
  }
});
test("failed baseline admission preserves current and its exact source", () => {
  const current = prepare(clone()).source_admission,
    bad = clone();
  delete bad.result.insight.last_refresh;
  const baseline = prepare(bad, { name: "baseline" }).source_admission;
  const result = mergePosthog({ current, baseline });
  assert.equal(result.reports.length, 1);
  assert.equal(result.reports[0].source_digest, current.report.source_digest);
  assert.equal(result.source_admissions.length, 2);
  assert.ok(result.collection_notes.some((s) => s.startsWith("baseline:")));
});

test("the complete native artifact descriptor survives normalization", () => {
  const out = prepare(clone());
  assert.equal(
    out.source_admission.source.artifact.path,
    "/synthetic/insight.json",
  );
  assert.equal(
    out.source_admission.source.artifact.digest,
    out.source_admission.report.source_digest,
  );
});
test("a partly elapsed query window is explicit and not compared as complete history", () => {
  const packet = clone();
  packet.result.insight.last_refresh = "2026-10-30T00:00:00Z";
  const out = prepare(packet).source_admission;
  assert.ok(out.report);
  assert.equal(out.report.comparison_eligible, false);
  assert.match(out.notes.join(" "), /observed so far/);
});

test("wrong source kinds remain an explicit gap with the actual source identity", () => {
  const packet = clone();
  packet.provider = "github";
  const out = prepare(packet).source_admission;
  assert.equal(out.report, null);
  assert.equal(out.source.provider, "github");
  const empty = prepare(null).source_admission;
  assert.equal(empty.report, null);
  assert.equal(empty.source.target, null);
});

test("empty breakdown settings do not make an ordinary saved chart unusable", () => {
  for (const packet of [clone(), retention()]) {
    packet.result.insight.query.kind === "InsightVizNode"
      ? (packet.result.insight.query.source.breakdownFilter = {
          breakdown: null,
          breakdowns: [],
        })
      : (packet.result.insight.query.breakdownFilter = {
          breakdown: null,
          breakdowns: [],
        });
    assert.ok(prepare(packet).source_admission.report);
  }
});
test("the analyst receives actual query scope and title rather than a hash alone", () => {
  const packet = clone();
  packet.result.insight.description = "New paid-team activation";
  packet.result.insight.query.source.properties = [
    { key: "plan", operator: "exact", value: "paid", type: "person" },
  ];
  const report = bindPosthog({
    source_admission: prepare(packet).source_admission,
    semantic_digest: "sha256:" + "b".repeat(64),
  }).source_admission.report;
  assert.equal(report.measurement_context.title, "Activation");
  assert.equal(
    report.measurement_context.description,
    "New paid-team activation",
  );
  assert.deepEqual(
    report.measurement_context.query.properties,
    packet.result.insight.query.source.properties,
  );
  assert.equal(report.measurement_context.query.response, undefined);
  assert.equal(
    report.measurement_context.source_artifact.path,
    "/synthetic/insight.json",
  );
});
