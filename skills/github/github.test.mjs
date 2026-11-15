import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";
import { activity, begun, follow, pageInput, planRead, turned } from "./github.mjs";

const SINCE = "2026-10-19T21:00:00Z";

/** A GitHub issue record, with the fields `activity` reads and some it leaves behind. */
function issue(number, fields = {}) {
  return {
    number,
    title: `Item ${number}`,
    state: "open",
    user: {
      login: "ada",
      type: "User",
      avatar_url: "https://avatars.example/ada",
    },
    created_at: "2026-10-19T22:00:00Z",
    updated_at: "2026-10-19T22:00:00Z",
    closed_at: null,
    labels: [{ name: "bug", color: "d73a4a" }],
    assignees: [],
    comments: 0,
    html_url: `https://github.com/runxhq/runx/issues/${number}`,
    repository_url: "https://api.github.com/repos/runxhq/runx",
    body: "",
    ...fields,
  };
}

/** `count` issues numbered from `first`, updated at `at`, or each a second apart from it. */
function issues(first, count, at, apart = false) {
  return Array.from({ length: count }, (_, index) =>
    issue(first + index, {
      updated_at: apart ? new Date(Date.parse(at) + index * 1000).toISOString().replace(".000", "") : at,
    }),
  );
}

const read = (entry, ...pages) => pages.reduce(turned, begun(entry));

test("a read asks for everything updated since its mark, oldest first, from a position, a time or the beginning", () => {
  const query = { repository: "runxhq/runx", state: "all", sort: "updated", direction: "asc", per_page: 100, page: 1 };
  assert.deepEqual(pageInput(begun({ repository: "runxhq/runx", since: SINCE })), { ...query, since: SINCE });
  const position = { updated_at: "2026-10-20T00:00:00Z", ids_at_mark: [4] };
  assert.deepEqual(pageInput(begun({ repository: "runxhq/runx", since: SINCE, position })), {
    ...query,
    since: position.updated_at,
  });
  assert.equal(pageInput(begun({ repository: "runxhq/runx" })).since, "1970-01-01T00:00:00Z");
});

test("an item updated mid-read is read at its newest, once, and nothing shifts out of the read", () => {
  // The first page is full, so the read asks again from its newest change,
  // and meanwhile item 5 was updated: GitHub returns it after the mark.
  const first = issues(1, 100, "2026-10-20T00:00:00Z", true);
  const mark = first[99].updated_at;
  const reading = read({ repository: "runxhq/runx", since: SINCE }, first, [
    first[99],
    issue(5, { updated_at: "2026-10-21T00:00:00Z" }),
  ]);
  assert.equal(reading.done, true);
  assert.equal(reading.items.length, 100);
  assert.deepEqual(reading.items.at(-1).number, 5);
  assert.equal(reading.items.at(-1).updated_at, "2026-10-21T00:00:00Z");
  assert.deepEqual(reading.mark, { updated_at: "2026-10-21T00:00:00Z", ids_at_mark: [5] });
  assert.ok(Date.parse(mark) < Date.parse(reading.mark.updated_at));
});

test("a full page sharing one timestamp reads on by page there, and a read from that mark drops what it read", () => {
  const at = "2026-10-20T00:00:00Z";
  const first = read({ repository: "runxhq/runx", since: at }, issues(1, 100, at));
  assert.equal(first.done, false);
  assert.deepEqual(pageInput(first), { ...pageInput(begun({ repository: "runxhq/runx", since: at })), page: 2 });
  const second = turned(first, issues(101, 30, at));
  assert.equal(second.done, true);
  assert.equal(second.items.length, 130);
  assert.deepEqual(second.mark, { updated_at: at, ids_at_mark: Array.from({ length: 130 }, (_, index) => index + 1) });
  const next = read({ repository: "runxhq/runx", position: second.mark }, [
    ...issues(1, 130, at).slice(0, 99),
    issue(131, { updated_at: "2026-10-20T00:00:01Z" }),
  ]);
  assert.deepEqual(next.items.map(({ number }) => number), [131]);
});

