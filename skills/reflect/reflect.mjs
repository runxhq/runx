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

/**
 * Who owns a run's package, from where its record says it was: the
 * workspace's own, a path inside it outside `.runx`, is changed where it
 * lies; one runx installed or bundles, or one outside the workspace, is never
 * changed here, and its fix is kept for whoever owns it.
 */
export function ownerOf(skillPath) {
  const path = String(skillPath ?? "");
  if (path === "" || path.startsWith("/") || path === ".." || path.startsWith("../")) return { owned: false, why: `its package lies outside this workspace, at ${path || "an unrecorded path"}, so its fix belongs where it lives` };
  if (path === ".runx" || path.startsWith(".runx/")) {
    const installed = /^\.runx\/added-skills\/([^/]+)\/([^/]+)\//.exec(`${path}/`);
    if (installed) {
      const copy = `skills/${installed[1]}/${installed[2]}`;
      return { owned: false, copy, why: `its package is one runx installed, at ${path}: copy it to ${copy}/, which runx then runs in its place, and reflection fixes it there; until then its fix is kept for its publisher` };
    }
    return { owned: false, why: `its package is runx's own, at ${path}, which reflection never changes` };
  }
  return { owned: true, target_dir: path };
}

/**
 * A file of the package at `target_dir` as the workspace names it: the
 * workspace's own root package is `.`, and its files are named bare.
 */
export function inPackage(target_dir, relative) {
  return target_dir === "." ? relative : `${target_dir}/${relative}`;
}

/** A workspace path inside the package at `target_dir`, from the package's root. */
export function fromPackage(target_dir, path) {
  if (target_dir === ".") return path.startsWith("./") ? path.slice(2) : path;
  return path.startsWith(`${target_dir}/`) ? path.slice(target_dir.length + 1) : path;
}

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
  const at = (record, fields) => ({ run: summary.run_id, skill: summary.skill, skill_path: summary.skill_path, stream: record.stream, date: summary.updated_at.slice(0, 10), ...fields });
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

/** The latest success of each skill, by skill name: a fixed rule over the record. */
export function sampledOf(successes) {
  const latest = new Map();
  for (const run of successes) {
    const held = latest.get(run.skill);
    if (!held || later(run, held)) latest.set(run.skill, run);
  }
  return [...latest.values()]
    .sort((left, right) => (left.skill < right.skill ? -1 : left.skill > right.skill ? 1 : 0))
    .map(({ run_id, skill, skill_path, method, receipt_id, updated_at }) => ({ run: run_id, skill, skill_path, method, evidence: receipt_id, date: updated_at.slice(0, 10) }));
}

function later(left, right) {
  return left.created_at > right.created_at || (left.created_at === right.created_at && left.run_id > right.run_id);
}

/**
 * The line that holds a rule in the instruction file: the person's exact
 * words, then their date and handle. Promotion writes this one form, and a
 * rule is held only where the file holds it as it is.
 */
export function ruleLine(rule) {
  return `- ${rule.text} (${rule.date}, ${rule.handle})`;
}

