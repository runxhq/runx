# Thread Story Contract

The thread story is the reviewer-facing projection for source-thread driven
work. It is derived from receipts, scafld state, outbox entries, and provider
observations; it is not the source of truth.

## Current Shape

The shared runtime implementation for bundled tools lives in
`tools/outbox/story.mjs`; handoff-state validation lives in
`@runxhq/contracts`. There is one story implementation,
not a thread alias beside the outbox owner:

- `StoryMilestoneId`
- `ThreadStorySectionId`
- `FeedStoryMilestoneKind`
- `renderThreadStoryMarkdown`
- `renderFeedStoryMarkdown`
- `buildFeedStoryOutboxEntry`
- `buildStoryOutboxIdempotencyMetadata`

Source-command normalization lives one layer earlier at the adapter edge. It
supplies canonical source/thread locators, safe command summaries, target repo
hints, and dedupe keys that story builders may reference. It does not own the
durable reviewer projection, and it must not publish to Slack, GitHub, or
Sentry directly.

The outbox tools use those helpers to produce:

- `story.schema`: `runx.thread-story.control.v1`
- `story.data.thread_locator`
- `story.data.title`
- `story.data.next_action`
- `story.data.milestones`
- `outbox_entry.metadata.schema_version`: `runx.outbox-entry.feed-entry.v1`
- `outbox_entry.metadata.workflow`
- `outbox_entry.metadata.milestone_kind`
- `outbox_entry.metadata.idempotency.key`
- `outbox_entry.metadata.idempotency.content_hash`
- `outbox_entry.metadata.body_markdown`

Provider publication is not owned by these helpers. GitHub, Slack,
support-channel, or other provider mutations run as governed connector
operations with Rust-supervised credential delivery; they must not be
implemented as hidden provider side effects in a local helper module.

The canonical v1 milestone ids are:

- `accepted`
- `hydrated`
- `triaged`
- `reply_drafted`
- `ask_for_info`
- `proposal_ready`
- `escalation_proposed`
- `tracking_item_created`
- `spec_ready`
- `build_started`
- `review_requested`
- `change_request_created`
- `review_fixup`
- `human_gate`
- `outcome_observed`
- `final_outcome`
- `no_action`
- `monitor`

`StoryMilestoneId`, `ThreadStorySectionId`, `FeedStoryMilestoneKind`, and
`outbox_entry.metadata.milestone_kind` use the same canonical v1 milestone
vocabulary. Friendly copy such as "Dev escalation proposed" is derived from
`proposal_kind`; those labels are not accepted as data ids.

Existing issue-to-PR lifecycle gates map into the canonical ids as a hard cut:

- `signal` -> `accepted`
- `decision` -> `triaged`
- `spec` -> `spec_ready`
- `build` -> `build_started`
- `review` -> `review_requested`
- `pull_request` -> `change_request_created`
- `merge_gate` -> `human_gate`
- `outcome` -> `final_outcome`

Runtime input rejects legacy ids. Published legacy entries may refresh into the
canonical entry during migration lookup only, preserving `comment_id`, locator,
and receipt refs, then writing the canonical milestone id to the refreshed
entry.

core-only story/outbox metadata references the existing provider idempotency
contract rather than replacing it. This preserves the existing provider idempotency contract while giving core helpers stable replay metadata. The
idempotency key is built from source id,
provider, source-thread ref, workflow/run id, lane id, canonical milestone id,
target ref, proposal id, and content hash. The content hash is derived from the
normalized public markdown. A same-key replay updates or reuses the existing
publication; different milestones produce distinct entries and do not collide.

## Schema Decision

Keep the story as an internal typed helper plus stable outbox metadata for this
cut. Do not publish a standalone `@runxhq/contracts` schema yet.

That boundary is deliberate:

- current consumers are first-party tools and tests inside OSS runx
- the provider surface is the outbox entry, not a separate registry packet
- provider integrations can already consume the rendered message and metadata
- delaying a public schema avoids freezing fields before live dogfood proves the
  final reviewer shape

Promote the packet to a public contract only when a non-tool consumer needs to
validate or exchange the story independently of the outbox entry. That promotion
must be a hard cut with call sites, docs, and tests updated in one change.

## Required Reviewer Sections

Public story markdown should summarize durable gates:

- source thread and request
- hydrated evidence status when adapter context was needed
- triage decision
- governed scafld task
- build result
- review verdict and finding counts
- PR link, branch, and base when known
- human merge gate
- observed merged or closed outcome

It should not publish low-level run events, full command dumps, raw provider
payloads, local absolute paths, token-shaped values, or consuming-repo policy
such as Slack channel names, Sentry project ids, or owner maps.

The public story carries concise status, evidence bullets, safe excerpts,
source-thread continuity, result refs, publication refs, and the exact next
human action. The private receipt and artifact refs carry raw provider payloads,
full command output, local paths, and detailed evidence for audit. Source-thread
publication is fail-closed: if policy requires a source-thread update and no
source-thread locator is present, helpers reject the projection instead of
falling back to a root channel. This keeps public comments idempotent and
reviewer-safe while preserving artifact refs for reconstruction.

## Non-goals

- This contract does not admit Slack, Sentry, or support-channel messages.
- This contract does not push outbox entries to providers.
- This contract does not decide whether an issue deserves a PR.
- This contract does not merge PRs.
- This contract does not replace receipts, ledgers, or scafld status.
- This contract does not encode consuming-product, runner-provider, or hosted
  deployment policy.
