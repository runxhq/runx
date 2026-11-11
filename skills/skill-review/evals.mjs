import { byHost, childRuns, readRecord, started, turnOf, within } from "./record.mjs";
import { released } from "./released.mjs";

// A package's evals, run and judged. Each eval is worked by fresh turns of the
// agent skill's write method, one child run each, under this run's authority,
// so a run under `effects: none` writes nothing at any depth. Each attempt is
// then judged by `.#grade` against the eval's criteria set, made by a
// deterministic step from what the eval expects, and its verdict is decided
// by the one rule against the subject and set this run expects. A verdict
// that judges something else, or names another evaluator, is refused, and a
// refused verdict answers nothing.

const DEFAULT_REPEATS = 1;
const HOST = "../agent#write";

export default async function evals(ctx, { subject, repeats, eval_ids }) {
  const runs = repeats ?? DEFAULT_REPEATS;
  const judged = released(subject);
  let declared = subject.inspect.evals ?? [];
  if (eval_ids !== undefined) {
    const known = new Set(declared.map((entry) => entry.id));
    const missing = eval_ids.filter((id) => !known.has(id));
    if (missing.length) throw new Error(`unknown eval IDs: ${missing.join(", ")}`);
    const selected = new Set(eval_ids);
    declared = declared.filter((entry) => selected.has(entry.id));
  }
  if (declared.length === 0) {
    throw new Error(`${judged.ref} declares no evals`);
  }
  const cases = [];
  for (const entry of declared) {
    const { criteria } = await ctx.tool("verdict.criteria", { criteria: checks(entry) });
    const set = await ctx.read(criteria);
    cases.push({
      entry,
      criteria: set,
      criteria_set: { ref: `${judged.ref}/evals/evals.json#${entry.id}`, revision: set.revision },
    });
  }
  const attempts = [];
  for (const kase of cases) {
    for (let attempt = 0; attempt < runs; attempt += 1) {
      const settled = await settle(
        ctx.skill(HOST, { act: { purpose: kase.entry.prompt } }, {
          context: given(kase.entry, subject.inspect.eval_files ?? []),
        }),
      );
      attempts.push({ kase, settled });
    }
  }
  const record = await readRecord(ctx);
  const turns = childRuns(record, HOST, attempts.length);
  for (const [index, attempt] of attempts.entries()) {
    attempt.run = turns[index];
    attempt.evidence = evidence(record.events, turns[index]);
    attempt.decision = await judge(ctx, judged, attempt);
  }
  return {
    evidence: {
      schema: "runx.eval_evidence.v1",
      package: judged,
      ...started(record),
      repeats: runs,
      hosts: byHost(attempts, ({ run }) => turnOf(record, run)).map(({ host, models, items }) => ({
        ...(host === null ? {} : { host }),
        models,
        results: results(cases, items),
      })),
    },
  };
}

// One host's attempts at each eval, and how each verdict was decided.
function results(cases, attempts) {
  const evals = cases
    .map((kase) => ({
      id: kase.entry.id,
      criteria_set: kase.criteria_set,
      attempts: attempts
        .filter((attempt) => attempt.kase === kase)
        .map(({ decision, evidence }) => ({
          decision: decision.decision,
          ...(decision.evaluator === undefined ? {} : { evaluator: decision.evaluator }),
          ...(evidence.receipt_id === undefined ? {} : { receipt_id: evidence.receipt_id }),
        })),
    }))
    .filter((result) => result.attempts.length > 0);
  const decisions = evals.flatMap((result) => result.attempts.map((attempt) => attempt.decision));
  const count = (verdict) => decisions.filter((decision) => decision.verdict === verdict).length;
  return {
    kind: "evals",
    evals,
    met: count("met"),
    not_met: count("not_met"),
    inconclusive: count("inconclusive"),
    refused: decisions.filter((decision) => decision.status === "refused").length,
  };
}

// The checks an eval is judged against: its expected output, then each of
// its expectations, all required.
function checks(entry) {
  return [
    { id: "expected_output", required: true, question: entry.expected_output },
    ...(entry.expectations ?? []).map((expectation, index) => ({
      id: `expectation_${index + 1}`,
      required: true,
      question: expectation,
    })),
  ];
}

// The context an attempt is given beside its act, the eval's prompt: each
// file the eval names, such as the prior run it builds on, as an entry its
// turn reads by locator, summarized by its path in the package.
function given(entry, evalFiles) {
  return (entry.files ?? []).map((path) => {
    const file = evalFiles.find((candidate) => candidate.path === path);
    if (!file) {
      throw new Error(`eval ${entry.id} names ${path}, which the package's inspection doesn't carry`);
    }
    return { summary: path, content: file.content };
  });
}

// An attempt's settlement, kept whether it resolved or was rejected: either
// is what the grader judges.
async function settle(call) {
  try {
    return { status: "fulfilled", value: await call };
  } catch (rejected) {
    return { status: "rejected", reason: rejected.reason, message: rejected.message };
  }
}

// The attempt judged: its verdict from `.#grade`, decided against what this
// run expects. A grade the runtime refused, for a verdict naming another
// evaluator, answers nothing either.
async function judge(ctx, judged, { kase, settled, evidence }) {
  let verdict;
  try {
    ({ verdict } = await ctx.skill(".#grade", {
      subject: judged,
      criteria_set: kase.criteria_set,
      criteria: kase.criteria,
      eval: { prompt: kase.entry.prompt, expected_output: kase.entry.expected_output },
      attempt: {
        status: settled.status,
        ...(settled.status === "fulfilled" ? { result: settled.value } : { failure: settled.message }),
        calls: evidence.calls,
        ...(evidence.receipt_id === undefined ? {} : { receipt_id: evidence.receipt_id }),
      },
    }));
  } catch (rejected) {
    return { decision: { status: "refused", reason: rejected.message } };
  }
  const read = await ctx.read(verdict);
  const { decision } = await ctx.tool("verdict.decide", {
    verdict: read,
    subject: judged,
    criteria_ref: kase.criteria_set.ref,
    criteria: kase.criteria,
  });
  return { decision: await ctx.read(decision), evaluator: read.evaluator };
}

// What an attempt's run did, as its record holds it: each call it made and
// how it ended, and the receipt it sealed.
function evidence(events, stream) {
  const calls = events
    .filter((record) => within(stream, record.stream) && record.event.kind === "call")
    .map(({ event }) => ({
      target: target(event.target),
      outcome: event.outcome.status,
      ...(event.outcome.reason === undefined ? {} : { reason: event.outcome.reason }),
    }));
  const sealed = events.find((record) => record.stream === stream && record.event.kind === "sealed");
  return { calls, receipt_id: sealed?.event.receipt_id };
}

function target(called) {
  switch (called.kind) {
    case "tool":
      return called.tool;
    case "operation":
      return `${called.requirement}/${called.operation}`;
    case "skill":
      return called.method === undefined ? called.skill : `${called.skill}#${called.method}`;
    default:
      return called.kind;
  }
}
