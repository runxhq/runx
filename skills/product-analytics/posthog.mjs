// PostHog saved-query interpretation. Native operations own IO and artifact identity.
const object = (value) =>
  value && typeof value === "object" && !Array.isArray(value) ? value : {};
const present = (value) => value !== null && value !== undefined;
const count = (value) =>
  typeof value === "number" &&
  Number.isFinite(value) &&
  value >= 0 &&
  value <= Number.MAX_SAFE_INTEGER;
const stamp = (value) =>
  typeof value === "string" &&
  /(?:Z|[+-]\d{2}:\d{2})$/.test(value) &&
  Number.isFinite(Date.parse(value));
const hasBreakdown = (filter) =>
  present(filter?.breakdown) ||
  (present(filter?.breakdowns) &&
    (!Array.isArray(filter.breakdowns) || filter.breakdowns.length > 0));
const eventName = (node) =>
  String(node.event ?? node.name ?? node.id ?? node.kind ?? "unnamed event");
function externalDefinition(value) {
  if (Array.isArray(value)) return value.some(externalDefinition);
  if (!value || typeof value !== "object") return false;
  return (
    value.kind === "ActionsNode" ||
    value.type === "actions" ||
    value.type === "cohort" ||
    Object.values(value).some(externalDefinition)
  );
}
function orderedCounts(query, insight, blockers) {
  const filter = object(query.funnelsFilter),
    series = query.series;
  if (
    (filter.funnelOrderType ?? "ordered") !== "ordered" ||
    (filter.funnelVizType ?? "steps") !== "steps"
  )
    blockers.push(
      "Saved funnel is not an ordered step-count funnel; use an explicit prepared measurement for its actual semantics.",
    );
  if (
    hasBreakdown(query.breakdownFilter) ||
    query.compareFilter?.compare ||
    query.compareFilter?.compare_to
  )
    blockers.push(
      "Saved funnel has breakdown or comparison series; select and prepare the exact series instead of flattening them.",
    );
  const rows = insight.result;
  if (
    !Array.isArray(series) ||
    series.length < 2 ||
    !Array.isArray(rows) ||
    rows.length !== series.length ||
    rows.some((row) => Array.isArray(row) || !count(row?.count))
  ) {
    blockers.push(
      "Cached funnel does not contain one bounded count for every ordered query step.",
    );
    return null;
  }
  if (rows.some((row, i) => row.order !== i))
    blockers.push("Cached funnel step order does not match the saved query.");
  return {
    steps: rows.map((row, i) => ({
      event: eventName(series[i]),
      count: row.count,
    })),
  };
}
function matureRetention(query, insight, interval, cutoff, blockers, notes) {
  const filter = object(query.retentionFilter),
    allRows = insight.result;
  if (
    hasBreakdown(query.breakdownFilter) ||
    !Array.isArray(allRows) ||
    allRows.some((row) => present(row.breakdown_value))
  ) {
    blockers.push(
      "Retention breakdowns need an explicit prepared cohort selection; unrelated cohorts are not summed.",
    );
    return null;
  }
  if ((filter.aggregationType ?? "count") !== "count") {
    blockers.push(
      "Property sums and averages are not eligible-entity retention counts.",
    );
    return null;
  }
  if (filter.cumulative) {
    blockers.push(
      "Cumulative retention requires complete follow-up across its entire declared horizon; prepare that cohort explicitly.",
    );
    return null;
  }
  const rows = [...allRows].sort(
    (a, b) => Date.parse(a.date) - Date.parse(b.date),
  );
  if (
    rows.some(
      (row, i) =>
        !stamp(row.date) ||
        (i && Date.parse(row.date) <= Date.parse(rows[i - 1].date)) ||
        !Array.isArray(row.values) ||
        !count(row.values[0]?.count),
    )
  ) {
    blockers.push(
      "Retention cohorts lack exact, unique provider boundaries or eligible counts.",
    );
    return null;
  }
  const brackets = filter.retentionCustomBrackets;
  const custom = Array.isArray(brackets) && brackets.length > 0;
  if (
    custom &&
    ((filter.period ?? "Day") !== "Day" ||
      brackets.some((width) => !Number.isInteger(width) || width <= 0) ||
      interval > brackets.length)
  ) {
    blockers.push(
      "Custom retention requires positive integral day brackets containing the selected interval.",
    );
    return null;
  }
  const boundaryDistance = custom
    ? 1 + brackets.slice(0, interval).reduce((a, b) => a + b, 0)
    : interval + 1;
  let eligible = 0,
    returned = 0,
    included = 0,
    omitted = 0;
  const rolling = filter.timeWindowMode === "24_hour_windows";
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i],
      cell = row.values[interval];
    // Calendar boundaries come from the provider. A rolling daily window starts
    // at the latest possible cohort entry, not at the first entry's midnight.
    let end = rows[i + boundaryDistance]?.date;
    if (rolling && (filter.period ?? "Day") === "Day") {
      const cohortEnd = rows[i + 1]?.date;
      if (cohortEnd)
        end = new Date(
          Date.parse(cohortEnd) + (boundaryDistance - 1) * 86400000,
        ).toISOString();
    } else if (rolling) {
      blockers.push(
        "Non-daily rolling retention needs explicit source-resolved maturity bounds.",
      );
      return null;
    }
    if (!end || Date.parse(end) > cutoff) {
      omitted++;
      continue;
    }
    if (!cell || !count(cell.count) || cell.count > row.values[0].count) {
      blockers.push(
        "Mature retention cell is missing or exceeds its own eligible cohort.",
      );
      return null;
    }
    eligible += row.values[0].count;
    returned += cell.count;
    included++;
    if (!count(eligible) || !count(returned)) {
      blockers.push("Retention aggregate exceeds safe numeric range.");
      return null;
    }
  }
  const label = rows.find((row) => row.values[interval])?.values[interval]
    .label;
  if (typeof label !== "string" || !label) {
    blockers.push(
      "Selected retention interval is absent from the provider result.",
    );
    return null;
  }
  notes.push(
    "Retention uses " +
      included +
      " fully observed cohort rows; " +
      omitted +
      " recent or boundary-unresolved rows are omitted. Zero eligible entities produce an unknown rate.",
  );
  return { period: label, eligible, returned };
}
export function preparePosthog({ page, artifact, name, return_interval = 1 }) {
  const blockers = [],
    notes = [];
  const admission = {
    name,
    report: null,
    blockers,
    notes,
    source: {
      artifact,
      artifact_digest: page.whole_digest,
      provider: null,
      target: null,
      observed_at: null,
      readback_uri: null,
    },
  };
  const finish = (query = {}) => ({
    source_admission: admission,
    query_semantics: query,
  });
  if (!page.eof || page.offset !== 0 || typeof page.data !== "string") {
    blockers.push(
      "Provider evidence exceeds this bounded control read; retain its artifact and prepare the selected aggregate through a source owner.",
    );
    return finish();
  }
  const packet = object(Runx.parseData(page.data, "json"));
  admission.source.provider =
    typeof packet.provider === "string" ? packet.provider : null;
  admission.source.target =
    typeof packet.target === "string" ? packet.target : null;
  admission.source.readback_uri =
    typeof packet.readback_uri === "string" ? packet.readback_uri : null;
  if (
    packet.schema !== "runx.provider.operation.v1" ||
    packet.status !== "success" ||
    packet.provider !== "posthog" ||
    packet.operation !== "insight.read"
  ) {
    blockers.push("Expected a completed PostHog insight.read provider packet.");
    return finish();
  }
  const insight = object(packet.result?.insight),
    rawQuery = object(insight.query),
    query =
      rawQuery.kind === "InsightVizNode" ? object(rawQuery.source) : rawQuery;
  admission.source.target =
    typeof packet.target === "string" ? packet.target : null;
  admission.source.readback_uri =
    typeof packet.readback_uri === "string" ? packet.readback_uri : null;
  admission.source.observed_at =
    typeof insight.last_refresh === "string" ? insight.last_refresh : null;
  const range = object(insight.resolved_date_range);
  if (!stamp(insight.last_refresh))
    blockers.push(
      "Cached computation time is missing; HTTP read time cannot replace last_refresh.",
    );
  if (
    !stamp(range.date_from) ||
    !stamp(range.date_to) ||
    Date.parse(range.date_to) <= Date.parse(range.date_from)
  )
    blockers.push(
      "Provider-resolved absolute report window is missing or invalid.",
    );
  if (typeof insight.timezone !== "string" || !insight.timezone)
    blockers.push("Provider measurement timezone is missing.");
  if (
    insight.hasMore === true ||
    insight.refreshing === true ||
    insight.query_status?.complete === false ||
    insight.query_status?.error
  )
    blockers.push("Cached result is incomplete, refreshing, or failed.");
  if (query.kind !== "FunnelsQuery" && query.kind !== "RetentionQuery")
    blockers.push(
      "Saved insight is not a funnel or retention query; use prepared reports for another measurement kind.",
    );
  if (blockers.length) return finish();
  // Date windows and cached responses are observations, not measurement identity.
  const { dateRange, response, tags, ...semantics } = query;
  const project = String(packet.target ?? "").split("/insights/")[0];
  const identity = present(query.aggregation_group_type_index)
    ? "group:" + query.aggregation_group_type_index
    : "person";
  const customIdentity =
    query.funnelsFilter?.customAggregationTarget ||
    query.retentionFilter?.customAggregationTarget;
  const report = {
    name,
    window: { start_at: range.date_from, end_at: range.date_to },
    definition: {
      product: project,
      identity: customIdentity ? "custom aggregation" : identity,
      time_zone: insight.timezone,
      cohort: "Saved query cohort awaiting native semantic binding",
      filters: "Saved query filters awaiting native semantic binding",
      funnel_rule: "not measured",
      retention_rule: "not measured",
    },
    source_digest: page.whole_digest,
    observed_at: insight.last_refresh,
    complete: true,
    estimated: present(query.samplingFactor) && query.samplingFactor !== 1,
    comparison_eligible: !externalDefinition(query),
  };
  report.measurement_context = {
    provider: "posthog",
    title:
      typeof insight.name === "string"
        ? insight.name
        : typeof insight.derived_name === "string"
          ? insight.derived_name
          : "",
    description:
      typeof insight.description === "string" ? insight.description : "",
    query: semantics,
    source_artifact: artifact,
  };
  if (Date.parse(insight.last_refresh) < Date.parse(range.date_to)) {
    report.comparison_eligible = false;
    notes.push(
      "The declared window extends beyond the cached observation. Counts are observed so far; a full-window comparison is withheld.",
    );
  }
  if (report.estimated)
    notes.push(
      "Provider sampling is enabled. Counts and rates are estimates; no integer rounding is applied.",
    );
  if (!report.comparison_eligible)
    notes.push(
      "Named actions or dynamic cohorts lack historical definition snapshots; cross-period comparison is withheld.",
    );
  if (query.kind === "FunnelsQuery")
    report.funnel = orderedCounts(query, insight, blockers);
  else
    report.retention = matureRetention(
      query,
      insight,
      return_interval,
      Math.min(Date.parse(range.date_to), Date.parse(insight.last_refresh)),
      blockers,
      notes,
    );
  notes.push(
    "Saved cached result observed at " +
      insight.last_refresh +
      "; reading it does not refresh it or prove current product state.",
  );
  if (!blockers.length) admission.report = report;
  return finish({
    project,
    principal: packet.principal_uri ?? null,
    account_id: packet.account_id ?? null,
    query: semantics,
    selected_return_interval:
      query.kind === "RetentionQuery" ? return_interval : null,
  });
}
export function bindPosthog({ source_admission, semantic_digest }) {
  const result = JSON.parse(JSON.stringify(source_admission));
  if (result.report) {
    const d = result.report.definition;
    d.cohort = "Saved query cohort; semantic definition " + semantic_digest;
    d.filters =
      "Saved query properties, test-account filtering, sampling and aggregation " +
      semantic_digest;
    if (result.report.funnel)
      d.funnel_rule =
        "Ordered same-entity funnel; exact saved semantics " + semantic_digest;
    if (result.report.retention)
      d.retention_rule =
        "Fully observed cohort rows at " +
        result.report.retention.period +
        "; exact saved semantics " +
        semantic_digest;
  }
  return { source_admission: result };
}
export function mergePosthog({ current, baseline, collection_notes = [] }) {
  const sources = [current, baseline].filter(Boolean);
  return {
    reports: sources.flatMap((item) => (item.report ? [item.report] : [])),
    collection_notes: [
      ...collection_notes,
      ...sources.flatMap((item) => [
        ...item.blockers.map((text) => item.name + ": " + text),
        ...item.notes.map((text) => item.name + ": " + text),
      ]),
    ],
    source_admissions: sources,
  };
}

export function selectPosthog({ selection }) {
  return {
    has_current: selection.current !== null,
    mode:
      selection.current === null
        ? "none"
        : selection.baseline === null
          ? "current"
          : "both",
  };
}
