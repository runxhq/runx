// memory's horizon evaluation at one size and phase: each of the phase's
// cases in evals/horizon.json asked of the routine, `repeats` times, each
// attempt judged by `grade`, a deterministic step whose verdict the runtime
// names the evaluator of, and decided by the one rule (`verdict.decide`).
// The history it asks over is laid down by its test, in a fresh home for
// each size, with the run's clock pinned. Only the record says what an
// attempt did: its turn's model, host and usage, how it ended, and what it
// kept.

const ROUTINE = "../routine";
const ROUTINES = [ROUTINE, `${ROUTINE}#draft`];
const SEARCH = "../..#search";
/// The seal reason of a turn whose model only wrote prose until its
/// resamples ran out.
const PROSE_ONLY = "managed_agent_empty_turn_budget_exhausted";

export async function sweep(ctx, { subject, memory, phase, case_id, repeats, focus }) {
  const judged = { ref: memory, revision: subject.inspect.package_digest };
  const horizon = await readJson(ctx, `${memory}/evals/horizon.json`);
  const cases = [];
  for (const kase of horizon.cases.filter((candidate) => candidate.phase === phase && (case_id === undefined || candidate.id === case_id))) {
    const { criteria } = await ctx.tool("verdict.criteria", { criteria: checks(kase) });
    const set = await ctx.read(criteria);
    cases.push({ kase, criteria: set, criteria_set: { ref: `${memory}/evals/horizon.json#${kase.id}`, revision: set.revision } });
  }
  if (cases.length === 0) throw new Error(`phase ${phase} has no case ${case_id ?? "to run"}`);
  const now = ctx.now();
  const attempts = [];
  for (const item of cases) {
    const question = dated(item.kase.question, now);
    for (let attempt = 0; attempt < repeats; attempt += 1) {
      const inputs = { question, ...(focus ? { focus: question } : {}) };
      const method = item.kase.method === undefined ? ROUTINE : `${ROUTINE}#${item.kase.method}`;
      const settled = await settle(ctx.skill(method, inputs));
      attempts.push({ item, settled, kept: await kept(ctx, item.kase) });
    }
  }
  const record = await readRecord(ctx);
  const runs = childRuns(record, ROUTINES, attempts.length);
  for (const [index, attempt] of attempts.entries()) {
    attempt.turn = turnOf(record, runs[index]);
    attempt.receipt_id = sealedIn(record, runs[index]);
    attempt.ending = await ending(ctx, attempt);
    attempt.decision = await judge(ctx, judged, attempt);
  }
  return {
    report: {
      phase,
      focus,
      repeats,
      ...started(record),
      attempts: attempts.map(({ item, turn, ending: end, decision, receipt_id }) => ({
        case: item.kase.id,
        category: item.kase.category,
        ...(item.kase.planted === undefined ? {} : { planted: item.kase.planted }),
        verdict: decision.decision?.verdict ?? null,
        ...(decision.decision?.status === "refused" ? { refused: decision.decision.reason } : {}),
        ending: end,
        model: turn.model,
        host: turn.host,
        usage: turn.usage,
        receipt_id,
      })),
    },
  };
}

/// A case's question with each `{days_ago:N}` as the date N days before the
/// run's clock, as its test laid the history's dates down.
function dated(question, now) {
  return question.replace(/\{days_ago:(\d+)\}/g, (_, days) =>
    new Date(now - Number(days) * 86_400_000).toISOString().slice(0, 10),
  );
}

/// The checks a case is judged against, each a question that must be
/// answered yes, all required.
function checks(kase) {
  const expect = kase.expect;
  const checks = [];
  if (expect.found !== undefined) {
    checks.push({ id: "found", required: true, question: `Does the answer say memory ${expect.found ? "held" : "didn't hold"} the answer?` });
  }
  for (const [index, word] of (expect.says ?? []).entries()) {
    checks.push({ id: `says_${index + 1}`, required: true, question: `Does the answer say "${word}"?` });
  }
  for (const [index, word] of (expect.never ?? []).entries()) {
    checks.push({ id: `never_${index + 1}`, required: true, question: `Does the answer leave out "${word}"?` });
  }
  if (expect.keeps) {
    checks.push({ id: "keeps", required: true, question: `Did the run keep an observation saying ${expect.keeps.says.join(" and ")}?` });
  }
  return checks;
}

/// What memory holds after an attempt that a capture case expects kept: the
/// current observations a search for its words finds. What was kept, never
/// what the turn says it kept.
async function kept(ctx, kase) {
  if (!kase.expect.keeps) return [];
  const { entries } = await ctx.skill(SEARCH, { levels: [{ level: "personal" }], query: kase.expect.keeps.says.join(" ") });
  return entries.filter((entry) => entry.labels.purpose === "observation").map((entry) => entry.data.text);
}

/// How an attempt ended, from its receipt: answered, prose_only when its
/// model only wrote prose until its resamples ran out, or the reason it
/// sealed with otherwise.
async function ending(ctx, { settled, receipt_id }) {
  if (settled.status === "fulfilled") return "answered";
  if (receipt_id === undefined) return settled.message;
  const { receipt_read } = await ctx.tool("receipt.read", { receipt_id, json_pointer: "/seal/reason", max_bytes: 256 });
  const reason = receipt_read.value;
  return reason === PROSE_ONLY ? "prose_only" : (reason ?? settled.message);
}

