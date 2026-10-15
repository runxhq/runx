---
name: twitter
description: >-
  Govern X (Twitter) account work through three lanes: evidence reads, typed
  act plans for posting and account hygiene, and gated execution with per-act
  provider evidence. Nothing reaches a live timeline or mutates an account
  without an explicit approval recorded in the receipt.
---

# Twitter

One account, three lanes: read evidence, plan typed acts, execute an approved
plan.

This is the public branded X (Twitter) catalog skill for the `message-send` action
family. Its core invariant: the agent may read, audit, draft, and plan freely,
but every act that publishes to a public timeline or mutates the account stops
at a human approval gate, and the sealed receipt proves which acts ran, against
which plan digest, with which provider evidence.

Write for the operator who owns the account, the reviewer who approves a live
act, and downstream skills that consume the resulting packets. Keep each plan
to the smallest evidence-backed act set that satisfies the objective. Existing
post and user ids must come from supplied evidence, never memory or guesswork;
name missing evidence as a blocker.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `data-store#append_event`
- `data-store#read_events`

## What this skill does

Five runners:

- `read`: collect account evidence from the live API through the `twitter`
  connector. Queries: `snapshot`, `posts`, `mentions`, `search`, `following`,
  `followers`. A query without `params.user_id` reads the authenticated
  account; with it, another account. The connector reads whole pages until it
  has at least `max_items`, keeps every item of them, and reports in
  `next_cursor` where a further read starts (pass it back as
  `params.pagination_token`). Emits `twitter.evidence.v1`.
  Read-only; no gate.
- `read-archive`: inspect one contained X archive export (`tweets.js`,
  `following.js`, or `follower.js`) through the runtime's digest-bound artifact
  page seam. Twitter code receives bounded, record-complete pages and never a
  path or whole-file escape hatch. The runtime snapshots up to 512 MiB and this
  skill requests 512 KiB pages; the page size is not a total archive limit.
  Emits the same `twitter.evidence.v1` packet; no gate.
- `plan`: turn one bounded objective plus evidence into `twitter.plan.v1`, an
  explicit list of typed acts with rationale, using agent judgment, then bind
  the exact plan through native `data.digest`. This is the curated lane, for
  dozens of acts that each deserve a reason. A plan is a draft; it delivers
  nothing. The result exposes `twitter_plan.data` and the matching
  `digest_result.data.digest` required by `execute`; callers never calculate
  the digest themselves.
- `select`: the bulk lane. Apply a deterministic predicate to an archive export
  and emit a compact plan, no agent judgment. Use it when the criterion is
  mechanical and the match set runs to thousands, where a per-item rationale
  would be wrong and would exceed the runtime output limit. Two targets:
  `posts` pages `tweets.js` and emits `delete_post` acts (predicate: author,
  date range, engagement threshold), with identical results across page sizes;
  `users` reads a bounded `following.js` bundle and emits
  `unfollow` acts (predicate: `non_mutual: true` for accounts you follow that
  do not follow back, needing `follower.js` too, or an explicit `user_ids`
  list). Emits `twitter.selection.v1` carrying a digest-bound `twitter_plan`.
- `execute`: run the next batch of an approved plan as one `twitter` connector
  operation behind its own approval, sealing per-act outcomes into
  `twitter.execution.v1` and appending one compact progress fact to a durable
  execution ledger. A batch is one published act (a post, reply, quote, or one
  thread segment), or the contiguous run of up to 50 acts of one other kind
  from the cursor. The approval binds that exact call. The kernel records each
  act's outcome as it lands and stops sending at a rate limit, so a monotonic
  cursor, not an accumulated id list, is enough to resume safely.

### Resume and the execution ledger

`execute` is stateless per turn; the state lives in a `data-store` stream keyed
by the immutable plan digest. The latest `twitter.execution.progress.v1` event
contains only the X account id the batch ran as, `next_act_index`,
`total_act_count`, an optional in-progress thread segment cursor, and a bounded
summary of the last batch. Its size is constant whether the plan has completed
two acts or two thousand.

