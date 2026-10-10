// Counts and comparability are domain decisions, not model arithmetic.
export function calculateMetrics({ objective, reports }) {
  const blockers = [];
  const names = reports.map(({ name }) => name);
  if (names.filter((name) => name === "current").length !== 1 || new Set(names).size !== names.length) {
    blockers.push("one current report and at most one baseline report are required");
  }
  const metrics = reports.map((report) => {
    const { name, window, funnel, retention } = report;
    const start = Date.parse(window.start_at);
    const end = Date.parse(window.end_at);
    if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start) blockers.push(`${name}: invalid report window`);
    if (!report.complete) blockers.push(`${name}: report is incomplete`);
    if (!funnel && !retention) blockers.push(`${name}: no funnel or retention counts supplied`);
    if (funnel) {
      const events = funnel.steps.map(({ event }) => event);
      if (new Set(events).size !== events.length) blockers.push(`${name}: funnel event definitions repeat`);
      for (let index = 1; index < funnel.steps.length; index++) {
        if (funnel.steps[index].count > funnel.steps[index - 1].count) {
          blockers.push(`${name}: ordered funnel counts increase at ${funnel.steps[index].event}`);
        }
      }
    }
    if (retention && retention.returned > retention.eligible) blockers.push(`${name}: retained count exceeds eligible cohort`);
    return {
      name,
      window,
      definition: report.definition,
      observed_at: report.observed_at,
      source_digest: report.source_digest,
      complete: report.complete,
      funnel: funnel ? funnel.steps.map((step, index, steps) => ({
        event: step.event,
        count: step.count,
        conversion_from_entry: rate(step.count, steps[0].count),
        conversion_from_previous: index ? rate(step.count, steps[index - 1].count) : null,
        lost_from_previous: index ? steps[index - 1].count - step.count : null,
      })) : [],
      retention: retention ? {
        period: retention.period,
        eligible: retention.eligible,
        returned: retention.returned,
        rate: rate(retention.returned, retention.eligible),
      } : null,
    };
  });
  const current = reports.find(({ name }) => name === "current");
  const baseline = reports.find(({ name }) => name === "baseline");
  let comparison = null;
  if (current && baseline) {
    if (Date.parse(current.window.end_at) - Date.parse(current.window.start_at)
        !== Date.parse(baseline.window.end_at) - Date.parse(baseline.window.start_at)) {
      blockers.push("comparison windows have different elapsed durations");
    }
    if (Date.parse(baseline.window.end_at) > Date.parse(current.window.start_at)) blockers.push("baseline overlaps or follows the current window");
    const definitionFields = [...new Set([...Object.keys(current.definition), ...Object.keys(baseline.definition)])].sort();
    for (const field of definitionFields) {
      if (current.definition[field] !== baseline.definition[field]) blockers.push(`baseline uses a different measurement definition: ${field}`);
    }
    const currentEvents = current.funnel?.steps.map(({ event }) => event) ?? [];
    const baselineEvents = baseline.funnel?.steps.map(({ event }) => event) ?? [];
    if (JSON.stringify(currentEvents) !== JSON.stringify(baselineEvents)) blockers.push("baseline uses a different ordered funnel definition");
    if ((current.retention?.period ?? null) !== (baseline.retention?.period ?? null)) blockers.push("baseline uses a different retention period");
    if (blockers.length === 0) {
      const currentMetrics = metrics.find(({ name }) => name === "current");
      const baselineMetrics = metrics.find(({ name }) => name === "baseline");
      comparison = {
        funnel: currentMetrics.funnel.map((step, index) => ({
          event: step.event,
          conversion_change_percentage_points: difference(step.conversion_from_entry, baselineMetrics.funnel[index].conversion_from_entry),
        })),
        retention_change_percentage_points: currentMetrics.retention
          ? difference(currentMetrics.retention.rate, baselineMetrics.retention.rate) : null,
      };
    }
  }
  return { metric_context: {
    decision: blockers.length ? "needs_more_evidence" : "ready",
    objective,
    reports: blockers.length ? [] : metrics,
    comparison,
    source_digests: [...new Set(reports.map(({ source_digest }) => source_digest))],
    blockers,
  } };
}

function rate(numerator, denominator) {
  return denominator === 0 ? null : numerator / denominator;
}

function difference(current, baseline) {
  return current === null || baseline === null ? null : (current - baseline) * 100;
}
