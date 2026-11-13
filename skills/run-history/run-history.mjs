export function prepareHistory(inputs) {
  const objective = requiredString(inputs.objective, "objective");
  const scope = stringValue(inputs.scope) || "workspace";
  const limit = inputs.limit;
  const historyReplay = Array.isArray(inputs.runs);
  const catalogReplay = Array.isArray(inputs.catalog_items);
  if (historyReplay !== catalogReplay) {
    throw new Error("runs and catalog_items must be supplied together for replay");
  }
  if (historyReplay && (stringValue(inputs.period) || stringValue(inputs.as_of))) {
    throw new Error("Replay summarizes already-windowed projections; apply period/as_of through run.list before capturing them, or omit replay inputs for a live filtered report.");
  }
  return {
    history_context: {
      objective,
      scope,
      skill_filter: ["workspace", "all"].includes(scope) ? null : scope,
      period: stringValue(inputs.period),
      // An absolute lower bound is passed as since; a relative one such as
      // 7d is resolved by the run list against as_of.
      since: absoluteInstant(inputs.period),
      lookback: absoluteInstant(inputs.period) ? null : stringValue(inputs.period),
      as_of: stringValue(inputs.as_of),
      limit,
      path: historyReplay ? "replay" : "live",
    },
  };
}

export function finalizeHistory(inputs) {
  const prepared = requiredRecord(inputs.history_context, "history_context");
  const historyOverride = prepared.path === "replay";
  const catalogOverride = prepared.path === "replay";
  const nativeCatalog = record(inputs.authoring_context);
  const listedRuns = historyOverride
    ? records(inputs.runs)
    : records(record(inputs.run_list).runs);
  const catalogItems = catalogOverride
    ? records(inputs.catalog_items)
    : records(nativeCatalog.catalog_skills);
  const matchesSkill = (name) =>
    !prepared.skill_filter || stringValue(name)?.toLowerCase().includes(prepared.skill_filter.toLowerCase());
  const matchingRuns = historyOverride ? listedRuns.filter((row) => matchesSkill(row.skill)) : listedRuns;
  const runs = historyOverride ? matchingRuns.slice(0, prepared.limit) : matchingRuns;
  const unreadable = historyOverride ? [] : records(record(inputs.run_list).unreadable);
  const sealed = runs.filter((row) => row.state === "sealed");
  const pending = runs.filter((row) => row.state === "paused");
  const filteredCatalog = catalogItems.filter((row) => matchesSkill(row.name));
  const outcomes = countBy(sealed, (row) => stringValue(row.outcome) || "unknown");
  const terminalCount = sealed.length;
  const refusalCount = (outcomes.blocked || 0) + (outcomes.declined || 0);
  const testedCatalogEntries = filteredCatalog.filter((item) => numberValue(item.fixtures) + numberValue(item.test_cases) > 0).length;
  const untestedCatalogEntries = filteredCatalog.length - testedCatalogEntries;
  const decision = unreadable.length > 0 || (terminalCount === 0 && pending.length === 0)
    ? "needs_more_evidence" : "ready";

  return {
    history_report: {
      schema: "runx/run-history.report.v1",
      decision,
      objective: prepared.objective,
      query: {
        scope: prepared.scope,
        period: prepared.period,
        since: prepared.since,
        skill_filter: prepared.skill_filter,
        limit: prepared.limit,
      },
      sources: {
        history: historyOverride ? "supplied_replay" : "run.list",
        catalog: catalogOverride ? "supplied_replay" : "runx.skill.inspect",
      },
      runs: {
        terminal_count: terminalCount,
        pending_count: pending.length,
        unreadable,
        outcomes,
        completed_rate: rate(outcomes.completed || 0, terminalCount),
        refusal_rate: rate(refusalCount, terminalCount),
        top_subjects: topCounts(sealed, (row) => stringValue(row.skill) || "unknown", 10),
      },
      catalog: {
        entry_count: filteredCatalog.length,
        tested_entry_count: testedCatalogEntries,
        untested_entry_count: untestedCatalogEntries,
        coverage_rate: rate(testedCatalogEntries, filteredCatalog.length),
      },
      recommendations: recommendations({ decision, outcomes, refusalCount, untestedCatalogEntries, unreadableCount: unreadable.length }),
      limitations: [
        "The run list exposes each run's state and sealed outcome, not its execution bodies.",
        "Catalog coverage proves declared fixtures or inline cases, not live provider behavior.",
        "Rates describe only this bounded selection of sealed runs; active states are outside the sealed/paused metrics.",
        ...(unreadable.length > 0 ? [`${unreadable.length} unreadable run record${unreadable.length === 1 ? " remains" : "s remain"} listed and excluded from outcome rates.`] : []),
        ...(!historyOverride && (prepared.skill_filter || prepared.period || prepared.as_of) ? ["Native skill/time filters cannot attribute unreadable records to this selection; this report does not establish their absence."] : []),
        ...(historyOverride ? ["Replay preserves the supplied row order and performs no temporal filtering; the source must already have the intended native window."] : []),
        ...(runs.length + unreadable.length >= prepared.limit ? [`The ${prepared.limit}-run bound was reached; older matching runs may be omitted.`] : []),
        ...(prepared.as_of ? [`The native history window ends at ${prepared.as_of}.`] : []),
      ],
    },
  };
}

