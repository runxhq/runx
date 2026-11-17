export default function planSync(inputs) {
  const { repo, direction, scope, resources } = inputs;
  const blockers = [];
  const filters = {
    ...(resources.filters ?? {}),
    limit: resources.filters?.limit ?? 30,
  };
  const paths = resources.paths ?? [];
  const mutations = resources.mutations ?? [];

  let mutation = null;
  if (direction === "push") {
    if (mutations.length > 0) mutation = mutations[0];
    else blockers.push("push requires at least one typed mutation");
    if (resources.kind === "batch" && mutations.length === 0)
      blockers.push("batch push requires at least one mutation");
  } else if (mutations.length > 0) {
    blockers.push("pull does not accept mutations");
  } else if (resources.kind === "batch") {
    blockers.push("batch resources are only valid for push");
  }

  const call =
    direction === "pull"
      ? pullOperation(repo, resources.kind, filters, paths)
      : direction === "push"
        ? writeOperation(repo, mutations)
        : null;
  if (direction === "pull" && blockers.length === 0 && call === null)
    blockers.push("pull requires a readable resource selector");
  if (direction === "push" && blockers.length === 0 && call === null) {
    blockers.push(
      "a push applies one operation; send issue, pull request and comment changes as separate pushes",
    );
  }

  let decision = blockers.length === 0 ? "ready" : "blocked";
  if (direction === "push" && scope !== "write") {
    blockers.push("push requires requested write scope");
    decision = "refused";
  } else if (direction === "pull" && scope !== "read") {
    blockers.push("pull requires requested read scope");
    decision = "refused";
  } else if (blockers.length === 0 && direction === "push") {
    decision = "ready_for_execution";
  }

  return {
    sync_plan: {
      decision,
      repo,
      direction,
      resource_selector: {
        kind: resources.kind,
        filters,
        paths,
      },
      resources_touched: [],
      mutation,
      mutations,
      diff_summary: mutations.map((entry) => ({
        path: entry.path,
        op: entry.op,
        fields: Object.keys(entry.payload).sort(),
      })),
      provider_operation: call?.operation ?? "",
      provider_input: call?.input ?? null,
      scope_used:
        direction === "push"
          ? call?.operation.startsWith("comments.")
            ? "issue.comment"
            : "repo.write"
          : "repo.read",
      provider_status: "not_called",
      blockers,
    },
  };
}

// The github operation a pull reads with, and its input: exact numbers when
// the selector names paths, the filtered list otherwise.
/** A cursor is the GitHub page a listing continues from. */
function page(cursor) {
  if (cursor === undefined) return undefined;
  const value = Number(cursor);
  if (!Number.isSafeInteger(value) || value < 1)
    throw new Error(
      "GitHub page must be a positive portable integer; do not round a cursor.",
    );
  return value;
}

function pullOperation(repository, kind, filters, paths) {
  const numbers = paths
    .map((path) => Number(String(path).split("/")[1]))
    .filter(Number.isInteger);
  const defined = (fields) =>
    Object.fromEntries(
      Object.entries(fields).filter(([, value]) => value !== undefined),
    );
  if (kind === "issues") {
    return numbers.length > 0
      ? { operation: "issues.read", input: { repository, numbers } }
      : {
          operation: "issues.list",
          input: defined({
            repository,
            state: filters.state,
            labels: Array.isArray(filters.labels)
              ? filters.labels.join(",")
              : undefined,
            since: filters.since,
            per_page: filters.limit,
            sort: filters.sort,
            direction: filters.direction,
            page: page(filters.cursor),
          }),
        };
  }
  if (kind === "prs") {
    return numbers.length > 0
      ? { operation: "pullrequests.read", input: { repository, numbers } }
      : {
          operation: "pullrequests.list",
          input: defined({
            repository,
            state: filters.state,
            base: filters.base,
            head: filters.head,
            sort: filters.sort,
            direction: filters.direction,
            per_page: filters.limit,
            page: page(filters.cursor),
          }),
        };
  }
  if (kind === "threads") {
    const thread = String(filters.thread_path ?? paths[0] ?? "");
    const number = Number(thread.split("/")[1]);
    return Number.isInteger(number) && number > 0
      ? {
          operation: "comments.list",
          input: defined({
            repository,
            number,
            per_page: filters.limit,
            page: page(filters.cursor),
          }),
        }
      : null;
  }
  return null;
}