Each batch first reads the bound account. It runs only when that account is
the plan's principal (`account:@handle`, any letter case) and, once the ledger
has pinned an account id, the same account every earlier batch ran as;
otherwise the run fails before anything is sent. The approval shows the
verified `@handle` and account id.

The driver reads only the latest ledger event (`read_events`, `limit: 1`) and
its stream version. It calls `execute` with that `expected_version` and the
canonical key `twitter:<plan_digest>:v<expected_version+1>`. The runner performs
the same tail read itself and refuses stale or misbound inputs before approval
or provider work. After a committed batch, the driver repeats with the new
version until `next_act_index == total_act_count`. It never scans history or
folds completed ids. An invalid plan, digest mismatch, stale cursor, or already
complete plan produces a sealed refusal/no-op receipt but does not append a
fake state transition.

Threads use one extra bounded cursor: each segment is its own batch, and
progress records the next segment plus the confirmed reply id until the last
one posts, so a resumed thread never reposts a confirmed segment. The operator binds
`data_source_ref` to durable local SQLite or another declared `data-store`
adapter; the skill never opens the database directly.

Each runner emits exactly one packet for its lane: typed evidence with
provenance and a content digest, a curated or bulk act plan bound by digest, or
provider outcomes bound to the executed plan digest and appended to the ledger.

The act vocabulary, with its consequence class:

| kind | consequence | params |
| --- | --- | --- |
| `post` | public_send | `text` |
| `reply` | public_send | `text`, `in_reply_to` |
| `quote` | public_send | `text`, `quote_of` |
| `thread` | public_send | `texts` (ordered, max 25) |
| `repost` | public_send | `post_id` |
| `delete_post` | live_mutation | `post_id` |
| `unfollow` | live_mutation | `target_user_id` |
| `mute` | live_mutation | `target_user_id` |
| `block` | live_mutation | `target_user_id` |
| `follow` | live_mutation | `target_user_id` |
| `like` | live_mutation | `post_id` |

Every kind is consequential, so every plan sets
`gates.human_approval_required: true`. `follow`, `like`, and `repost` are
engagement acts and share a hard cap of 10 per plan. Direct messages are
outside this skill.

A published act is confirmed by reading the post back. The other kinds are
acknowledged: X answers each with the resulting state (`following: false`,
`deleted: true`), which is the evidence, and has no read of one follow or like
cheap enough to confirm a batch. They set a state, so resending one changes
nothing. An answer that does not state the resulting state stops the cursor at
that act, and a later batch sends it again.

Post and reply `text`, and each thread segment, is a non-empty string of at
most 4000 characters, published verbatim; ids are X id strings. A plan that
breaks either is refused before approval.

## When to use this skill

- Promote a release, publish a thread, or reply to a mention on behalf of a
  named principal, with content bound verbatim in the plan.
- Audit an account's own post history or following list and prune it: bulk
  delete old posts, unfollow low-value accounts, against operator-stated
  criteria.
- Gather timeline, mention, or search evidence for downstream research,
  triage, or lead skills that consume `twitter.evidence.v1`.
- Route a `message-send` plan onto X as its provider delivery lane.

## When not to use this skill

- To send direct messages or manage DM conversations.
- To farm engagement: follow-churn, mass liking, coordinated reposting, or any
  volume pattern designed to game visibility. The engagement cap is not a
  budget to fill.
- To mention, reply to, or dogpile users who have not engaged with the
  principal, at volume.
- To operate an account the operator does not control, or to automate consumer
  account creation.
- To bypass the approval gate, the act caps, or the operator's spending limit
  on the pay-per-use API.

## Procedure

For the `plan` runner, build `twitter_plan` this way:

1. Hold one bounded objective. If the ask bundles unrelated jobs (a promo
   thread plus a follow purge), return `needs_input` with the split.
