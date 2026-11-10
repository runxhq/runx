import { childRuns, readRecord, within } from "./record.mjs";

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

export default async function evals(ctx, { subject, repeats }) {
  const runs = repeats ?? DEFAULT_REPEATS;
  const packagePath = subject.resolved_path;
  const judged = { ref: packagePath, revision: subject.inspect.package_digest };
  const file = await readJson(ctx, `${packagePath}/evals/evals.json`);
  const cases = [];
  for (const entry of file.evals) {
    const { criteria } = await ctx.tool("verdict.criteria", { criteria: checks(entry) });
    const set = await ctx.read(criteria);
    cases.push({
      entry,
      criteria: set,
      criteria_set: { ref: `${packagePath}/evals/evals.json#${entry.id}`, revision: set.revision },
    });
  }
  const attempts = [];
  for (const kase of cases) {
    for (let attempt = 0; attempt < runs; attempt += 1) {
      const settled = await settle(ctx.skill(HOST, { act: act(kase.entry, packagePath) }));
      attempts.push({ kase, settled });
    }
  }
  const record = await readRecord(ctx);
  const turns = childRuns(record, HOST, attempts.length);
  for (const [index, attempt] of attempts.entries()) {
    attempt.evidence = evidence(record.events, turns[index]);
    attempt.outcome = await judge(ctx, judged, attempt);
  }
  const results = cases.map((kase) => ({
    id: kase.entry.id,
    prompt: kase.entry.prompt,
    criteria_set: kase.criteria_set,
    attempts: attempts
      .filter((attempt) => attempt.kase === kase)
      .map(({ outcome, evidence }) => ({
        ...outcome,
        calls: evidence.calls,
        ...(evidence.receipt_id === undefined ? {} : { receipt_id: evidence.receipt_id }),
      })),
  }));
  const count = (verdict) =>
    results.flatMap((result) => result.attempts).filter((attempt) => attempt.verdict === verdict).length;
  return {
    eval_results: {
      package: packagePath,
      package_digest: judged.revision,
      repeats: runs,
      evals: results,
      met: count("met"),
      not_met: count("not_met"),
      inconclusive: count("inconclusive"),
      refused: count("refused"),
    },
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

// The act an attempt is given: the eval's prompt, and the files it names, as
// paths in the workspace.
function act(entry, packagePath) {
  const files = entry.files ?? [];
  return {
    purpose: entry.prompt,
    ...(files.length === 0
      ? {}
      : { reason: `Files: ${files.map((file) => `${packagePath}/${file}`).join(", ")}` }),
  };
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
    return { verdict: "refused", reason: rejected.message };
  }
  const read = await ctx.read(verdict);
  const { decision } = await ctx.tool("verdict.decide", {
    verdict: read,
    subject: judged,
    criteria_ref: kase.criteria_set.ref,
    criteria: kase.criteria,
  });
  const decided = await ctx.read(decision);
  if (decided.status === "refused") {
    return { verdict: "refused", reason: decided.reason, evaluator: read.evaluator };
  }
  return { verdict: decided.verdict, unresolved: decided.unresolved, evaluator: read.evaluator };
}

async function readJson(ctx, path) {
  const { file_read } = await ctx.tool("fs.read", { path });
  const read = await ctx.read(file_read);
  if (read.truncated) {
    throw new Error(`${path} is longer than one read`);
  }
  return JSON.parse(read.contents);
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