// The one github write operation that applies every mutation, or null when
// they need more than one.
function writeOperation(repository, mutations) {
  const calls = new Map();
  const add = (operation, field, item) => {
    if (!calls.has(operation))
      calls.set(operation, { operation, input: { repository, [field]: [] } });
    calls.get(operation).input[field].push(item);
  };
  for (const mutation of mutations) {
    const [first, second, third] = String(mutation.path).split("/");
    if (first === "issues" && second === "comments")
      add("comments.update", "comments", {
        comment_id: Number(third),
        body: mutation.payload.body,
      });
    else if (first === "issues" && second === undefined)
      add("issues.create", "issues", mutation.payload);
    else if (third === "comments")
      add("comments.create", "comments", {
        number: Number(second),
        body: mutation.payload.body,
      });
    else if (first === "issues")
      add("issues.update", "updates", {
        number: Number(second),
        patch: mutation.payload,
      });
    else if (first === "pulls")
      add("pullrequests.update", "updates", {
        number: Number(second),
        patch: mutation.payload,
      });
    else return null;
  }
  return calls.size === 1 ? [...calls.values()][0] : null;
}

// Coverage (coverage.md): activity reads what changed in each repository
// since its position, to the end, and writes nothing; follow is what a
// schedule runs, keeping each reader's positions in this package's own
// `positions` stream and moving each only past what it read and stored.
//
// A read is a keyset read: each page asks for everything updated since the
// mark, oldest first, and drops what was already read at the mark. Page
// numbers would skip an item whenever another is updated mid-read, since the
// updated one moves to the end and everything after it shifts back; a keyset
// read only ever finds an updated item later, where it gets read. When a
// whole page shares the mark's timestamp, the read goes on by page within it,
// and the mark stays there with the ids read at it.

/** Items a page asks for: GitHub's largest page. */
const PAGE = 100;

/**
 * The items one activity read returns across its repositories, so its result
 * crosses the worker whole. A read that reaches it stops where it stands, and
 * its positions carry the rest to the next read.
 */
const READ_ITEMS = 2000;

/** The query every position is a position in. A changed query starts new positions. */
export const QUERY = Object.freeze({
  state: "all",
  sort: "updated",
  direction: "asc",
  per_page: PAGE,
});

/** Where a repository with no position and no `since` is read from. */
const BEGINNING = "1970-01-01T00:00:00Z";

/** The characters of a body an item keeps; it says how many it left out. */
const BODY_CHARS = 600;

/** A read of one repository, before its first page. */
export function begun({ repository, since, position }) {
  const mark = position ?? { updated_at: since ?? BEGINNING, ids_at_mark: [] };
  return { repository, mark, page: 1, since: mark.updated_at, items: [], done: false };
}

/** The `issues.list` input for a read's next page. */
export function pageInput(reading) {
  return {
    repository: reading.repository,
    ...QUERY,
    since: reading.mark.updated_at,
    page: reading.page,
  };
}

/**
 * The read after one page: each item not already read, in order, an item
 * updated again during the read kept once at its newest, and the mark at the
 * newest change read, with the ids read at it. A page shorter than a page is
 * the end, which is a fact of GitHub's answer. A page that never moved the
 * mark asks for the next page at the same mark.
 */
