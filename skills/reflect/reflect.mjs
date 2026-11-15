// runx/reflect's code (reflection.md): a worker with no I/O. Everything
// survey reads is a call the kernel admits and records, so the same record
// gives the same survey, by digest.

/** This skill's name, whose runs no survey reads. */
const SELF = "reflect";
/** Reflection's own stream: one mark per reader, which only keep moves. */
export const MARKS = "marks";
/** Reflection's own stream of declined changes, one per change and its evidence. */
export const DECLINES = "declines";
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
  // Each item's date is its run's: the fact's own date, never when it's read.
  const at = (record, fields) => ({ run: summary.run_id, skill: summary.skill, stream: record.stream, date: summary.updated_at.slice(0, 10), ...fields });
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

/** Each standing said:workspace rule, split by whether the instruction file holds its handle. */
export function rulesOf(indexEntries, instructions) {
  const rules = [];
  for (const entry of indexEntries ?? []) {
    for (const item of entry?.data?.entries ?? []) {
      if (item?.kind === "said" && STANDING.has(item.purpose) && String(item.handle).startsWith("memory:said:workspace")) rules.push(item);
    }
  }
  const sorted = rules
    .sort((left, right) => (left.handle < right.handle ? -1 : left.handle > right.handle ? 1 : 0))
    .map(({ handle, text, summary, date, purpose }) => ({ handle, text, summary, date, purpose }));
  return {
    unheld: sorted.filter((rule) => !instructions.includes(rule.handle)),
    held: sorted.filter((rule) => instructions.includes(rule.handle)),
  };
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
  const { unheld, held } = rulesOf(entries, instructions);
  const revision = (await tool("data.digest", { value: { head, runs: [...readRuns].sort(), open: [...open].sort() } })).digest_result.digest;
  const subject = { ref: `runx/reflect/window/${reader}`, revision };
  const { criteria } = await tool("verdict.criteria", { criteria: checksOf({ resistance, unheld, sampled }) });
  return {
    criteria,
    subject,
    head,
    resistance,
    sampled,
    unheld,
    held,
    mark: { version: mark.version, through, at_through: atThrough.sort(), open: [...new Set(open)].sort() },
    read_to_end,
  };
}

/** The refs a run's record holds that a finding may cite: each sealed receipt, and each event's address. */
export function refsOf(records) {
  const refs = new Set();
  for (const record of records) {
    refs.add(`runx:run:${record.stream}@${record.version}`);
    const event = record.event;
    if (typeof event.receipt_id === "string") refs.add(event.receipt_id);
    if (typeof event.outcome?.receipt_id === "string") refs.add(event.outcome.receipt_id);
  }
  return refs;
}

/**
 * A finding a turn returned, kept only when it cites evidence, and every ref
 * it cites is a sealed receipt or an event of the run it judged. Otherwise
 * it's dropped, and says why: a diagnosis that rests on nothing the record
 * holds is no diagnosis.
 */
export function kept(criterion, finding, refs) {
  const evidence = Array.isArray(finding?.evidence) ? finding.evidence : [];
  const unknown = evidence.filter((ref) => !refs.has(ref));
  if (evidence.length === 0) return { criterion, dropped: "it cites no evidence" };
  if (unknown.length > 0) return { criterion, dropped: `it cites what the record doesn't hold: ${unknown.join(", ")}` };
  return { criterion, finding };
}