function recommendations({ decision, outcomes, refusalCount, untestedCatalogEntries, unreadableCount }) {
  if (decision === "needs_more_evidence" && unreadableCount === 0) {
    return [{ lane: "none", action: "Run governed skills before treating an empty ledger as platform health." }];
  }
  const items = [];
  if (unreadableCount > 0) {
    items.push({ lane: "none", action: `Inspect the ${unreadableCount} unreadable run record${unreadableCount === 1 ? "" : "s"} with a compatible runtime before relying on this partial history.` });
  }
  if ((outcomes.failed || 0) > 0 || (outcomes.timed_out || 0) > 0) {
    items.push({ lane: "diagnose-skill-run", action: "Diagnose representative failed or timed-out runs and route bounded fixes through skill-build improve." });
  }
  if (refusalCount > 0) {
    items.push({ lane: "receipt-audit", action: "Sample the receipts of blocked or declined runs to confirm the governance boundary is behaving as intended." });
  }
  if (untestedCatalogEntries > 0) {
    items.push({ lane: "skill-build test", action: `Add public-contract coverage to ${untestedCatalogEntries} catalog entr${untestedCatalogEntries === 1 ? "y" : "ies"} with no declared fixture or inline case.` });
  }
  return items;
}

function countBy(rows, keyFor) {
  const counts = new Map();
  for (const row of rows) {
    const key = keyFor(row);
    counts.set(key, (counts.get(key) || 0) + 1);
  }
  return Object.fromEntries(counts);
}

function topCounts(rows, keyFor, limit) {
  return Object.entries(countBy(rows, keyFor))
    .map(([subject, count]) => ({ subject, count }))
    .sort((left, right) => right.count - left.count || left.subject.localeCompare(right.subject))
    .slice(0, limit);
}

function rate(numerator, denominator) {
  return denominator === 0 ? null : Number((numerator / denominator).toFixed(4));
}


function absoluteInstant(value) {
  const text = stringValue(value);
  return text && /[Tt]/u.test(text) ? text : null;
}

function numberValue(value) {
  return Number.isFinite(value) ? Math.max(0, Math.trunc(value)) : 0;
}

function records(value) {
  return Array.isArray(value) ? value.map(record) : [];
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function requiredRecord(value, field) {
  const parsed = record(value);
  if (Object.keys(parsed).length === 0) throw new Error(`${field} must be a non-empty object`);
  return parsed;
}

function requiredString(value, field) {
  const parsed = stringValue(value);
  if (!parsed) throw new Error(`${field} must be a non-empty string`);
  return parsed;
}

function stringValue(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}
