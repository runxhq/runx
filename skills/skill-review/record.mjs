// What a script reads of its own run's record, through `run.read`: every page
// of it, and the child run each of its calls of a host started. A turn's own
// claim about what it did is never read; only what the record holds.

// This run's record, every page of it.
export async function readRecord(ctx) {
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
// it made them: the run its call at `<step>/<n>` started. A step's calls are
// on a stream of their own, `<run>/<step>@<attempt>`, beside the run's. The
// record must hold exactly the calls the script made.
export function childRuns({ run, events }, host, made) {
  const root = run.run_id;
  const calls = new Map();
  for (const { stream, event } of events) {
    if (
      event.kind === "call" &&
      stream === `${root}/${event.step_id}@${event.attempt}` &&
      event.target?.kind === "skill" &&
      event.target.skill === host
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
      if (stream === undefined) {
        throw new Error(`this run's record holds no child run for its call ${position}`);
      }
      return stream;
    });
  if (runs.length !== made) {
    throw new Error(`this run's record holds ${runs.length} calls of ${host}, and this script made ${made}`);
  }
  return runs;
}

// Whether `stream` is the child run `run`'s, or one of the streams below it.
export function within(run, stream) {
  return stream === run || stream.startsWith(`${run}/`);
}

// Where the run ran and what started it, as its start recorded them.
export function started({ events }) {
  const start = events.find(({ event }) => event.kind === "started");
  if (start === undefined) throw new Error("this run's record holds no start");
  return { ran_on: start.event.ran_on, implementation: start.event.implementation };
}

// The host that answered the turn in the run `run`, as its answer records
// it, and the model it stated: from the agent step's event in that run.
export function turnOf({ events }, run) {
  const step = events.find(
    ({ stream, event }) => within(run, stream) && event.kind === "step" && (event.host || event.model),
  );
  return { host: step?.event.host ?? null, model: step?.event.model ?? null };
}

// `items` grouped by the host each one's turn records, in first-seen order:
// numbers from different hosts are never blended.
export function byHost(items, turn) {
  const groups = new Map();
  for (const item of items) {
    const { host, model } = turn(item);
    const key = JSON.stringify(host);
    if (!groups.has(key)) groups.set(key, { host, models: new Set(), items: [] });
    const group = groups.get(key);
    if (model !== null) group.models.add(model);
    group.items.push(item);
  }
  return [...groups.values()].map(({ host, models, items }) => ({ host, models: [...models], items }));
}
