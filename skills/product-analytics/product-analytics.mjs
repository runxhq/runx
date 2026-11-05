// Counts and comparability are domain decisions, not model arithmetic.
export function calculateMetrics({
  objective,
  reports,
  collection_notes = [],
}) {
  const blockers = [],
    comparisonBlockers = [],
    valid = new Map();
  const names = reports.map((r) => r.name);
  if (
    names.filter((n) => n === "current").length !== 1 ||
    new Set(names).size !== names.length
  )
    blockers.push(
      "one current report and at most one baseline report are required",
    );
  for (const report of reports) {
    const issues = [],
      { name, window, funnel, retention } = report;
    const start = Date.parse(window.start_at),
      end = Date.parse(window.end_at);
    if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start)
      issues.push(`${name}: invalid report window`);
    if (!report.complete) issues.push(`${name}: report is incomplete`);
    if (!funnel && !retention)
      issues.push(`${name}: no funnel or retention counts supplied`);
    // Step position distinguishes repeated events in an ordered journey.
    if (funnel)
      for (let i = 1; i < funnel.steps.length; i++) {
        if (funnel.steps[i].count > funnel.steps[i - 1].count)
          issues.push(
            `${name}: ordered funnel counts increase at step ${i + 1} (${funnel.steps[i].event})`,
          );
      }
    if (retention && retention.returned > retention.eligible)
      issues.push(`${name}: retained count exceeds eligible cohort`);
    if (issues.length) {
      (name === "current" ? blockers : comparisonBlockers).push(...issues);
      continue;
    }
    valid.set(name, {
      name,
      window,
      definition: report.definition,
      observed_at: report.observed_at,
      source_digest: report.source_digest,
      complete: report.complete,
      ...(report.measurement_context
        ? { measurement_context: report.measurement_context }
        : {}),
      estimated: report.estimated ?? false,
      comparison_eligible: report.comparison_eligible ?? true,
      funnel: funnel
        ? funnel.steps.map((step, index, steps) => ({
            event: step.event,
            count: step.count,
            conversion_from_entry: rate(step.count, steps[0].count),
            conversion_from_previous: index
              ? rate(step.count, steps[index - 1].count)
              : null,
            lost_from_previous: index
              ? steps[index - 1].count - step.count
              : null,
          }))
        : [],
      retention: retention
        ? {
            period: retention.period,
            eligible: retention.eligible,
            returned: retention.returned,
            rate: rate(retention.returned, retention.eligible),
          }
        : null,
    });
  }
  const current = reports.find((r) => r.name === "current"),
    baseline = reports.find((r) => r.name === "baseline");
  let comparison = null;
  if (current && baseline && valid.has("current") && valid.has("baseline")) {
    if (
      current.comparison_eligible === false ||
      baseline.comparison_eligible === false
    )
      comparisonBlockers.push(
        "source definitions lack the historical evidence needed for comparison",
      );
    if ((current.estimated ?? false) !== (baseline.estimated ?? false))
      comparisonBlockers.push("comparison mixes exact and estimated counts");
    if (
      Date.parse(current.window.end_at) -
        Date.parse(current.window.start_at) !==
      Date.parse(baseline.window.end_at) - Date.parse(baseline.window.start_at)
    )
      comparisonBlockers.push(
        "comparison windows have different elapsed durations",
      );
    if (
      Date.parse(baseline.window.end_at) > Date.parse(current.window.start_at)
    )
      comparisonBlockers.push(
        "baseline overlaps or follows the current window",
      );
    const fields = [
      ...new Set([
        ...Object.keys(current.definition),
        ...Object.keys(baseline.definition),
      ]),
    ].sort();
    for (const field of fields)
      if (current.definition[field] !== baseline.definition[field])
        comparisonBlockers.push(
          `baseline uses a different measurement definition: ${field}`,
        );
    const currentEvents = current.funnel?.steps.map((s) => s.event) ?? [],
      baselineEvents = baseline.funnel?.steps.map((s) => s.event) ?? [];
    if (JSON.stringify(currentEvents) !== JSON.stringify(baselineEvents))
      comparisonBlockers.push(
        "baseline uses a different ordered funnel definition",
      );
    if (
      (current.retention?.period ?? null) !==
      (baseline.retention?.period ?? null)
    )
      comparisonBlockers.push("baseline uses a different retention period");
    if (!blockers.length && !comparisonBlockers.length) {
      const c = valid.get("current"),
        b = valid.get("baseline");
      comparison = {
        funnel: c.funnel.map((step, i) => ({
          event: step.event,
          conversion_change_percentage_points: difference(
            step.conversion_from_entry,
            b.funnel[i].conversion_from_entry,
          ),
        })),
        retention_change_percentage_points: c.retention
          ? difference(c.retention.rate, b.retention.rate)
          : null,
      };
    }
  }
  return {
    metric_context: {
      decision: blockers.length ? "needs_more_evidence" : "ready",
      objective,
      reports: blockers.length
        ? []
        : reports
            .filter((r) => valid.has(r.name))
            .map((r) => valid.get(r.name)),
      comparison,
      source_digests: [...new Set(reports.map((r) => r.source_digest))],
      blockers,
      comparison_blockers: comparisonBlockers,
      collection_notes,
    },
  };
}
function rate(numerator, denominator) {
  return denominator === 0 ? null : numerator / denominator;
}
function difference(current, baseline) {
  return current === null || baseline === null
    ? null
    : (current - baseline) * 100;
}