/** A skill's purpose: its manual's front-matter description, and nothing of its design. */
export function purposeOf(manual) {
  const front = /^---\n([\s\S]*?)\n---/.exec(manual ?? "");
  const line = front?.[1].split("\n").find((entry) => entry.startsWith("description:"));
  return line?.slice("description:".length).trim().replace(/^(["'])(.*)\1$/, "$2") || undefined;
}

/**
 * Judge the window: one diagnosis per resistance and one purpose-only
 * question per sampled success, each its own child run in a fresh context.
 * Code reads each run's record first, so a finding is kept only when what it
 * cites is in it. A sampled skill whose purpose can't be read is left
 * unjudged, which its check reads as unresolved.
 */
export async function judge(ctx, { survey }) {
  const findings = [];
  const recordOf = async (run_id) => {
    const records = [];
    let after;
    for (;;) {
      const page = await ctx.read((await ctx.tool("run.read", { run_id, ...(after ? { after } : {}) })).run_read);
      records.push(...page.events);
      if (!page.more) return records;
      after = page.cursor;
    }
  };
  for (const [index, resistance] of survey.resistance.entries()) {
    const refs = refsOf(await recordOf(resistance.run));
    const { finding } = await ctx.skill(".#diagnose", { resistance });
    findings.push(kept(`resisted_${index + 1}`, finding, refs));
  }
  for (const [index, sampled] of survey.sampled.entries()) {
    const criterion = `sampled_${index + 1}`;
    let purpose;
    try {
      const manual = await ctx.read((await ctx.tool("git.show", { revision: "HEAD", path: `skills/${sampled.skill}/SKILL.md` })).git_show);
      purpose = purposeOf(manual.text);
    } catch (rejection) {
      if (rejection?.code !== "failed") throw rejection;
    }
    if (typeof purpose !== "string" || purpose.length === 0) {
      findings.push({ criterion, dropped: `the purpose of ${sampled.skill} can't be read` });
      continue;
    }
    const records = await recordOf(sampled.run);
    // The run as its record holds it, and the skill's purpose: never its design.
    const run = { run: sampled.run, skill: sampled.skill, method: sampled.method, events: records.map((record) => ({ address: `runx:run:${record.stream}@${record.version}`, ...record.event })) };
    const { finding } = await ctx.skill(".#challenge", { purpose, run });
    findings.push(kept(criterion, finding, refsOf(records)));
  }
  return { findings };
}

/**
 * One assessment per check, from what survey found and the findings judge
 * kept, as a runx.verdict.v1 whose subject is survey's window. A resistance
 * is met once its fix and case are proven and applied, or, with keep, kept as
 * an observation, and a rule once AGENTS.md holds it; until then each is
 * not_met, saying what it lacks, a declined fix with the person's reason. A sampled run is met or not as its purpose-only
 * reader found, and unresolved when no finding was kept.
 */
export function assessments({ survey, findings, proposals = [], promoted = [], observed = [] }) {
  const observations = new Map(observed.filter((entry) => entry.handle).map((entry) => [entry.criterion, entry.handle]));
  const written = new Set(promoted.map((rule) => rule.handle));
  const by = new Map(findings.map((entry) => [entry.criterion, entry]));
  const proposed = new Map(proposals.map((entry) => [entry.criterion, entry]));
  const where = (item) => `${item.stream}${item.step ? `#${item.step}` : ""}${item.index === undefined ? "" : `/${item.index}`}`;
  const answers = [];
  for (const criterion of survey.criteria.criteria) {
    const { id } = criterion;
    if (id === "record_read") {
      answers.push(
        survey.read_to_end
          ? { id, verdict: "met", evidence: [] }
          : { id, verdict: "not_met", evidence: [], failure: { expected: "every run in the window read to its last event", actual: "the calls ran out first", where: survey.subject.ref } },
      );
      continue;
    }
    const [kind, ordinal] = [id.slice(0, id.lastIndexOf("_")), Number(id.slice(id.lastIndexOf("_") + 1)) - 1];
    const entry = by.get(id);
    if (kind === "resisted") {
      const item = survey.resistance[ordinal];
      const proposal = proposed.get(id);
      if (proposal?.applied || observations.has(id)) {
        answers.push({ id, verdict: "met", evidence: [item.evidence] });
        continue;
      }
      answers.push({
        id,
        verdict: "not_met",
        evidence: [item.evidence],
        failure: {
          expected: "a fix with a case that fails on the old revision and passes on the fix, or an observation with its sources",
          actual: proposal?.declined !== undefined
            ? `its fix was declined: ${proposal.declined ?? "no reason given"}`
            : proposal?.dropped
              ? `its fix was dropped: ${proposal.dropped}`
              : entry?.finding
                ? `diagnosed, and nothing kept yet: ${entry.finding.summary ?? ""}`.trim()
                : `no diagnosis kept: ${entry?.dropped ?? "none was made"}`,
          where: where(item),
        },
      });
    } else if (kind === "unheld") {
      const rule = survey.unheld[ordinal];
      answers.push(
        written.has(rule.handle)
          ? { id, verdict: "met", evidence: [] }
          : { id, verdict: "not_met", evidence: [], failure: { expected: `AGENTS.md holds ${rule.handle}`, actual: "it isn't promoted", where: "AGENTS.md" } },
      );
    } else if (kind === "sampled") {
      const run = survey.sampled[ordinal];
      if (!entry?.finding) continue;
      const { finding } = entry;
      answers.push(
        finding.met === true
          ? { id, verdict: "met", evidence: finding.evidence }
          : { id, verdict: "not_met", evidence: finding.evidence, failure: { expected: `what ${run.skill}'s purpose asks`, actual: finding.summary ?? "it did something else", where: `runx:run:${run.run}` } },
      );
    }
  }
  const verdict = answers.some((answer) => answer.verdict === "not_met")
    ? "not_met"
    : answers.length === survey.criteria.criteria.length && answers.every((answer) => answer.verdict === "met")
      ? "met"
      : "inconclusive";
  return {
    verdict: {
      schema: "runx.verdict.v1",
      verdict,
      subject: survey.subject,
      criteria_set: { ref: "runx/reflect", revision: survey.criteria.revision },
      assessments: answers,
    },
  };
}

/** Each test case a change writes: its path and the name its YAML gives it. */
export function casesOf(writes) {
  return writes
    .filter((write) => /^tests\/[^/]+\.ya?ml$/.test(write.path))
    .map((write) => ({ path: write.path, name: /^name:\s*(\S+)/m.exec(write.contents)?.[1] ?? write.path }));
}

/** A package's files with a change applied: its writes in place, its deletes gone. */
export function applied(files, bundle) {
  const byPath = new Map(files.map((file) => [file.path, file.contents]));
  for (const path of bundle.deletes ?? []) byPath.delete(path);
  for (const write of bundle.writes ?? []) byPath.set(write.path, write.contents);
  return [...byPath].map(([path, contents]) => ({ path, contents })).sort((left, right) => (left.path < right.path ? -1 : 1));
}

/** How each named case came out in a validation: its status, or absent when it didn't run. */
function outcomes(validation, names) {
  const cases = validation?.test?.cases ?? [];
  return Object.fromEntries(names.map((name) => [name, cases.find((entry) => entry.name === name)?.status ?? "absent"]));
}

/**
 * Propose a fix for each kept diagnosis, through skill-build#draft, whose
 * own turns write the fix and its case. Code keeps a proposal only when the
 * draft adds a case, names each case it changes or removes, and two candidate
 * tests prove the new case fails on the old revision and passes on the fix:
 * the workspace's package with only the new case added, then the package
 * with the whole change. Nothing is in the workspace until then. The change
 * is applied by runx.skill.apply under effects: ask, so the person approves
 * the exact files; a decline writes nothing, and its reason is kept, so a
 * later run doesn't propose that change again unless its evidence changed.
 */
export async function propose(ctx, { survey, findings }) {
  const proposals = [];
  const digest = async (value) => (await ctx.tool("data.digest", { value })).digest_result.digest;
  const packageFiles = async (target_dir) => {
    const listed = await ctx.read((await ctx.tool("fs.list", { path: target_dir, depth: 4, max_entries: 256 })).file_list);
    const paths = listed.entries.filter((entry) => entry.kind === "file").map((entry) => entry.path);
    const files = [];
    for (let at = 0; at < paths.length; at += 16) {
      const bundle = await ctx.read((await ctx.tool("fs.read_bundle", { paths: paths.slice(at, at + 16) })).file_read_bundle);
      for (const file of bundle.files) files.push({ path: file.path.slice(target_dir.length + 1), contents: file.contents });
    }
    return files;
  };
  const validate = async (files) =>
    (await ctx.tool("runx.skill.validate", { candidate_files: files, allow_execute_test: false })).skill_validation;

  for (const entry of findings) {
    if (!entry.criterion.startsWith("resisted_") || !entry.finding) continue;
    const item = survey.resistance[Number(entry.criterion.slice("resisted_".length)) - 1];
    const { criterion, finding } = entry;
    const target_dir = `skills/${item.skill}`;
    const key = (await digest({ target_dir, kind: item.kind, evidence: item.evidence, cause: finding.cause })).slice(7);
    const declined = (await ctx.operation("store/key.inspect", { stream: DECLINES, key }))?.provider_operation?.result;
    if ((declined?.version ?? 0) > 0) {
      proposals.push({ criterion, target_dir, declined: declined.last_event?.data?.reason ?? null, again: false });
      continue;
    }
    const old = await packageFiles(target_dir);
    const draft = await ctx.skill("runx/skill-build#draft", {
      objective: `Fix ${item.skill}: ${finding.summary}`,
      repo_root: ".",
      target_dir,
      receipt_id: item.evidence,
      receipt_summary: `${item.kind} at ${item.stream}${item.step ? `#${item.step}` : ""}${item.reason ? `: ${item.reason}` : ""}`,
      test_output: finding.cause,
    });
    const bundle = await ctx.read(draft.change_bundle);
    if (!bundle || bundle.decision !== "write") {
      proposals.push({ criterion, target_dir, dropped: "the draft changes nothing" });
      continue;
    }
    const oldPaths = new Set(old.map((file) => file.path));
    const cases = casesOf(bundle.writes);
    const added = cases.filter((written) => !oldPaths.has(written.path));
    const changed = cases.filter((written) => oldPaths.has(written.path)).map((written) => written.path);
    const removed = (bundle.deletes ?? []).filter((path) => path.startsWith("tests/"));
    if (added.length === 0) {
      proposals.push({ criterion, target_dir, dropped: "it adds no case", changed, removed });
      continue;
    }
    const names = added.map((written) => written.name);
    const addedFiles = bundle.writes.filter((write) => added.some((written) => written.path === write.path));
    const before = outcomes(await validate(applied(old, { writes: addedFiles })), names);
    const afterValidation = await validate(applied(old, bundle));
    const after = outcomes(afterValidation, names);
    const proof = { before, after, passed: afterValidation?.test?.status ?? null };
    if (!names.every((name) => before[name] === "failed")) {
      proposals.push({ criterion, target_dir, dropped: "its new case doesn't fail on the old revision", proof, changed, removed });
      continue;
    }
    if (afterValidation?.test?.status !== "passed" || !names.every((name) => after[name] === "passed")) {
      proposals.push({ criterion, target_dir, dropped: "the fix doesn't pass its cases", proof, changed, removed });
      continue;
    }
    try {
      const result = await ctx.tool("runx.skill.apply", { target_dir, repo_root: ".", mode: "improve", change_bundle: bundle });
      proposals.push({ criterion, target_dir, applied: await ctx.read(result.apply_result), cases: names, changed, removed, proof });
    } catch (rejection) {
      if (rejection?.code !== "declined") throw rejection;
      // A declined call's message is the person's reason.
      const reason = rejection.message ?? null;
      await ctx.operation(
        "store/events.append",
        { stream: DECLINES, key, expected_version: 0, type: "proposal.declined", data: { target_dir, evidence: item.evidence, summary: finding.summary, reason } },
        { idempotency_key: `decline:${key}` },
      );
      proposals.push({ criterion, target_dir, declined: reason, again: false, cases: names, changed, removed, proof });
    }
  }
  return { proposals };
}

/** How many bytes a text is in UTF-8: the worker has no TextEncoder. */
function utf8Bytes(text) {
  let bytes = 0;
  for (const character of text) {
    const point = character.codePointAt(0);
    bytes += point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
  }
  return bytes;
}

/** The heading a promoted rule goes under in the instruction file. */
export const RULES_HEADING = "## Rules";
/** An instruction file reaches a turn whole up to this bound (agent_invocation/profiles.rs). */
const INSTRUCTIONS_BOUND = 128 * 1024;

/**
 * The instruction file with each rule's line added at the end of its
 * Rules section, which it opens when there's none: the person's exact
 * words, then their date and handle, so the repository holds the rule and a
 * later survey reads it held at HEAD.
 */
export function withRules(current, rules) {
  const lines = rules.map((rule) => `- ${rule.text} (${rule.date}, ${rule.handle})`);
  if (lines.length === 0) return current;
  const text = current.endsWith("\n") || current === "" ? current : `${current}\n`;
  const at = text.split("\n").findIndex((line) => line.trim() === RULES_HEADING);
  if (at < 0) return `${text}${text === "" ? "" : "\n"}${RULES_HEADING}\n\n${lines.join("\n")}\n`;
  const all = text.split("\n");
  let end = all.findIndex((line, index) => index > at && /^#{1,2} /.test(line));
  if (end < 0) end = all.length;
  while (end > at + 1 && all[end - 1].trim() === "") end -= 1;
  return [...all.slice(0, end), ...lines, ...all.slice(end)].join("\n").replace(/\n*$/, "\n");
}

/**
 * Promote each standing rule the repository doesn't hold: its exact words
 * into AGENTS.md under its Rules heading, with its date and handle, written
 * under effects: ask so the person approves the exact file. A write that
 * would take the file past what reaches a turn whole is refused, and the
 * rules go elsewhere. Reflection never commits: the person does, and the
 * next survey reads the rule held at HEAD.
 */
export async function promote(ctx, { survey }) {
  if (survey.unheld.length === 0) return { promotion: { promoted: [] } };
  let current = "";
  let expected_digest;
  try {
    const read = await ctx.read((await ctx.tool("fs.read", { path: INSTRUCTIONS, line_count: 100000, max_bytes: INSTRUCTIONS_BOUND + 1 })).file_read);
    if (read.truncated) return { promotion: { promoted: [], refused: `${INSTRUCTIONS} is past ${INSTRUCTIONS_BOUND} bytes already, so no rule is added to it` } };
    current = read.contents;
    expected_digest = read.content_digest;
  } catch (rejection) {
    if (rejection?.code !== "failed") throw rejection;
  }
  const rules = survey.unheld.filter((rule) => !current.includes(rule.handle));
  const next = withRules(current, rules);
  if (utf8Bytes(next) > INSTRUCTIONS_BOUND) {
    return { promotion: { promoted: [], refused: `promoting would take ${INSTRUCTIONS} past ${INSTRUCTIONS_BOUND} bytes, so these rules go elsewhere` } };
  }
  if (rules.length > 0) {
    try {
      await ctx.tool("fs.write", { path: INSTRUCTIONS, contents: next, ...(expected_digest ? { expected_digest } : { create: true }) });
    } catch (rejection) {
      if (rejection?.code !== "declined") throw rejection;
      return { promotion: { promoted: [], declined: rejection.message ?? null } };
    }
  }
  return { promotion: { promoted: survey.unheld.map(({ handle, text, date }) => ({ handle, text, date })) } };
}

/**
 * An observation for each kept diagnosis, with its evidence as its sources,
 * on the workspace's observed stream, dated by its run. One that matches an
 * observation memory already holds is that one, so nothing is kept twice.
 */
export async function observe(ctx, { survey, findings }) {
  const observed = [];
  for (const entry of findings) {
    if (!entry.criterion.startsWith("resisted_") || !entry.finding) continue;
    const item = survey.resistance[Number(entry.criterion.slice("resisted_".length)) - 1];
    const { finding } = entry;
    const kept = await ctx.skill("runx/memory#observe", {
      level: "workspace",
      text: `${item.skill}: ${finding.summary} ${finding.cause}`.slice(0, 4000),
      summary: `${item.skill}: ${finding.summary}`.replace(/\s+/g, " ").slice(0, 120),
      date: item.date,
      sources: finding.evidence.slice(0, 16),
      supersedes: "none",
    });
    const matches = await ctx.read(kept.matches ?? []);
    const handle = kept.handle ?? matches[0]?.data?.handle ?? null;
    observed.push({ criterion: entry.criterion, handle, already: kept.handle === undefined });
  }
  return { observed };
}

/**
 * Close what the record holds now and move the mark: for each rule the
 * repository holds at HEAD, the disable that closes it, its reason naming the
 * commit, which the person runs, since reflection never writes a said:
 * stream; and the mark, moved past the window the verdict covers, at the
 * version survey read, so a concurrent reflection's move wins and this one
 * says it lost.
 */
export async function keep(ctx, { reader, survey }) {
  const disable = survey.held.map((rule) => ({
    skill: "runx/memory#disable",
    handle: rule.handle,
    reason: `Held by ${INSTRUCTIONS} at ${survey.head}, so the repository holds it.`,
  }));
  const { through, at_through, open, version } = survey.mark;
  let mark;
  try {
    const moved = await ctx.operation(
      "store/events.append",
      { stream: MARKS, key: reader, expected_version: version, type: "mark.moved", data: { through, at_through, open } },
      { idempotency_key: `mark:${reader}@${version}` },
    );
    mark = { moved: true, version: moved?.provider_operation?.result?.version ?? version + 1, through };
  } catch (rejection) {
    if (rejection?.code !== "failed" && rejection?.code !== "refused") throw rejection;
    mark = { moved: false, reason: rejection.message ?? null };
  }
  return { kept: { disable, mark } };
}