/**
 * Each standing said:workspace rule, split by what the instruction file
 * holds of it: its line as promotion writes it (held), its handle under other
 * words (edited, which keeps its memory active and shows the person both), or
 * nothing (unheld).
 */
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
  const lines = instructions.split("\n").map((line) => line.trimEnd());
  // Held where the file holds the rule's line whole: its own lines, each as
  // a line of the file, never as part of a longer one.
  const held = (rule) => {
    const wanted = ruleLine(rule).split("\n");
    return lines.some((_, at) => wanted.every((line, offset) => lines[at + offset] === line));
  };
  return {
    unheld: sorted.filter((rule) => !held(rule) && !instructions.includes(rule.handle)),
    held: sorted.filter(held),
    edited: sorted
      .filter((rule) => !held(rule) && instructions.includes(rule.handle))
      .map((rule) => ({ ...rule, wording: lines.find((line) => line.includes(rule.handle))?.trim() ?? "" })),
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
      question: `Did run ${run.run} of ${run.skill} do what ${run.skill}'s purpose asks, judged from its purpose and the run alone, and where it didn't, is why kept as a fix with a case that fails on the old revision and passes on the fix, or as an observation with its sources?`.slice(0, 1024),
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
export async function survey(ctx, { reader, until: given }) {
  // A window ends when it's given, or now: a loop's wake reads up to its time.
  const until = given ?? new Date(await ctx.now()).toISOString();
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
  const inspected = (await read((await ctx.operation("store/key.inspect", { stream: MARKS, key: reader }))?.provider_operation))?.result;
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
  // The runs still owed at the mark first: each read again once it seals.
  // A run whose judgement was unresolved is here too, until its checks are met.
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

  // Only a skill the workspace owns, whose manual the repository holds at
  // HEAD, is sampled, its purpose that manual's description: an installed or
  // bundled skill's purpose isn't the workspace's to judge, and a check no one
  // can answer is no check.
  const sampled = [];
  for (const candidate of sampledOf(successes)) {
    if (sampled.length === SAMPLED_SKILLS || !room(1 + 3)) break;
    const owner = ownerOf(candidate.skill_path);
    if (!owner.owned) continue;
    let purpose;
    try {
      purpose = purposeOf((await read((await tool("git.show", { revision: "HEAD", path: inPackage(owner.target_dir, "SKILL.md") })).git_show)).text);
    } catch (rejection) {
      if (rejection?.code !== "failed") throw rejection;
    }
    if (purpose) sampled.push({ ...candidate, purpose });
  }
  const { unheld, held, edited } = rulesOf(entries, instructions);
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
    edited,
    mark: { version: mark.version, through, at_through: atThrough.sort(), open: [...new Set(open)].sort() },
    read_to_end,
    runs: readRuns.length,
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
    const { purpose } = sampled;
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
 * kept, as a runx.verdict.v1 whose subject is survey's window. A repair, a
 * resistance or a sampled run that didn't do what its purpose asks, is met
 * once its fix and case are proven and applied, or kept as an observation,
 * and a rule once AGENTS.md holds it; until then each is not_met, saying what
 * it lacks, a declined fix with the person's reason. A sampled run that did
 * what its purpose asks is met, and one with no finding kept is unresolved.
 */
export function assessments({ survey, findings, proposals = [], promoted = [], observed = [] }) {
  const observations = new Map(observed.filter((entry) => entry.handle).map((entry) => [entry.criterion, entry.handle]));
  const written = new Set(promoted.map((rule) => rule.handle));
  const by = new Map(findings.map((entry) => [entry.criterion, entry]));
  const proposed = new Map(proposals.map((entry) => [entry.criterion, entry]));
  const where = (item) => `${item.stream}${item.step ? `#${item.step}` : ""}${item.index === undefined ? "" : `/${item.index}`}`;
  // A repair's answer: met once kept, otherwise what it lacks.
  const repaired = (id, evidence, entry, at) => {
    const proposal = proposed.get(id);
    if (proposal?.applied || observations.has(id)) return { id, verdict: "met", evidence };
    return {
      id,
      verdict: "not_met",
      evidence,
      failure: {
        expected: "a fix with a case that fails on the old revision and passes on the fix, or an observation with its sources",
        actual: proposal?.declined !== undefined
          ? `its fix was declined: ${proposal.declined ?? "no reason given"}`
          : proposal?.dropped
            ? `its fix was dropped: ${proposal.dropped}`
            : entry?.finding
              ? `diagnosed, and nothing kept yet: ${entry.finding.summary ?? ""}`.trim()
              : `no diagnosis kept: ${entry?.dropped ?? "none was made"}`,
        where: at,
      },
    };
  };
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
      answers.push(repaired(id, [item.evidence], entry, where(item)));
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
      answers.push(entry.finding.met === true ? { id, verdict: "met", evidence: entry.finding.evidence } : repaired(id, entry.finding.evidence, entry, `runx:run:${run.run}`));
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

/**
 * Each test case a change to the package at `target_dir` writes: its path
 * and the name its YAML gives it. A change names files from the workspace
 * root, each inside the package, as bind admits them.
 */
export function casesOf(target_dir, writes) {
  return writes
    .filter((write) => /^tests\/[^/]+\.ya?ml$/.test(fromPackage(target_dir, write.path)))
    .map((write) => ({ path: write.path, name: /^name:\s*(\S+)/m.exec(write.contents)?.[1] ?? write.path }));
}


/** How each named case came out in a validation: its status, or absent when it didn't run. */
function outcomes(validation, names) {
  const cases = validation?.test?.cases ?? [];
  return Object.fromEntries(names.map((name) => [name, cases.find((entry) => entry.name === name)?.status ?? "absent"]));
}

/**
 * What needs repair, one entry per check: each resistance whose diagnosis
 * was kept, and each sampled success its purpose-only reader found didn't do
 * what its purpose asks, citing the run. A wrong result is repaired as a
 * failure is, so a run that succeeded at the wrong thing is kept and fixed
 * the same way.
 */
export function repairsOf(survey, findings) {
  const repairs = [];
  for (const entry of findings) {
    if (!entry.finding) continue;
    const [kind, ordinal] = [entry.criterion.slice(0, entry.criterion.lastIndexOf("_")), Number(entry.criterion.slice(entry.criterion.lastIndexOf("_") + 1)) - 1];
    if (kind === "resisted") repairs.push({ criterion: entry.criterion, item: survey.resistance[ordinal], finding: entry.finding });
    if (kind === "sampled" && entry.finding.met === false) {
      const run = survey.sampled[ordinal];
      repairs.push({
        criterion: entry.criterion,
        item: { kind: "wrong_result", run: run.run, skill: run.skill, skill_path: run.skill_path, stream: run.run, date: run.date, reason: entry.finding.summary, evidence: run.evidence },
        finding: entry.finding,
      });
    }
  }
  return repairs;
}

/**
 * Propose a fix for each repair to a package the workspace owns, through
 * skill-lab#draft, whose own turns write the fix and its case. Code keeps a
 * proposal only when the draft adds a case, names each case it changes or
 * removes, and two validations where the package lies prove the new case
 * fails on the old revision and passes on the fix: the workspace's package
 * with only the new case added, then the package with the whole change.
 * Nothing is in the workspace until then. The change
 * is applied by runx.skill.apply under effects: ask, so the person approves
 * the exact files; a decline writes nothing, and its reason is kept, so a
 * later run doesn't propose that change again unless its evidence changed.
 */
export async function propose(ctx, { survey, findings }) {
  const proposals = [];
  const digest = async (value) => (await ctx.tool("data.digest", { value })).digest_result.digest;
  // The cases a package holds now, from a listing of its root, which a
  // package with no tests yet has too. Null when the package is too large to
  // list whole, since a case it couldn't see would pass as a new one.
  const casePaths = async (target_dir) => {
    const listed = await ctx.read((await ctx.tool("fs.list", { path: target_dir, depth: 2, max_entries: 5000 })).file_list);
    if (listed.truncated) return null;
    return new Set(listed.entries
      .filter((entry) => entry.kind === "file" && /^tests\/[^/]+$/.test(fromPackage(target_dir, entry.path)))
      .map((entry) => entry.path));
  };
  // Whether the workspace holds the directory at `path`, from listings of
  // each directory above it, which a path that isn't there never fails.
  const holds = async (path) => {
    let parent = ".";
    for (const segment of path.split("/")) {
      const listed = await ctx.read((await ctx.tool("fs.list", { path: parent, depth: 1, max_entries: 5000 })).file_list);
      const child = parent === "." ? segment : `${parent}/${segment}`;
      if (!listed.entries.some((entry) => entry.kind === "directory" && entry.path === child)) return false;
      parent = child;
    }
    return true;
  };
  // The package as it would stand with a change, validated beside where it
  // lies, so the skills it calls resolve as they do for it. A change's paths
  // are named from the package's root.
  const validate = async (target_dir, { writes = [], deletes = [] }) => {
    const relative = (path) => fromPackage(target_dir, path);
    const change = { writes: writes.map((write) => ({ path: relative(write.path), contents: write.contents })), deletes: deletes.map(relative) };
    return ctx.read((await ctx.tool("runx.skill.validate", { skill_path: target_dir, change, allow_execute_test: false })).skill_validation);
  };

  for (const { criterion, item, finding } of repairsOf(survey, findings)) {
    // Reflection never changes its own package, its checks or its cases.
    if (item.skill === SELF) {
      proposals.push({ criterion, target_dir: null, dropped: "reflection never changes its own package" });
      continue;
    }
    // Only the workspace's own package is changed here; the fix for one runx
    // installed or bundles, or one outside the workspace, is kept for its
    // owner as the repair's observation.
    // An installed package is fixed in the person's own copy of it, which
    // runx runs in its place, once they've made one: a copy pins its version,
    // so reflection never makes one itself.
    const owner = ownerOf(item.skill_path);
    const copied = owner.copy !== undefined && (await holds(owner.copy));
    if (!owner.owned && !copied) {
      proposals.push({ criterion, target_dir: null, dropped: owner.why });
      continue;
    }
    const target_dir = owner.owned ? owner.target_dir : owner.copy;
    // Facts only, never a model's words, so a change the person declined is
    // the same change however its cause is put.
    const key = (await digest({ target_dir, kind: item.kind, evidence: item.evidence })).slice(7);
    const declined = (await ctx.read((await ctx.operation("store/key.inspect", { stream: DECLINES, key }))?.provider_operation))?.result;
    if ((declined?.version ?? 0) > 0) {
      proposals.push({ criterion, target_dir, declined: declined.last_event?.data?.reason ?? null, again: false });
      continue;
    }
    const oldCases = await casePaths(target_dir);
    if (oldCases === null) {
      proposals.push({ criterion, target_dir, dropped: `its package at ${target_dir} holds more than 5000 files, too many to tell which cases are new` });
      continue;
    }
    let draft;
    try {
      draft = await ctx.skill("runx/skill-lab#draft", {
        objective: `Fix ${item.skill}: ${finding.summary}`,
        target_dir,
        receipt_id: item.evidence,
        receipt_summary: `${item.kind} at ${item.stream}${item.step ? `#${item.step}` : ""}${item.reason ? `: ${item.reason}` : ""}`,
        test_output: finding.cause ?? finding.summary,
      });
    } catch (rejection) {
      // A draft its plan refuses, or that fails, is this finding's, and the
      // window goes on: it's kept as an observation, never written.
      if (rejection?.code !== "failed") throw rejection;
      proposals.push({ criterion, target_dir, dropped: `the draft failed: ${rejection.message ?? "no reason given"}` });
      continue;
    }
    const bundle = await ctx.read(draft.change_bundle);
    if (!bundle || bundle.decision !== "write") {
      proposals.push({ criterion, target_dir, dropped: "the draft changes nothing" });
      continue;
    }
    const cases = casesOf(target_dir, bundle.writes);
    const added = cases.filter((written) => !oldCases.has(written.path));
    const changed = cases.filter((written) => oldCases.has(written.path)).map((written) => written.path);
    const removed = (bundle.deletes ?? []).filter((path) => fromPackage(target_dir, path).startsWith("tests/"));
    if (added.length === 0) {
      proposals.push({ criterion, target_dir, dropped: "it adds no case", changed, removed });
      continue;
    }
    const names = added.map((written) => written.name);
    const addedFiles = bundle.writes.filter((write) => added.some((written) => written.path === write.path));
    const beforeValidation = await validate(target_dir, { writes: addedFiles });
    const before = outcomes(beforeValidation, names);
    const afterValidation = await validate(target_dir, bundle);
    const after = outcomes(afterValidation, names);
    const proof = { before, after, passed: afterValidation?.test?.status ?? null };
    // A case that never ran, or ran to no verdict, proves nothing either way:
    // its own failures say why, or its package's validation does.
    const validations = [beforeValidation, afterValidation];
    const unproven = names.filter((name) => ![before[name], after[name]].every((status) => status === "passed" || status === "failed"));
    if (unproven.length > 0) {
      // The reason is the side's that reached no verdict: a case that failed
      // as expected before the change says nothing of why it didn't run after.
      const verdictless = (entry) => entry && entry.status !== "passed" && entry.status !== "failed";
      const failures = unproven.flatMap((name) => validations.flatMap((validation) => {
        const entry = validation?.test?.cases?.find((found) => found.name === name);
        return verdictless(entry) ? entry.failures ?? [] : [];
      }));
      const absent = (validation) => unproven.some((name) => !validation?.test?.cases?.some((found) => found.name === name));
      const why = failures.find(Boolean) ?? validations.filter(absent).map((validation) => validation?.test?.reason).find(Boolean);
      proposals.push({ criterion, target_dir, dropped: `its case ${unproven.join(", ")} reached no verdict: ${why ?? "the package's tests didn't run"}`, proof, changed, removed });
      continue;
    }
    if (!names.every((name) => before[name] === "failed")) {
      proposals.push({ criterion, target_dir, dropped: "its new case doesn't fail on the old revision", proof, changed, removed });
      continue;
    }
    if (afterValidation?.test?.status !== "passed" || !names.every((name) => after[name] === "passed")) {
      proposals.push({ criterion, target_dir, dropped: "the fix doesn't pass its cases", proof, changed, removed });
      continue;
    }
    try {
      const result = await ctx.tool("runx.skill.apply", { target_dir, mode: "improve", change_bundle: bundle });
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
  const lines = rules.map(ruleLine);
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
 * An observation for each repair, with its evidence as its sources, on the
 * workspace's observed stream, dated by its run.
 */
export async function observe(ctx, { survey, findings }) {
  const observed = [];
  for (const { criterion, item, finding } of repairsOf(survey, findings)) {
    const kept = await ctx.skill("runx/memory#observe", {
      level: "workspace",
      text: `${item.skill}: ${finding.summary}${finding.cause ? ` ${finding.cause}` : ""}`.slice(0, 4000),
      summary: `${item.skill}: ${finding.summary}`.replace(/\s+/g, " ").slice(0, 120),
      date: item.date,
      sources: finding.evidence.slice(0, 16),
    });
    observed.push({ criterion, handle: kept.handle });
  }
  return { observed };
}

/**
 * Close what the repository holds now and move the mark. Each rule AGENTS.md
 * holds at HEAD is closed through runx/memory#disable, whose own gate asks
 * the person, its reason naming the commit. A close the person declines is
 * kept in the declines stream, keyed by the rule's handle alone, so a later
 * run doesn't ask again until the rule changes. Reflection never keeps or
 * corrects a person's words. Then the mark moves past the window the verdict
 * covers, at the version survey read, so a concurrent reflection's move wins
 * and this one says it lost.
 */
export async function keep(ctx, { reader, survey, proposals = [], promotion = null, observed = [], verdict }) {
  const closed = [];
  for (const rule of survey.held) {
    const key = (await ctx.tool("data.digest", { value: { close: rule.handle } })).digest_result.digest.slice(7);
    const declined = (await ctx.read((await ctx.operation("store/key.inspect", { stream: DECLINES, key }))?.provider_operation))?.result;
    if ((declined?.version ?? 0) > 0) {
      closed.push({ handle: rule.handle, declined: declined.last_event?.data?.reason ?? null, again: false });
      continue;
    }
    const reason = `Held by ${INSTRUCTIONS} at ${survey.head}, so the repository holds it.`;
    try {
      const disabled = await ctx.skill("runx/memory#disable", { handle: rule.handle, reason });
      closed.push({ handle: rule.handle, reason, closed: disabled.handle });
    } catch (rejection) {
      if (rejection?.code !== "declined") throw rejection;
      // A declined call's message is the person's reason.
      const why = rejection.message ?? null;
      await ctx.operation(
        "store/events.append",
        { stream: DECLINES, key, expected_version: 0, type: "close.declined", data: { handle: rule.handle, reason: why } },
        { idempotency_key: `decline:${key}` },
      );
      closed.push({ handle: rule.handle, reason, declined: why, again: false });
    }
  }
  const { through, at_through, version } = survey.mark;
  const assessed = new Map((verdict?.assessments ?? []).map((answer) => [answer.id, answer.verdict]));
  const unresolved = [...new Set([
    ...survey.resistance.filter((_, index) => assessed.get(`resisted_${index + 1}`) !== "met").map((item) => item.run),
    ...survey.sampled.filter((_, index) => assessed.get(`sampled_${index + 1}`) !== "met").map((item) => item.run),
  ])];
  const open = [...new Set([...survey.mark.open, ...unresolved])].sort();
  let mark;
  try {
    const moved = await ctx.operation(
      "store/events.append",
      { stream: MARKS, key: reader, expected_version: version, type: "mark.moved", data: { through, at_through, open } },
      { idempotency_key: `mark:${reader}@${version}` },
    );
    // The store answers a version that moved as a conflict, not a refusal.
    const answer = (await ctx.read(moved?.provider_operation))?.result;
    mark = ["committed", "replayed"].includes(answer?.status)
      ? { moved: true, version: answer.version, through }
      : { moved: false, reason: `the mark's append answered ${answer?.status ?? "nothing"}` };
  } catch (rejection) {
    if (rejection?.code !== "failed" && rejection?.code !== "refused") throw rejection;
    mark = { moved: false, reason: rejection.message ?? null };
  }
  // A rule whose handle the file holds under other words stays active, and
  // the person sees both: the rule as they said it, and what the file says.
  const edited = (survey.edited ?? []).map(({ handle, text, wording }) => ({ handle, said: text, file_says: wording }));
  return { kept: { closed, edited, mark }, loop: windowNotice({ survey, proposals, promotion, observed, closed, unresolved, verdict }) };
}

function counted(count, one, many = `${one}s`) {
  return `${count} ${count === 1 ? one : many}`;
}

/**
 * What a reflection tells its loop, rendered in code: what the window read,
 * what resisted and what was wrong, what was proven and applied, declined or
 * kept only as an observation, and the rules promoted and closed.
 */
export function windowNotice({ survey, proposals, promotion, observed, closed, unresolved = [], verdict }) {
  const applied = proposals.filter((proposal) => proposal.applied).length;
  const declined = proposals.filter((proposal) => proposal.declined !== undefined).length;
  const promoted = promotion?.promoted?.length ?? 0;
  const closes = closed.filter((rule) => rule.closed).length;
  const wrong = (survey.sampled ?? []).length;
  const parts = [
    `Reflected on ${counted(survey.runs ?? 0, "run")}: ${counted((survey.resistance ?? []).length, "thing", "things")} resisted, and ${counted(wrong, "success", "successes")} checked against its skill's purpose.`,
    `${counted(applied, "fix", "fixes")} proven by ${applied === 1 ? "its case" : "their cases"} and applied, ${declined} declined, ${counted(observed.length, "observation")} kept.`,
  ];
  if (promoted > 0 || closes > 0) parts.push(`${counted(promoted, "rule")} written into ${INSTRUCTIONS}, ${closes} closed in memory.`);
  if (survey.read_to_end === false) parts.push("The window was cut short, and the next reflection goes on from where this one stopped.");
  if (unresolved.length > 0) {
    parts.push(`${counted(unresolved.length, "run")} still ${unresolved.length === 1 ? "needs" : "need"} reflection.`);
    for (const answer of verdict?.assessments ?? []) {
      if (answer.verdict !== "met" && answer.failure?.actual) parts.push(answer.failure.actual);
    }
  }
  return { changed: applied + promoted + closes + observed.length > 0, notice: { text: parts.join(" ") } };
}
