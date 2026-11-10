import { byHost, childRuns, readRecord, started, turnOf, within } from "./record.mjs";
import { released } from "./released.mjs";

// How often a host selects one released package for each of its eval
// queries. Each query is worked by fresh turns of the agent skill's read
// method, one child run each, and a turn selected the package when the run's
// record shows a load in that turn's run that bound the package's
// distribution digest, whether the load completed or waits on the person's
// trust. What a turn says it did is never the evidence.

// A query passes when the package is selected in at least half its runs for
// a request that should select it, and in fewer than half for one that
// shouldn't.
const DEFAULT_REPEATS = 3;
// The most queries one measurement takes, so its calls stay inside the
// step's max_calls: the queries' read, a turn per query and repeat, and the
// pages of the run's record.
const MAX_QUERIES = 40;
const HOST = "../agent#read";

export default async function triggers(ctx, { subject, repeats }) {
  const runs = repeats ?? DEFAULT_REPEATS;
  const pinned = released(subject);
  const queries = evalQueries(subject);
  const turns = [];
  for (const [query, entry] of queries.entries()) {
    for (let turn = 0; turn < runs; turn += 1) {
      await ctx.skill(HOST, { act: { purpose: entry.query } });
      turns.push(query);
    }
  }
  const record = await readRecord(ctx);
  const children = childRuns(record, HOST, turns.length);
  const selected = selections(record, pinned.revision, children);
  const measured = turns.map((query, turn) => ({ query, run: children[turn], selected: selected[turn] }));
  return {
    evidence: {
      schema: "runx.eval_evidence.v1",
      package: pinned,
      ...started(record),
      repeats: runs,
      hosts: byHost(measured, ({ run }) => turnOf(record, run)).map(({ host, models, items }) => ({
        ...(host === null ? {} : { host }),
        models,
        results: selection(queries, items),
      })),
    },
  };
}

// One host's selection: each query's runs on it, and whether it passed.
function selection(queries, measured) {
  const rows = queries.map(({ query, should_trigger }, index) => {
    const runs = measured.filter((turn) => turn.query === index);
    const selected = runs.filter((turn) => turn.selected).length;
    const passed = should_trigger ? selected * 2 >= runs.length : selected * 2 < runs.length;
    return { query, should_trigger, selected, runs: runs.length, passed };
  }).filter((row) => row.runs > 0);
  const tally = (wanted) => {
    const kind = rows.filter((row) => row.should_trigger === wanted);
    return { queries: kind.length, passed: kind.filter((row) => row.passed).length };
  };
  return {
    kind: "selection",
    queries: rows,
    passed: rows.filter((row) => row.passed).length,
    should_trigger: tally(true),
    should_not_trigger: tally(false),
  };
}

// The package's eval queries, as its inspection carries them.
function evalQueries(subject) {
  const queries = subject.inspect.eval_queries ?? [];
  if (queries.length === 0) {
    throw new Error(`${subject.resolved_path} declares no eval queries`);
  }
  if (queries.length > MAX_QUERIES) {
    throw new Error(
      `${subject.resolved_path} declares ${queries.length} eval queries, past the ${MAX_QUERIES} one measurement takes`,
    );
  }
  return queries;
}

// For each child run, whether a load in it, or a run below it, bound
// `digest`.
function selections(record, digest, children) {
  const loads = record.events
    .filter(
      ({ event }) =>
        event.kind === "call" &&
        event.target?.kind === "load" &&
        event.loaded?.kind === "skill" &&
        event.loaded.package_digest === digest,
    )
    .map(({ stream }) => stream);
  return children.map((run) => loads.some((load) => within(run, load)));
}