test("keeps what a reader needs, a body's first 600 characters, and leaves GitHub's whole record behind", () => {
  const { items } = read({ repository: "runxhq/runx", since: SINCE }, [
    issue(7, { created_at: "2026-10-18T09:00:00Z", updated_at: "2026-10-19T23:00:00Z", comments: 2, body: "x".repeat(650) }),
    issue(9, {
      pull_request: { merged_at: "2026-10-20T01:00:00Z" },
      state: "closed",
      closed_at: "2026-10-20T01:00:00Z",
      updated_at: "2026-10-20T01:00:00Z",
      user: { login: "dependabot[bot]", type: "Bot" },
      body: null,
    }),
  ]);
  assert.deepEqual(
    items.map(({ number, kind, state, new: opened, author, bot, body_left_out }) => ({ number, kind, state, opened, author, bot, body_left_out })),
    [
      { number: 7, kind: "issue", state: "open", opened: false, author: "ada", bot: false, body_left_out: 50 },
      { number: 9, kind: "pull_request", state: "merged", opened: true, author: "dependabot[bot]", bot: true, body_left_out: 0 },
    ],
  );
  assert.deepEqual(items[0].labels, ["bug"]);
  assert.equal(items[0].body.length, 600);
  assert.equal(items[1].body, "");
  assert.equal("repository_url" in items[0], false);
});

/**
 * A script's ctx over a fake GitHub, whose answer for each repository is a
 * list of pages or a rejection, and an in-memory store, as the runtime gives
 * calls: each result whole, each failure a rejection with its code.
 */
/** What GitHub returns of a page for `since`: the items updated at or after it, or a malformed answer as it is. */
const since = (page, at) => (Array.isArray(page) ? page.filter(({ updated_at }) => updated_at >= at) : page);

function context({ github, max_calls = 400, store = new Map(), moved = () => {}, unstored = [] }) {
  const now = Date.parse("2026-11-15T21:00:00Z");
  const asked = new Map();
  const reject = (code, message) => Object.assign(new Error(message), { code, message });
  const ctx = {
    max_calls,
    store,
    now: async () => now,
    read: async (value) => value,
    tool: async (name, { value, criteria }) => {
      if (name === "data.digest")
        return { digest_result: { digest: `sha256:${createHash("sha256").update(JSON.stringify(value)).digest("hex")}` } };
      if (name === "verdict.criteria") return { criteria: { schema: "runx.criteria.v1", revision: "sha256:criteria", criteria } };
      if (name === "artifact.store") {
        if (unstored.includes(value.repository)) throw reject("failed", "the artifact store refused the value");
        return { artifact: { digest: `sha256:fragment-${value.repository}-${value.items.length}` } };
      }
      throw new Error(`no tool ${name}`);
    },
    skill: async (_reference, inputs) => activity({ ...ctx, max_calls: 400 }, inputs),
    operation: async (name, inputs) => {
      if (name === "github/issues.list") {
        const answer = github[inputs.repository];
        if (answer instanceof Error) throw answer;
        const page = asked.get(inputs.repository) ?? 0;
        asked.set(inputs.repository, page + 1);
        return { provider_operation: { plan_digest: `sha256:read-${inputs.repository}-${page}`, result: { issues: since(answer[page] ?? [], inputs.since) } } };
      }
      const held = store.get(inputs.key) ?? [];
      if (name === "store/key.inspect") {
        return { provider_operation: { result: { version: held.length, last_event: held.at(-1) ?? null } } };
      }
      moved(inputs.key, store);
      const current = store.get(inputs.key) ?? [];
      if (current.length !== inputs.expected_version) throw reject("failed", `${inputs.key} is at version ${current.length}`);
      store.set(inputs.key, [...current, { type: inputs.type, data: inputs.data }]);
      return { provider_operation: { plan_digest: `sha256:append-${inputs.key}-${current.length}` } };
    },
  };
  return ctx;
}

test("one repository failing leaves the rest read, an answer that isn't a list is a failure, and a read out of calls leaves the rest unstarted", async () => {
  const at = "2026-10-20T00:00:00Z";
  const github = {
    "runxhq/a": [issues(1, 2, at)],
    "runxhq/down": Object.assign(new Error("GitHub answered HTTP 502"), { code: "failed" }),
    "runxhq/odd": [{ message: "Not Found" }],
    "runxhq/late": [issues(1, 1, at)],
  };
  const repositories = Object.keys(github).map((repository) => ({ repository, since: SINCE }));
  const { coverage } = await activity(context({ github }), { repositories });
  assert.deepEqual(
    coverage.map(({ repository, started, complete, error, items }) => [repository, started, complete, error?.code, items.length]),
    [
      ["runxhq/a", true, true, undefined, 2],
      ["runxhq/down", true, false, "failed", 0],
      ["runxhq/odd", true, false, "unreadable", 0],
      ["runxhq/late", true, true, undefined, 1],
    ],
  );
  const short = await activity(context({ github, max_calls: 2 }), { repositories });
  assert.deepEqual(short.coverage.map(({ started }) => started), [true, false, false, false]);
});

