import assert from "node:assert/strict";
import test from "node:test";
import { activityWindow, digestActivity } from "./github.mjs";

const SINCE = "2026-10-19T21:00:00Z";

/** A GitHub issue record, with the fields `activity` reads and some it leaves behind. */
function issue(number, fields = {}) {
  return {
    number,
    title: `Item ${number}`,
    state: "open",
    user: { login: "ada", type: "User", avatar_url: "https://avatars.example/ada" },
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

const digest = (issues, page) =>
  digestActivity({
    read_input: activityWindow({ repo: "runxhq/runx", since: SINCE, page }).read_input,
    provider_operation: { result: { issues } },
  }).activity;

test("reads one full page of everything updated since, open or closed", () => {
  assert.deepEqual(activityWindow({ repo: "runxhq/runx", since: SINCE }).read_input, {
    repository: "runxhq/runx",
    since: SINCE,
    state: "all",
    per_page: 100,
    page: 1,
  });
  assert.equal(activityWindow({ repo: "runxhq/runx", since: SINCE, page: 3 }).read_input.page, 3);
});

test("keeps what a reader needs, newest change first, and leaves GitHub's whole record behind", () => {
  const activity = digest([
    issue(7, { created_at: "2026-10-18T09:00:00Z", updated_at: "2026-10-19T23:00:00Z", comments: 2 }),
    issue(9, {
      pull_request: { merged_at: "2026-10-20T01:00:00Z" },
      state: "closed",
      closed_at: "2026-10-20T01:00:00Z",
      updated_at: "2026-10-20T01:00:00Z",
      user: { login: "dependabot[bot]", type: "Bot" },
    }),
  ]);
  assert.equal(activity.repository, "runxhq/runx");
  assert.equal(activity.next_page, undefined);
  assert.deepEqual(
    activity.items.map(({ number, kind, state, new: opened, author, bot }) => ({ number, kind, state, opened, author, bot })),
    [
      { number: 9, kind: "pull_request", state: "merged", opened: true, author: "dependabot[bot]", bot: true },
      { number: 7, kind: "issue", state: "open", opened: false, author: "ada", bot: false },
    ],
  );
  assert.deepEqual(activity.items[1].labels, ["bug"]);
  assert.equal(activity.items[1].comments, 2);
  assert.equal(activity.items[1].url, "https://github.com/runxhq/runx/issues/7");
  assert.equal("repository_url" in activity.items[1], false);
});

test("keeps a body's first 600 characters and says how many it left out", () => {
  const [long, short] = digest([issue(1, { body: "x".repeat(650) }), issue(2, { body: null })]).items;
  assert.equal(long.body.length, 600);
  assert.equal(long.body_left_out, 50);
  assert.equal(short.body, "");
  assert.equal(short.body_left_out, 0);
});

test("names the next page only when GitHub filled this one", () => {
  const full = Array.from({ length: 100 }, (_, index) => issue(index + 1));
  assert.equal(digest(full, 2).next_page, 3);
  assert.equal(digest(full.slice(1), 2).next_page, undefined);
  assert.deepEqual(digest([]).items, []);
});