/// One attempt judged by the grade, decided against what this sweep expects
/// judged. A grade the runtime refused answers nothing.
async function judge(ctx, judged, { item, settled, kept: keptTexts }) {
  let verdict;
  try {
    ({ verdict } = await ctx.skill(".#grade", {
      subject: judged,
      criteria_set: item.criteria_set,
      criteria: item.criteria,
      case: item.kase,
      attempt: {
        status: settled.status,
        ...(settled.status === "fulfilled" ? { result: { answer: settled.value.answer, found: settled.value.found } } : {}),
        kept: keptTexts,
      },
    }));
  } catch (rejected) {
    return { decision: { status: "refused", reason: rejected.message } };
  }
  const { decision } = await ctx.tool("verdict.decide", {
    verdict: await ctx.read(verdict),
    subject: judged,
    criteria_ref: item.criteria_set.ref,
    criteria: item.criteria,
  });
  return { decision: await ctx.read(decision) };
}

/// The grade: each check answered from the attempt alone, with no model.
/// An attempt that gave no answer leaves every check inconclusive.
export function grade({ subject, criteria_set, criteria, case: kase, attempt }) {
  const answer = String(attempt.result?.answer ?? "");
  const lowered = answer.toLowerCase();
  const expect = kase.expect;
  const assessments = criteria.criteria.map(({ id }) => {
    if (attempt.status !== "fulfilled") return { verdict: "inconclusive", id, evidence: [] };
    const [kind, number] = id.split("_");
    const met = (holds, expected, actual, where = "the routine's answer") =>
      holds
        ? { verdict: "met", id, evidence: [] }
        : { verdict: "not_met", id, evidence: [], failure: { expected, actual: actual || "nothing", where } };
    switch (kind) {
      case "found":
        return met(attempt.result?.found === expect.found, `found ${expect.found}`, `found ${attempt.result?.found}`);
      case "says": {
        const word = expect.says[Number(number) - 1];
        return met(lowered.includes(word.toLowerCase()), word, answer);
      }
      case "never": {
        const word = expect.never[Number(number) - 1];
        return met(!lowered.includes(word.toLowerCase()), `no "${word}"`, answer);
      }
      case "keeps": {
        const held = attempt.kept.some((text) =>
          expect.keeps.says.every((word) => text.toLowerCase().includes(word.toLowerCase())),
        );
        return met(held, expect.keeps.says.join(" and "), attempt.kept.join(" | "), "memory's observations");
      }
      default:
        throw new Error(`no check is named ${id}`);
    }
  });
  const answers = assessments.map((assessment) => assessment.verdict);
  const verdict = answers.includes("not_met") ? "not_met" : answers.includes("inconclusive") ? "inconclusive" : "met";
  return { verdict: { schema: "runx.verdict.v1", verdict, subject, criteria_set, assessments } };
}

async function settle(call) {
  try {
    return { status: "fulfilled", value: await call };
  } catch (rejected) {
    return { status: "rejected", message: rejected.message };
  }
}

async function readJson(ctx, path) {
  const { file_read } = await ctx.tool("fs.read", { path });
  const read = await ctx.read(file_read);
  if (read.truncated) throw new Error(`${path} is longer than one read`);
  return JSON.parse(read.contents);
}

// This run's record, every page of it.
async function readRecord(ctx) {
  const pages = [];
  let after;
  for (;;) {
    const { run_read } = await ctx.tool("run.read", after === undefined ? {} : { after });
    const page = await ctx.read(run_read);
    pages.push(page);
    if (!page.more) return { run: page.run, events: pages.flatMap((read) => read.events) };
    after = page.cursor;
  }
}

// The child run of each call this run's script made of `host`, in the order
// it made them. A step's calls are on a stream of their own,
// `<run>/<step>@<attempt>`, beside the run's.
function childRuns({ run, events }, hosts, made) {
  const root = run.run_id;
  const calls = new Map();
  for (const { stream, event } of events) {
    if (
      event.kind === "call" &&
      stream === `${root}/${event.step_id}@${event.attempt}` &&
      event.target?.kind === "skill" &&
      hosts.includes(event.target.skill)
    ) {
      calls.set(event.index, `${event.step_id}/${event.index}`);
    }
  }
  const children = new Map(
    events
      .filter(({ event }) => event.kind === "child_started" && event.parent === root)
      .map(({ stream, event }) => [event.step_id, stream]),
  );
  const runs = [...calls.entries()]
    .sort(([left], [right]) => left - right)
    .map(([, position]) => {
      const stream = children.get(position);
      if (stream === undefined) throw new Error(`this run's record holds no child run for its call ${position}`);
      return stream;
    });
  if (runs.length !== made) {
    throw new Error(`this run's record holds ${runs.length} calls of ${hosts.join(" or ")}, and this script made ${made}`);
  }
  return runs;
}

// Whether `stream` is the child run `run`'s, or one of the streams below it.
function within(run, stream) {
  return stream === run || stream.startsWith(`${run}/`);
}

// Where the run ran and what started it, as its start recorded them.
function started({ events }) {
  const start = events.find(({ event }) => event.kind === "started");
  if (start === undefined) throw new Error("this run's record holds no start");
  return { ran_on: start.event.ran_on, implementation: start.event.implementation };
}

// The model that answered the turn in the run `run`, the host that gave its
// answer and what its calls to the model used, as its answer states them:
// from the agent step's event in that run.
function turnOf({ events }, run) {
  const step = events.find(
    ({ stream, event }) => within(run, stream) && event.kind === "step" && (event.host || event.model),
  );
  return { host: step?.event.host ?? null, model: step?.event.model ?? null, usage: step?.event.usage ?? null };
}

// The receipt the run `run` sealed with.
function sealedIn({ events }, run) {
  return events.find(({ stream, event }) => stream === run && event.kind === "sealed")?.event.receipt_id;
}
