export function prepareHistory(inputs) {
  const objective = requiredString(inputs.objective, "objective");
  const scope = stringValue(inputs.scope) || "workspace";
  const limit = boundedLimit(inputs.limit);
  const historyReplay = Array.isArray(inputs.runs);
  const catalogReplay = Array.isArray(inputs.catalog_items);
  if (historyReplay !== catalogReplay) {
    throw new Error("runs and catalog_items must be supplied together for replay");
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
    ? records(inputs.runs).slice(0, prepared.limit)
    : records(record(inputs.run_list).runs);
  const catalogItems = catalogOverride
    ? records(inputs.catalog_items)
    : records(nativeCatalog.catalog_skills);
  const matchesSkill = (name) =>
    !prepared.skill_filter || stringValue(name)?.toLowerCase().includes(prepared.skill_filter.toLowerCase());
  const runs = listedRuns.filter((row) => matchesSkill(row.skill));
  const sealed = runs.filter((row) => row.state === "sealed");
  const pending = runs.filter((row) => row.state === "paused");
  const filteredCatalog = catalogItems.filter((row) => matchesSkill(row.name));
  const statuses = countBy(sealed, (row) => stringValue(row.disposition) || "unknown");
  const terminalCount = sealed.length;
  const refusalCount = (statuses.blocked || 0) + (statuses.declined || 0);
  const testedCatalogEntries = filteredCatalog.filter((item) => numberValue(item.fixtures) + numberValue(item.harness_cases) > 0).length;
  const untestedCatalogEntries = filteredCatalog.length - testedCatalogEntries;
  const decision = terminalCount === 0 && pending.length === 0 ? "needs_more_evidence" : "ready";

  return {
    history_report: {
      schema: "runx.history_report.v1",
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
        statuses,
        closed_rate: rate(statuses.closed || 0, terminalCount),
        refusal_rate: rate(refusalCount, terminalCount),
        top_subjects: topCounts(sealed, (row) => stringValue(row.skill) || "unknown", 10),
      },
      catalog: {
        entry_count: filteredCatalog.length,
        tested_entry_count: testedCatalogEntries,
        untested_entry_count: untestedCatalogEntries,
        coverage_rate: rate(testedCatalogEntries, filteredCatalog.length),
      },
      recommendations: recommendations({ decision, statuses, refusalCount, untestedCatalogEntries }),
      limitations: [
        "The run list exposes each run's state and sealed disposition, not its execution bodies.",
        "Catalog coverage proves declared fixtures or inline cases, not live provider behavior.",
      ],
    },
  };
}

function recommendations({ decision, statuses, refusalCount, untestedCatalogEntries }) {
  if (decision === "needs_more_evidence") {
    return [{ lane: "none", action: "Run governed skills before treating an empty ledger as platform health." }];
  }
  const items = [];
  if ((statuses.failed || 0) > 0 || (statuses.timed_out || 0) > 0) {
    items.push({ lane: "diagnose-skill-run", action: "Diagnose representative failed or timed-out runs and route bounded fixes through skill-build improve." });
  }
  if (refusalCount > 0) {
    items.push({ lane: "receipt-audit", action: "Sample the receipts of blocked or declined runs to confirm the governance boundary is behaving as intended." });
  }
  if (untestedCatalogEntries > 0) {
    items.push({ lane: "skill-build harness", action: `Add public-contract coverage to ${untestedCatalogEntries} catalog entr${untestedCatalogEntries === 1 ? "y" : "ies"} with no declared fixture or inline case.` });
  }
  return items;
}

function countBy(rows, keyFor) {
  return rows.reduce((counts, row) => {
    const key = keyFor(row);
    counts[key] = (counts[key] || 0) + 1;
    return counts;
  }, {});
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

function boundedLimit(value) {
  if (value === undefined || value === null || value === "") return 1_000;
  const limit = Number(value);
  if (!Number.isInteger(limit) || limit < 1 || limit > 1_000) throw new Error("limit must be an integer from 1 to 1000");
  return limit;
}

function absoluteInstant(value) {
  const text = stringValue(value);
  return text && text.includes("T") && Number.isFinite(Date.parse(text)) ? text : null;
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
