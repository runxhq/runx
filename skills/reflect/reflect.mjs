// runx/reflect's code (reflection.md): a worker with no I/O. Everything
// survey reads is a call the kernel admits and records, so the same record
// gives the same survey, by digest.

/** This skill's name, whose runs no survey reads. */
const SELF = "reflect";
/** Reflection's own stream: one mark per reader, which only keep moves. */
export const MARKS = "marks";
/** The most runs one run.list call returns. */
const LIST_LIMIT = 1000;
/** How many skills' latest success the purpose-only reader is given. */
const SAMPLED_SKILLS = 8;
/** The instruction file a promoted rule goes into, read as HEAD holds it. */
export const INSTRUCTIONS = "AGENTS.md";
/** The purposes that make the person's words a standing rule (memory.md). */
const STANDING = new Set(["mandate", "procedure"]);

/** Survey's own check, answered by whether every run was read to its end. */
export const RECORD_READ = {
  id: "record_read",
  required: true,
  question: "Was every run in the window read to its last event, and every run still open kept for the next?",
};

/**
 * A step's or a call's output field as the record holds it: a packet's
 * envelope, its schema beside its data, or a plain value.
 */
function packetOf(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return undefined;
  if (typeof value.schema === "string" && "data" in value) return { schema: value.schema, data: value.data };
  if (typeof value.schema === "string") return { schema: value.schema, data: value };
  return undefined;
}

/** Where an event is in the record, as evidence a verdict can cite. */
function addressOf(record) {
  return `runx:run:${record.stream}@${record.version}`;
}

/**
 * What resisted in one run's events: every refused, declined and failed call,
 * failed step, declined ask with the person's reason, and unmet verdict, each
 * with its evidence, the sealed receipt when there is one and otherwise the
 * event's address. A run that sealed other than completed with nothing else to
 * show is one item itself.
 */
export function resisted(summary, records) {
  const items = [];
  const at = (record, fields) => ({ run: summary.run_id, skill: summary.skill, stream: record.stream, ...fields });
  for (const record of records) {
    const event = record.event;
    if (event.kind === "call") {
      const outcome = event.outcome;
      if (["refused", "declined", "failed"].includes(outcome.status)) {
        items.push(at(record, { kind: `${outcome.status}_call`, step: event.step_id, index: event.index, target: event.target, reason: outcome.reason, evidence: addressOf(record) }));
      } else if (outcome.status === "completed" && outcome.succeeded === false) {
        items.push(at(record, { kind: "failed_call", step: event.step_id, index: event.index, target: event.target, reason: outcome.reason ?? null, evidence: outcome.receipt_id }));
      }
    } else if (event.kind === "step") {
      if (event.succeeded === false) {
        items.push(at(record, { kind: "failed_step", step: event.step_id, reason: event.reason ?? null, evidence: event.receipt_id }));
      }
      for (const value of Object.values(event.output ?? {})) {
        const packet = packetOf(value);
        if (packet?.schema === "runx.approval.decision.v1" && packet.data?.approved === false) {
          items.push(at(record, { kind: "declined_ask", step: event.step_id, gate: packet.data.gate_id ?? null, reason: packet.data.reason ?? null, evidence: event.receipt_id }));
        }
        if (packet?.schema === "runx.verdict.v1" && packet.data?.verdict !== "met") {
          items.push(at(record, { kind: "unmet_verdict", step: event.step_id, verdict: packet.data?.verdict ?? null, subject: packet.data?.subject ?? null, evidence: event.receipt_id }));
        }
      }
    }
  }
  const sealed = records.find((record) => record.event.kind === "sealed" && record.stream === summary.run_id);
  if (items.length === 0 && sealed && sealed.event.outcome !== "completed") {
    items.push(at(sealed, { kind: `sealed_${sealed.event.outcome}`, reason: null, evidence: sealed.event.receipt_id }));
  }
  return items;
}

