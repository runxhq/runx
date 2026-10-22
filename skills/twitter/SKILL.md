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

## What this skill does

Seven runners:

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

### Durable operation memory and continuation

The skill remembers an operation in its bound `store`, local to the workspace
by default. It saves the exact plan once under its native digest, separately
from the compact execution progress stream. The saved plan preserves the
objective, account principal, exact content and targets, rationale, evidence
references and any applied context bindings. Progress preserves the pinned
account id, next act, partial thread cursor, cumulative processed, succeeded and failed act counts, a bounded last failure
evidence reference, the last batch summary and structured stop advice. No host opens the store database, reconstructs
completed ids or calculates plan hashes.

On the first `execute`, supply `plan_json` and its native `plan_digest` from
`plan` or `select`. A valid plan is saved before the effect approval; saving it
is not approval. On later calls, omit `plan_json` and supply only the digest.
The skill reads its own progress, selects the next batch and derives the
expected stream version and canonical retry key. The logical store defaults to
`local://twitter`; pass `data_source_ref` when the operation uses another
binding, and keep using that same workspace and store.

- `status` reads one plan and its progress without contacting X. It returns the
  objective, principal, current cursor, partial thread, last batch, next batch
  summary, cumulative outcome counts, failure evidence, structured `stop`,
  `retry_at`, and compact `next_input` for `execute`. It never returns
  an approval or claims a proposed batch has run. `needs_plan` means this store
  has no saved plan: provide the original exact plan once, rather than
  inventing it or resetting a progress stream.
- `recent` lists a bounded page of prior execution summaries and plan digests.
  It does not scan history or return every post. A plan saved at an approval
  pause has no completed batch yet; find that paused invocation through native
  run history and resume its run id.

Each execution reads the bound account. It runs only when that account matches
the plan's principal (`account:@handle`, case insensitive) and, once progress
has pinned an account id, that same account. The exact approval identifies the
verified handle, account id, target and payload. Another available account is
never a fallback.

A native invocation and a multi-batch plan have different identities. If the
invocation pauses, continue **that run id** through `runx resume`; do not start
another run to escape an approval. If a call is interrupted or its outcome is
unknown, read native history and reconcile that same invocation first. Runx
owns in-flight custody and whether an effect may be retried. Only after a batch
is sealed and its result understood should another `execute` start the next
batch. A saved plan, status, receipt or supplied approval reference is context,
never new authority. Relay only actual operator decisions bound to the exact
native request.

Threads advance one segment per batch. The saved cursor contains the next
segment and confirmed reply id, so continuation does not repost earlier
segments. The cursor counts processed acts. Only provider-confirmed successful acts
increment `counts.succeeded`; settled refusals increment `counts.failed`. A
partially published thread is not a completed act. Failures remain visible
across later batches through cumulative counts and a bounded `last_failure`
containing the act, reason and native evidence reference. At the end, execution
returns `completed_with_failures` and status returns `complete_with_failures`
when any terminal act failed. Detailed outcomes remain in native evidence. A refused plan, digest mismatch or already-complete plan does not
append a fake progress event.

`stop.action` distinguishes `wait`, `review` and `reconcile`. A known rate
reset gives `waiting` status and a conditional `next_input`: the host must
wait until `retry_at` before using it. Unknown retry timing or account,
authentication and billing problems require review and expose no automatic
next input. An unsettled or ambiguous outcome asks for native reconciliation;
never start a replacement publish invocation to escape it. After resolving a
review stop, an operator may explicitly invoke execute again under a fresh
exact native approval. `retry_at` is the provider's reported reset, not proof the account will be
ready then. Wait until that time before another attempt, or stop for the
operator when retry timing is unknown. Inspect native failure evidence for
calls that fail before a settled batch result. Do not loop on permission,
billing, account restrictions or unknown acknowledgement. The host owns
scheduling and may choose a stricter cadence or spending cap. A CLI host can
return the wait to its operator; Fieldwork can persist a wakeup on its Work.
The skill contains no sleeping process, scheduler or personal memory service.

### Direct CLI use

Use the installed CLI's inspection for exact input contracts. Select the
workspace and connector once, then use the same workspace for continuation:

```sh
runx connector use twitter --credential user
runx connector use store
runx skill ./skills/twitter read -i query=snapshot --json
runx skill ./skills/twitter recent --json
runx skill ./skills/twitter status -i plan_digest=<native-plan-digest> --json
runx skill ./skills/twitter execute -i plan_digest=<native-plan-digest> --json
```

For the first execution, `--inputs <file-or-stdin>` carries the original plan
and native digest; subsequent executions use the compact `next_input` from
status. Local paths above illustrate a checkout; installed registry refs and
other compatible hosts use the same runner contracts. Paths, run ids and
artifact references come from runx, never guessed filesystem layouts.

The operating sequence is evidence → grounded plan or deterministic selection
→ exact native approval → bounded execution → status or native recovery.
Codex supplies judgment and relays the person's decisions. It needs no X HTTP,
SQL, batching, canonical hashing or private operator scripts. Fieldwork owns
ongoing goals, brand/taste context, longer-lived memory and future wakeups;
the same skill and native evidence work from either host.

### Connector implementations

The skill targets typed X operations, not an HTTP library or browser driver.
An API implementation and an X-specific browser implementation can implement
the same operations if they preserve account binding, exact payload semantics,
bounded outcomes, idempotency/custody requirements and truthful finality. A
generic browser connector alone does not implement `post.create` or
`users.follow`: its X-specific implementation must translate those operations
and verify their results. Bind an implementation explicitly; do not silently
switch transport or account after a failure. Missing operations are readiness
failures, not permission to fall back to ad hoc browser or HTTP code. The
current packaged connector uses the X API; this skill change does not supply
or claim a working browser implementation.

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
  reset time. After the reset, inspect `status` and invoke its compact next input;
  the skill reads and verifies the latest stream version itself.
- **One act refused:** a refused act is recorded as `failed` with the
  provider's reason and the cursor moves past it. Unfollowing a deactivated
  account is already the state the act asked for, so it counts as done.
- **Plan digest mismatch on execute:** the apply step refuses and executes
  nothing. A refusal is receipt evidence, not a fake ledger transition.
- **Concurrent or interrupted work:** reconcile the native run first. The skill
  derives its stream version and batch key; a host must not invent either.
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
- `twitter.execution.v1`: `decision` (`executed`, `completed_with_failures`, `partial`, `stopped`,
  `refused`, `noop`), `plan_digest`, `principal`, `results[]` (`act_id`, `kind`,
  `consequence`, `status`, `provider_ref`, `detail`) for the current bounded
  batch, `next_act_index`, `total_act_count`, `remaining_count`, optional
  `active_thread`, cumulative `counts`, `last_failure`, structured `stop`, `rate`, `blockers[]`, and `success_checkpoint`. A ready batch
  also carries the compact `twitter.execution.progress.v1` ledger delta, with
  the reported rate/reset information needed by the next host.

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
- `execute`: `plan_digest` (required), optional `plan_json` on the first call,
  `data_source_ref` (default `local://twitter`) and `max_acts` (1–50). Native
  digest verification precedes saving the plan or provider effects. Version,
  retry identity and next batch are derived inside the skill.
- `status`: `plan_digest` (required), optional `data_source_ref` and `max_acts`.
- `recent`: optional `data_source_ref`, `limit` (1–100, default 5) and page
  `cursor`. Only execution summaries are listed.

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