export function turned(reading, issues) {
  const asked = reading.mark.updated_at;
  let mark = asked;
  let atMark = new Set(reading.mark.ids_at_mark);
  const items = [...reading.items];
  for (const issue of issues) {
    const at = issue.updated_at;
    if (at === mark && atMark.has(issue.number)) continue;
    const earlier = items.findIndex((item) => item.number === issue.number);
    if (earlier >= 0) items.splice(earlier, 1);
    items.push(activityItem(issue, Date.parse(reading.since)));
    if (at === mark) atMark.add(issue.number);
    else if (Date.parse(at) > Date.parse(mark)) {
      mark = at;
      atMark = new Set([issue.number]);
    }
  }
  const full = issues.length === PAGE;
  return {
    ...reading,
    mark: { updated_at: mark, ids_at_mark: [...atMark].sort((a, b) => a - b) },
    page: full && mark === asked ? reading.page + 1 : 1,
    items,
    done: !full,
  };
}

/**
 * activity: each repository read from its position, or from `since`, page by
 * page to the end. A repository that fails is reported with its error and the
 * rest go on, so a failure never reads as nothing changed. A read that
 * reaches its calls or its items stops where it stands: that repository is
 * incomplete at its position, and the rest are unstarted.
 */
export async function activity(ctx, { repositories }) {
  const coverage = [];
  const evidence = [];
  // Each page is a call and the read of its answer.
  let calls = 0;
  let room = READ_ITEMS;
  for (const entry of repositories) {
    if (calls + 2 > ctx.max_calls || room <= 0) {
      const { repository, mark } = begun(entry);
      coverage.push({ repository, started: false, complete: false, items: [], position: mark });
      continue;
    }
    let reading = begun(entry);
    let error;
    while (!reading.done && calls + 2 <= ctx.max_calls && reading.items.length < room) {
      calls += 2;
      let read;
      try {
        read = await ctx.read((await ctx.operation("github/issues.list", pageInput(reading)))?.provider_operation);
      } catch (rejection) {
        error = { code: rejection.code, message: rejection.message };
        break;
      }
      evidence.push(read.plan_digest);
      if (!Array.isArray(read.result?.issues)) {
        error = {
          code: "unreadable",
          message: `GitHub's issue list for ${reading.repository} is not a list, so nothing was read from it.`,
        };
        break;
      }
      reading = turned(reading, read.result.issues);
    }
    room -= reading.items.length;
    coverage.push({
      repository: reading.repository,
      started: true,
      complete: reading.done && error === undefined,
      ...(error === undefined ? {} : { error }),
      items: reading.items,
      position: reading.mark,
    });
  }
  return { coverage, evidence };
}

/** What follow's verdict answers: one required check. */
const COVERAGE_CRITERIA = [
  {
    id: "every_repository_read",
    required: true,
    question: "Was every repository read to the end of the window?",
  },
];

/** The stream positions are kept in, in this package's own store. */
const POSITIONS = "positions";

/**
 * follow: what a schedule runs. Each repository's position is read for this
 * reader, account and query, activity reads from there, what it read is
 * stored as a fragment, and only then does the position move, at the version
 * it was read at. A move that loses to a concurrent one reads again from the
 * winner's position. A run that neither completes a repository nor moves a
 * position fails, since it made no progress.
 */