2. Ground every reference. Ids for `delete_post`, `unfollow`, `reply`,
   `quote`, `like`, `mute`, and `block` must come from `evidence_json` or an
   explicit operator-supplied id. If the evidence is missing or stale, name it
   as a blocker; never invent or approximate an id.
3. Write public content into the act params verbatim: the exact `text` or
   `texts` to publish, shaped by `brand_context` when supplied. Do not plan a
   summary of what will be written; approval binds the exact words.
4. Give every act a stable `act_id` (`act-001` onward), its `kind`, `params`,
   its `consequence` from the table, and a one-line `rationale` tied to the
   objective or the operator's stated criteria.
5. Apply `operator_policy` narrowly. A prune criterion like "no engagement and
   older than 2023" selects only posts the evidence shows meet it; borderline
   items go to `open_questions`, not into the act list.
6. Keep the plan small: at most 50 acts, at most 10 engagement acts, threads
   at most 25 segments. A larger job becomes staged plans.
7. Set `decision`: `ready` when acts are grounded and complete; `needs_input`
   for missing objective, principal, evidence, or content; `reject` for asks
   outside the vocabulary or inside the abuse boundaries.
8. Never place credentials, bearer tokens, raw API dumps, or third-party
   personal data beyond public ids and handles in the plan.

## Edge cases and stop conditions

- **Mixed objective:** return `needs_input` with the runner or plan split.
- **Id not in evidence:** blocker; the plan stays `needs_input`.
- **Prune criteria ambiguous:** select the unambiguous items, list the rest in
  `open_questions`.
- **Rate limit or provider refusal during read:** the read fails with the
  provider's reason; a read is whole or it fails.
- **Oversized user-selection archive:** the current two-file `users` selector
  refuses when either bounded text file exceeds the native read limit; it never
  treats truncation as an empty following/follower set. `read-archive` itself
  remains paged. Do not claim an oversized users bulk plan was evaluated.
- **Rate limit during execute:** nothing more is sent; the acts before it are
  settled and the packet reports `next_act_index`, `remaining_count`, and the
  reset time. After the reset, read the latest ledger version and invoke the
  next canonical batch; do not reconstruct ids.
- **One act refused:** a refused act is recorded as `failed` with the
  provider's reason and the cursor moves past it. Unfollowing a deactivated
  account is already the state the act asked for, so it counts as done.
- **Plan digest mismatch on execute:** the apply step refuses and executes
  nothing. A refusal is receipt evidence, not a fake ledger transition.
- **Stale stream version or batch key:** refusal before the approval/provider
  boundary. Re-read the one-event ledger tail and derive the canonical key.
- **Partially posted thread:** resume from the ledger's `active_thread` cursor;
  never restart the thread from its first confirmed segment.
- **Approval missing or denied:** the execute graph stops at the gate.
- **No bound account:** admission refuses before any provider call; bind one
  with `runx connector use twitter --credential user`.
- **Fully autonomous live posting requested:** `reject`; the gate is the
  contract.

## Output schema

- `twitter.evidence.v1`: `decision` (`ok` or `needs_input`), `source`
  (`live` or `archive`), `query`, `account`, `items[]` (typed post or user
  records with metrics), `item_count`, `truncated`, `next_cursor` (where the
  next live read starts, or null), `provenance`
  (`retrieved_via`, the readback reference of the provider answers or the
  archive artifact, and `content_digest`), `blockers[]`.
- `twitter.plan.v1` (returned as `twitter_plan.data` together with
  `digest_result.data.digest`): `decision` (`ready`, `needs_input`, `reject`),
  `objective`, `principal`, `acts[]` (`act_id`, `kind`, `params`,
  `consequence`, `rationale`), `gates`
  (`human_approval_required`, `approval_ref`), `evidence_refs[]`,
  optional `context_bindings[]` (`packet_digest`, `applied_rules[]`) preserving
  the exact brand or taste context applied by the planner, `open_questions[]`,
  `blockers[]`, `success_checkpoint`.