/** The latest success of each skill, by skill name, at most SAMPLED_SKILLS: a fixed rule over the record. */
export function sampledOf(successes) {
  const latest = new Map();
  for (const run of successes) {
    const held = latest.get(run.skill);
    if (!held || later(run, held)) latest.set(run.skill, run);
  }
  return [...latest.values()]
    .sort((left, right) => (left.skill < right.skill ? -1 : left.skill > right.skill ? 1 : 0))
    .slice(0, SAMPLED_SKILLS)
    .map(({ run_id, skill, method, receipt_id }) => ({ run: run_id, skill, method, evidence: receipt_id }));
}

function later(left, right) {
  return left.created_at > right.created_at || (left.created_at === right.created_at && left.run_id > right.run_id);
}

/** Each standing said:workspace rule whose handle the instruction file doesn't hold. */
export function unheldOf(indexEntries, instructions) {
  const rules = [];
  for (const entry of indexEntries ?? []) {
    for (const item of entry?.data?.entries ?? []) {
      if (item?.kind === "said" && STANDING.has(item.purpose) && String(item.handle).startsWith("memory:said:workspace")) rules.push(item);
    }
  }
  return rules
    .filter((rule) => !instructions.includes(rule.handle))
    .sort((left, right) => (left.handle < right.handle ? -1 : left.handle > right.handle ? 1 : 0))
    .map(({ handle, text, summary, date, purpose }) => ({ handle, text, summary, date, purpose }));
}

/** The checks, one per item, after survey's own: laid out before anything is judged. */
export function checksOf({ resistance, unheld, sampled }) {
  const where = (item) => `${item.stream}${item.step ? `#${item.step}` : ""}${item.index === undefined ? "" : `/${item.index}`}`;
  return [
    RECORD_READ,
    ...resistance.map((item, index) => ({
      id: `resisted_${index + 1}`,
      required: true,
      question: `Is why the ${item.kind.replaceAll("_", " ")} at ${where(item)} happened found from its evidence, and kept as a fix with a case that fails on the old revision and passes on the fix, or as an observation with its sources?`.slice(0, 1024),
    })),
    ...unheld.map((rule, index) => ({
      id: `unheld_${index + 1}`,
      required: true,
      question: `Does ${INSTRUCTIONS} hold ${rule.handle} word for word, with its date and handle?`.slice(0, 1024),
    })),
    ...sampled.map((run, index) => ({
      id: `sampled_${index + 1}`,
      required: true,
      question: `Did run ${run.run} of ${run.skill} do what ${run.skill}'s purpose asks, judged from its purpose and the run alone?`.slice(0, 1024),
    })),
  ];
}

/**
 * Read the mark, the record since it, the workspace's standing memory and its
 * revision, and lay out the checks. The window is every run that started from
 * the mark's `through` to `until`, oldest first, less those already read at
 * `through`, and every run open at the mark. A run not yet sealed is kept open
 * for the next survey. A survey that runs out of calls stops before the run it
 * can't finish, says so, and its mark moves only past what it read.
 */