export async function follow(ctx, { repositories, reader, until, principal }) {
  let calls = 0;
  const room = (needed) => calls + needed <= ctx.max_calls;
  const tool = async (name, inputs) => {
    calls += 1;
    return ctx.tool(name, inputs);
  };
  const operation = async (name, inputs, options) => {
    calls += 1;
    return ctx.operation(name, inputs, options);
  };
  const digest = async (value) => (await tool("data.digest", { value })).digest_result.digest;
  // The digest names the reader, the account and the query, and the key
  // then names the repository, inside the store's bound on a key.
  const prefix = (await digest({ reader, principal: principal.id, query: QUERY })).slice(7, 39);
  const keyOf = (repository) => `${prefix}:${repository}`;
  const subject = {
    ref: "runx/github.coverage/window",
    revision: await digest({ repositories: [...repositories].sort(), until }),
  };
  const { criteria } = await tool("verdict.criteria", { criteria: COVERAGE_CRITERIA });
  const now = new Date(await ctx.now()).toISOString().replace(/\.\d{3}Z$/, "Z");
  // A complete read checks a repository through now, and so through the
  // window's end once that has passed.
  const checkedThrough = Date.parse(now) >= Date.parse(until) ? until : now;
  const evidence = [];

  const positionOf = async (repository) => {
    const inspected = await operation("store/key.inspect", { stream: POSITIONS, key: keyOf(repository) });
    const read = inspected?.provider_operation?.result;
    return { version: read?.version ?? 0, stored: read?.last_event?.data };
  };
  // A read through activity is a call and, at most, a read of each of its
  // two fields.
  const READ_CALLS = 3;
  const readFrom = async (positions) => {
    calls += READ_CALLS;
    const read = await ctx.skill(".#activity", {
      repositories: positions.map(({ repository, stored }) =>
        stored === undefined
          ? { repository }
          : { repository, position: { updated_at: stored.updated_at, ids_at_mark: stored.ids_at_mark } },
      ),
    });
    evidence.push(...(await ctx.read(read.evidence)));
    return ctx.read(read.coverage);
  };

  const positions = [];
  for (const repository of repositories) positions.push({ repository, ...(await positionOf(repository)) });
  const read = await readFrom(positions);
  let budget = read.some((entry) => !entry.complete && entry.error === undefined);
  let moved = false;
  const coverage = [];
  for (const [index, position] of positions.entries()) {
    let { version, stored } = position;
    let entry = read[index];
    let error = entry.error;
    let fragments = [];
    // Until the position moves, or there's nothing to move it by.
    for (;;) {
      const checked = entry.complete ? checkedThrough : stored?.checked_through;
      const progress = entry.items.length > 0 || (entry.complete && checked !== stored?.checked_through);
      if (!progress) break;
      if (!room(2)) {
        budget = true;
        break;
      }
      try {
        // What was read is stored first, so a position never moves past
        // results the reader can't reach.
        const stored_fragments =
          entry.items.length === 0
            ? []
            : [(await tool("artifact.store", { value: { repository: position.repository, items: entry.items } })).artifact.digest];
        const data = {
          updated_at: entry.position.updated_at,
          ids_at_mark: entry.position.ids_at_mark,
          ...(checked === undefined ? {} : { checked_through: checked }),
          fragments: stored_fragments,
        };
        const appended = await operation(
          "store/events.append",
          { stream: POSITIONS, key: keyOf(position.repository), expected_version: version, type: "position.moved", data },
          { idempotency_key: `position:${keyOf(position.repository)}@${version}` },
        );
        // The store answers a version that moved as a conflict, not a
        // refusal: it takes the same way back.
        const status = appended.provider_operation?.result?.status;
        if (status !== "committed" && status !== "replayed") {
          throw { code: "refused", message: `the position's append answered ${status}` };
        }
        evidence.push(appended.provider_operation.plan_digest);
        stored = data;
        fragments = stored_fragments;
        moved = true;
        break;
      } catch (rejection) {
        // A key past the version this read it at lost to a concurrent move:
        // read again from the winner's position. Anything else, a store that
        // failed among them, is this repository's failure, and its position
        // stays where it was.
        if (!room(1 + READ_CALLS)) {
          budget = true;
          break;
        }
        const current = await positionOf(position.repository);
        if (current.version === version) {
          error = { code: rejection.code, message: rejection.message };
          break;
        }
        ({ version, stored } = current);
        [entry] = await readFrom([{ repository: position.repository, stored }]);
        error = entry.error;
        budget ||= !entry.complete && entry.error === undefined;
      }
    }
    coverage.push({
      repository: position.repository,
      position: stored === undefined ? null : { updated_at: stored.updated_at, ids_at_mark: stored.ids_at_mark },
      checked_through: stored?.checked_through ?? null,
      complete: entry.complete && error === undefined && stored?.checked_through === checkedThrough,
      ...(error === undefined ? {} : { error }),
      fragments,
    });
  }
  if (!moved && !coverage.some((entry) => entry.complete)) {
    throw new Error(
      `No repository was read to its end and no position moved: ${coverage
        .map((entry) => `${entry.repository}: ${entry.error?.message ?? "not read"}`)
        .join("; ")}`,
    );
  }
  return { coverage, budget_exhausted: budget, evidence, subject, criteria };
}