test("follow keeps a position per reader, account and query, loses a concurrent move by reading again, and a run out of calls makes progress the next run continues", async () => {
  const at = "2026-10-20T00:00:00Z";
  const github = { "runxhq/a": [issues(1, 2, at, true)] };
  const inputs = { repositories: ["runxhq/a"], reader: "morning", until: "2026-11-15T21:00:00Z", principal: { id: "7", login: "ada" } };
  const ctx = context({ github });
  const first = await follow(ctx, inputs);
  assert.equal(first.coverage[0].complete, true);
  assert.equal(first.coverage[0].checked_through, inputs.until);
  assert.deepEqual(first.coverage[0].fragments, ["sha256:fragment-runxhq/a-2"]);
  // Another reader, another account or another query never reads this one's position.
  const keys = [...ctx.store.keys()];
  assert.equal(keys.length, 1);
  const elsewhere = context({ github });
  await follow(elsewhere, { ...inputs, reader: "evening" });
  await follow(elsewhere, { ...inputs, principal: { id: "8", login: "ada" } });
  assert.equal(new Set([...elsewhere.store.keys(), ...keys]).size, 3);
  // A rotated token is the same account, so the same key: login never keys it.
  const renamed = context({ github });
  await follow(renamed, { ...inputs, principal: { id: "7", login: "ada-renamed" } });
  assert.deepEqual([...renamed.store.keys()], keys);

  // A concurrent follower moves the position first: this one's append loses,
  // and it reads again from the winner's position.
  const racing = context({
    github: { "runxhq/a": [issues(1, 2, at, true), [issue(3, { updated_at: "2026-10-21T00:00:00Z" })]] },
    moved: (key, store) => {
      if ((store.get(key) ?? []).length === 0)
        store.set(key, [{ type: "position.moved", data: { updated_at: issues(1, 2, at, true)[1].updated_at, ids_at_mark: [2], fragments: [] } }]);
    },
  });
  const raced = await follow(racing, inputs);
  assert.deepEqual(raced.coverage[0].position, { updated_at: "2026-10-21T00:00:00Z", ids_at_mark: [3] });
  assert.equal([...racing.store.values()][0].length, 2);

  // Out of calls before b's append, a run moves a, says so, and leaves b
  // where it was; the next run reads only what's left.
  const both = { ...inputs, repositories: ["runxhq/a", "runxhq/b"] };
  const wire = { "runxhq/a": [issues(1, 2, at, true)], "runxhq/b": [issues(1, 1, at)] };
  const store = new Map();
  const stopped = await follow(context({ github: wire, max_calls: 10, store }), both);
  assert.equal(stopped.budget_exhausted, true);
  assert.deepEqual(stopped.coverage.map(({ position }) => position?.ids_at_mark ?? null), [[2], null]);
  const resumed = await follow(context({ github: wire, store }), both);
  assert.equal(resumed.budget_exhausted, false);
  assert.deepEqual(resumed.coverage.map(({ complete, fragments }) => [complete, fragments.length]), [[true, 0], [true, 1]]);

  // What was read and couldn't be stored never moves its position: it's
  // that repository's failure, and the rest move.
  const unstored = new Map();
  const held = await follow(context({ github: wire, store: unstored, unstored: ["runxhq/a"] }), both);
  assert.deepEqual(
    held.coverage.map(({ repository, position, error }) => [repository, position, error?.message]),
    [
      ["runxhq/a", null, "the artifact store refused the value"],
      ["runxhq/b", { updated_at: at, ids_at_mark: [1] }, undefined],
    ],
  );
  assert.equal(unstored.size, 1);
});

test("the common reader selects the same exact typed operations without direction or scope", () => {
  assert.deepEqual(
    planRead({ repo: "runxhq/runx", number: 241 }).sync_plan.provider_input,
    { repository: "runxhq/runx", numbers: [241] },
  );
  assert.equal(
    planRead({ repo: "runxhq/runx", kind: "pull_requests", number: 554 })
      .sync_plan.provider_operation,
    "pullrequests.read",
  );
  assert.deepEqual(
    planRead({
      repo: "runxhq/runx",
      kind: "comments",
      number: 241,
      page: 2,
      limit: 100,
    }).sync_plan.provider_input,
    { repository: "runxhq/runx", number: 241, per_page: 100, page: 2 },
  );
});

test("comments need a target, filters stay honest, and page cursors never round", () => {
  assert.equal(
    planRead({ repo: "runxhq/runx", kind: "comments" }).sync_plan.decision,
    "blocked",
  );
  assert.equal(
    planRead({ repo: "runxhq/runx", kind: "pull_requests", labels: ["bug"] })
      .sync_plan.decision,
    "blocked",
  );
  assert.throws(
    () => planRead({ repo: "runxhq/runx", page: 9007199254740992 }),
    /portable integer/,
  );
});