- `twitter.selection.v1`: `decision`, `objective`, `principal`, `predicate`,
  `matched`, `scanned`, `truncated`, `twitter_plan` (a `twitter.plan.v1`),
  `plan_digest`, `blockers[]`. The `twitter_plan` is what a driver hands to
  `execute`, and `plan_digest` is bound so the approved and executed bytes are
  provably identical.
- `twitter.execution.v1`: `decision` (`executed`, `partial`, `stopped`,
  `refused`, `noop`), `plan_digest`, `principal`, `results[]` (`act_id`, `kind`,
  `consequence`, `status`, `provider_ref`, `detail`) for the current bounded
  batch, `next_act_index`, `total_act_count`, `remaining_count`, optional
  `active_thread`, `rate`, `blockers[]`, and `success_checkpoint`. A ready batch
  also carries the compact `twitter.execution.progress.v1` ledger delta.

## Worked example

Input: objective "delete my zero-engagement posts from before 2024",
principal `account:@example`, evidence from
`read-archive(query: posts, archive_file: data/tweets.js)` showing three matching
posts, operator_policy "keep anything with replies".

Output: `decision: ready`; three `delete_post` acts, each carrying the post id
from the evidence and a rationale quoting its age and zero metrics;
`gates.human_approval_required: true`; one borderline post with two replies
listed in `open_questions`. Execution then stops at the approval gate, and the
sealed receipt binds the approved digest to the three deletions the provider
confirmed.

## Inputs

- `read`: `query` (required), `params` (`user_id`, `q`, `pagination_token`),
  `max_items`.
- `read-archive`: `query` and relative `archive_file` (required),
  `archive_base` (`workspace` default, or `skill` for packaged fixtures), and
  `max_items`.
- `plan`: `objective` (required), `principal` (required), `evidence_json`,
  `operator_policy`, `brand_context`, `operator_context`.
- `select`: `objective` (required), `principal` (required), `target` (`posts`
  default, or `users`), `archive_file` (posts), `following_file` and
  `followers_file` (users), `predicate` (required: posts take `rt_of`,
  `is_retweet`, `text_prefix`, `text_contains`, `max_likes`, `max_reposts`,
  `before_year`, `after_year`; users take `non_mutual` or `user_ids`),
  `archive_base`, `max_acts`.
- `execute`: `plan_json` and `plan_digest` (required), `data_source_ref`
  (required), `expected_version` (required), `idempotency_key` (required),
  `max_acts` (acts per batch, hard-capped at 50). `idempotency_key` must be exactly
  `twitter:<plan_digest>:v<expected_version+1>`; the runner independently
  verifies both values against the durable one-event tail before execution.

## Credentials and cost

The `twitter` connector declares two credentials, and the bound account's
credential decides how a request authenticates. Credentials never appear as
inputs or receipt material.

- `user`: one JSON object holding `consumer_key`, `consumer_secret`,
  `access_token`, `access_secret` (OAuth 1.0a user context). Required for
  every mutation and for the authenticated account's own reads. Bind it with
  `runx connector use twitter --credential user`, the JSON on stdin.
- `app`: the app-context bearer token, enough for `search` and reads that name
  another account by `user_id`. Bind it with `runx connector use twitter
  --credential app`, the token on stdin.

The X API bills per request on current plans and a post containing a link
costs a large multiple of a plain one, so prefer archive exports for bulk
history, set a spending cap in the developer portal, and let `max_items` and
the act caps bound each run.

## Agent task contract

### `twitter-plan`

Follow the `plan` procedure and act vocabulary above. Return one
`twitter_plan` containing `decision`, `objective`, `principal`, typed `acts`,
approval gates, evidence refs, open questions, blockers, and a truthful success
checkpoint. Every referenced post or user id must be grounded in supplied
evidence, and every public word must appear verbatim in the act params. A plan
never executes, publishes, deletes, follows, or otherwise mutates the account.