export async function survey(ctx, { reader, until }) {
  let calls = 0;
  const room = (needed) => calls + needed <= ctx.max_calls;
  const tool = async (name, inputs) => {
    calls += 1;
    return ctx.tool(name, inputs);
  };
  const read = async (value) => {
    calls += 1;
    return ctx.read(value);
  };

  // The repository first: the checks are bound to its HEAD, and a rule is
  // held when AGENTS.md holds its handle there. Once HEAD reads, a failed
  // read of AGENTS.md is a HEAD without one, which holds no rule.
  let head;
  try {
    head = (await read((await tool("git.log", { revision: "HEAD", max_count: 1 })).git_log)).commits?.[0]?.commit;
  } catch (rejection) {
    throw new Error(`reflection reads a workspace that is a git repository, and this one's HEAD can't be read: ${rejection?.message ?? rejection}`);
  }
  if (!head) throw new Error("reflection reads a workspace that is a git repository, and this one has no commit at HEAD");
  let instructions = "";
  try {
    const shown = await read((await tool("git.show", { revision: "HEAD", path: INSTRUCTIONS })).git_show);
    if (shown.truncated) throw new Error(`${INSTRUCTIONS} at HEAD is past what git.show reads whole, so which rules it holds can't be known`);
    instructions = shown.text;
  } catch (rejection) {
    if (rejection?.code !== "failed") throw rejection;
  }

  calls += 1;
  const inspected = (await ctx.operation("store/key.inspect", { stream: MARKS, key: reader }))?.provider_operation?.result;
  const mark = { version: inspected?.version ?? 0, ...(inspected?.last_event?.data ?? {}) };
  const readAt = new Set(mark.at_through ?? []);
  const previouslyOpen = mark.open ?? [];

  // Every listed run, newest first, paged older until a page comes back short.
  const listed = new Map();
  let bound = until;
  for (;;) {
    if (!room(2)) break;
    const page = (await read((await tool("run.list", { ...(mark.through ? { since: mark.through } : {}), until: bound, limit: LIST_LIMIT })).run_list)).runs ?? [];
    for (const run of page) listed.set(run.run_id, run);
    if (page.length < LIST_LIMIT) break;
    const oldest = page.at(-1).created_at;
    if (oldest === bound) break;
    bound = oldest;
  }
  // Reflection never judges its own runs: its walls keep its package its own.
  const window = [...listed.values()]
    .filter((run) => run.skill !== SELF && !readAt.has(run.run_id) && !previouslyOpen.includes(run.run_id))
    .sort((left, right) => (later(left, right) ? 1 : later(right, left) ? -1 : 0));

  // One run's record, every page of it.
  const recordOf = async (run_id) => {
    const records = [];
    let after;
    let summary;
    for (;;) {
      const page = await read((await tool("run.read", { run_id, ...(after ? { after } : {}) })).run_read);
      summary = page.run;
      records.push(...page.events);
      if (!page.more) return { summary, records };
      if (!room(2)) return undefined;
      after = page.cursor;
    }
  };

  const resistance = [];
  const successes = [];
  const readRuns = [];
  const open = [];
  let read_to_end = true;
  const take = ({ summary, records }) => {
    readRuns.push(summary.run_id);
    const items = resisted(summary, records);
    resistance.push(...items);
    if (items.length === 0 && summary.outcome === "completed") successes.push(summary);
  };
  // The runs open at the mark first: each read again, and kept open until it seals.
  for (const run_id of previouslyOpen) {
    if (!room(2)) {
      read_to_end = false;
      open.push(run_id);
      continue;
    }
    const recorded = await recordOf(run_id);
    if (recorded === undefined) {
      read_to_end = false;
      open.push(run_id);
    } else if (recorded.summary.state !== "sealed") open.push(run_id);
    else take(recorded);
  }
  // Then the window, oldest first, stopping before a run it can't finish.
  let through = mark.through ?? null;
  let atThrough = [...readAt];
  for (const run of window) {
    if (run.state !== "sealed") {
      open.push(run.run_id);
    } else {
      // Room for its read and what follows it: memory, the subject's digest
      // and the checks.
      if (!room(2 + 6)) {
        read_to_end = false;
        break;
      }
      const recorded = await recordOf(run.run_id);
      if (recorded === undefined) {
        read_to_end = false;
        break;
      }
      take(recorded);
    }
    if (run.created_at !== through) {
      through = run.created_at;
      atThrough = [];
    }
    atThrough.push(run.run_id);
  }

  calls += 3;
  const memory = await ctx.skill("runx/memory#index", { levels: [{ level: "workspace" }] });
  const entries = await ctx.read(memory.entries);

  const sampled = sampledOf(successes);
  const unheld = unheldOf(entries, instructions);
  const revision = (await tool("data.digest", { value: { head, runs: [...readRuns].sort(), open: [...open].sort() } })).digest_result.digest;
  const subject = { ref: `runx/reflect/window/${reader}`, revision, head, runs: readRuns.length };
  const { criteria } = await tool("verdict.criteria", { criteria: checksOf({ resistance, unheld, sampled }) });
  return {
    criteria,
    subject,
    resistance,
    sampled,
    unheld,
    mark: { version: mark.version, through, at_through: atThrough.sort(), open: [...new Set(open)].sort() },
    read_to_end,
  };
}
