export default function planSync(inputs) {
  const { repo, direction, scope, resources } = inputs;
  const blockers = [];
  const filters = { ...(resources.filters ?? {}), limit: resources.filters?.limit ?? 30 };
  const paths = resources.paths ?? [];
  const mutations = resources.mutations ?? [];

  let mutation = null;
  if (direction === "push") {
    if (mutations.length >= 1 && mutations.length <= 8) mutation = mutations[0];
    else blockers.push("push requires 1 to 8 typed mutations");
    if (resources.kind === "batch" && mutations.length === 0) blockers.push("batch push requires at least one mutation");
  } else if (mutations.length > 0) {
    blockers.push("pull does not accept mutations");
  } else if (resources.kind === "batch") {
    blockers.push("batch resources are only valid for push");
  }

  const call = direction === "pull"
    ? pullOperation(repo, resources.kind, filters, paths)
    : direction === "push" ? writeOperation(repo, mutations) : null;
  if (direction === "pull" && blockers.length === 0 && call === null) blockers.push("pull requires a readable resource selector");
  if (direction === "push" && blockers.length === 0 && call === null) {
    blockers.push("a push applies one operation; send issue, pull request and comment changes as separate pushes");
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
      scope_used: direction === "push" ? "repo.write" : "repo.read",
      provider_status: "not_called",
      blockers,
    },
  };
}

// The github operation a pull reads with, and its input: exact numbers when
// the selector names paths, the filtered list otherwise.
/** A cursor is the GitHub page a listing continues from. */
function page(cursor) {
  return cursor === undefined ? undefined : Number(cursor);
}

function pullOperation(repository, kind, filters, paths) {
  const numbers = paths.map((path) => Number(String(path).split("/")[1])).filter(Number.isInteger);
  const defined = (fields) => Object.fromEntries(Object.entries(fields).filter(([, value]) => value !== undefined));
  if (kind === "issues") {
    return numbers.length > 0
      ? { operation: "issues.read", input: { repository, numbers } }
      : {
        operation: "issues.list",
        input: defined({
          repository,
          state: filters.state,
          labels: Array.isArray(filters.labels) ? filters.labels.join(",") : undefined,
          since: filters.since,
          per_page: filters.limit,
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
      ? { operation: "comments.list", input: defined({ repository, number, per_page: filters.limit }) }
      : null;
  }
  return null;
}

// The one github write operation that applies every mutation, or null when
// they need more than one.
function writeOperation(repository, mutations) {
  const calls = new Map();
  const add = (operation, field, item) => {
    if (!calls.has(operation)) calls.set(operation, { operation, input: { repository, [field]: [] } });
    calls.get(operation).input[field].push(item);
  };
  for (const mutation of mutations) {
    const [first, second, third] = String(mutation.path).split("/");
    if (first === "issues" && second === "comments") add("comments.update", "comments", { comment_id: Number(third), body: mutation.payload.body });
    else if (first === "issues" && second === undefined) add("issues.create", "issues", mutation.payload);
    else if (third === "comments") add("comments.create", "comments", { number: Number(second), body: mutation.payload.body });
    else if (first === "issues") add("issues.update", "updates", { number: Number(second), patch: mutation.payload });
    else if (first === "pulls") add("pullrequests.update", "updates", { number: Number(second), patch: mutation.payload });
    else return null;
  }
  return calls.size === 1 ? [...calls.values()][0] : null;
}

// The activity runner: what changed in a repository since a time, for a
// reader. GitHub's issue listing holds pull requests too, so one read finds
// both; its whole records stay in the read's provider evidence, and the
// reader gets the fields a summary needs.

/** Items a page of activity holds: GitHub's largest page. */
const ACTIVITY_PAGE = 100;

/** The characters of a body an item keeps; it says how many it left out. */
const BODY_CHARS = 600;

/** The `issues.list` input that reads one page of everything updated since `since`. */
export function activityWindow({ repo, since, page }) {
  return {
    read_input: { repository: repo, since, state: "all", per_page: ACTIVITY_PAGE, page: page ?? 1 },
  };
}

/** The page the read returned, newest change first, and the next page when GitHub filled this one. */
export function digestActivity({ read_input: read, provider_operation: operation }) {
  const issues = Array.isArray(operation?.result?.issues) ? operation.result.issues : [];
  const items = issues
    .map((issue) => activityItem(issue, Date.parse(read.since)))
    .sort((a, b) => Date.parse(b.updated_at) - Date.parse(a.updated_at));
  return {
    activity: {
      repository: read.repository,
      since: read.since,
      page: read.page,
      items,
      ...(issues.length === read.per_page ? { next_page: read.page + 1 } : {}),
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
    labels: (issue.labels ?? []).map((label) => (typeof label === "string" ? label : label.name)),
    assignees: (issue.assignees ?? []).map((assignee) => assignee.login),
    comments: issue.comments ?? 0,
    url: issue.html_url,
    body: body.slice(0, BODY_CHARS),
    body_left_out: Math.max(0, body.length - BODY_CHARS),
  };
}