/**
 * The verdict on follow's coverage, against runx/github.coverage: met only
 * when every repository is checked through the window's end. Each one that
 * isn't is named in its failure, with why.
 */
export function coverageVerdict({ coverage, budget_exhausted, evidence, subject, criteria, until }) {
  const short = coverage.filter(
    (entry) => entry.checked_through === null || Date.parse(entry.checked_through) < Date.parse(until),
  );
  const assessment =
    short.length === 0
      ? { id: "every_repository_read", verdict: "met", evidence }
      : {
          id: "every_repository_read",
          verdict: "not_met",
          evidence,
          failure: {
            expected: `every repository checked through ${until}`,
            actual: short
              .map(
                (entry) =>
                  `${entry.repository} ${entry.checked_through === null ? "never checked" : `checked through ${entry.checked_through}`}${entry.error === undefined ? "" : `, ${entry.error.message}`}`,
              )
              .join("; "),
            where: short.map((entry) => `runx/github/positions#${entry.repository}`).join(", "),
          },
        };
  return {
    coverage,
    budget_exhausted,
    verdict: {
      schema: "runx.verdict.v1",
      verdict: assessment.verdict,
      subject,
      criteria_set: { ref: "runx/github.coverage", revision: criteria.revision },
      assessments: [assessment],
    },
  };
}

function activityItem(issue, since) {
  const pull = issue.pull_request;
  const body = typeof issue.body === "string" ? issue.body : "";
  return {
    number: issue.number,
    kind: pull === undefined ? "issue" : "pull_request",
    title: issue.title,
    state: pull?.merged_at ? "merged" : issue.state,
    new: Date.parse(issue.created_at) >= since,
    author: issue.user?.login ?? null,
    bot: issue.user?.type === "Bot",
    created_at: issue.created_at,
    updated_at: issue.updated_at,
    closed_at: issue.closed_at ?? null,
    labels: (issue.labels ?? []).map((label) =>
      typeof label === "string" ? label : label.name,
    ),
    assignees: (issue.assignees ?? []).map((assignee) => assignee.login),
    comments: issue.comments ?? 0,
    url: issue.html_url,
    body: body.slice(0, BODY_CHARS),
    body_left_out: Math.max(0, body.length - BODY_CHARS),
  };
}

/** A common read uses the same typed operation planner as explicit sync. */
export function planRead({
  repo,
  kind = "issues",
  number,
  state = "open",
  labels = [],
  since,
  limit = 30,
  page: requestedPage = 1,
}) {
  const resourceKind = {
    issues: "issues",
    pull_requests: "prs",
    comments: "threads",
  }[kind];
  const filters = { limit, cursor: String(requestedPage) };
  if (kind !== "comments") filters.state = state;
  if (kind === "issues" && labels.length) filters.labels = labels;
  if (kind === "issues" && since !== undefined) filters.since = since;
  const prefix = kind === "pull_requests" ? "pulls" : "issues";
  const paths =
    number === undefined
      ? []
      : [`${prefix}/${number}${kind === "comments" ? "/comments" : ""}`];
  const planned = planSync({
    repo,
    direction: "pull",
    scope: "read",
    resources: { kind: resourceKind, filters, paths },
  });
  if (kind === "comments" && number === undefined)
    planned.sync_plan.blockers.push(
      "A comments read needs the exact issue or pull-request number.",
    );
  if (kind !== "issues" && (labels.length || since !== undefined))
    planned.sync_plan.blockers.push(
      "Labels and since filter the issue feed; choose issues or use the explicit pull selector for other queries.",
    );
  if (planned.sync_plan.blockers.length) planned.sync_plan.decision = "blocked";
  return planned;
}
