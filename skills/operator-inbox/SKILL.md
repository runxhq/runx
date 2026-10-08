---
name: operator-inbox
description: Maintain a provider-neutral local action queue from bounded provider observations and explicit human dispositions.
---

# Operator Inbox

Maintain a durable action queue without turning a connector into the owner of
operator state.

The caller fetches bounded, grant-authorized provider pages and passes their
normalized observations to this skill. The skill owns work-item identity,
status, dispositions, replay suppression, reopen rules, and scan coverage. Every
read and write is composed through `data-store`; the skill does not call Slack,
SQLite, Postgres, or another provider directly.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `data-store#append_event`
- `data-store#list_stream_heads`
- `data-store#read_events`

## What this skill does

Use `local://runx/operator-inbox/default` unless the operator selects another
logical source. The queue lives in the workspace's store: the workspace store
(SQLite under `.runx/store.sqlite`) unless the operator confirmed another store
instance, such as Postgres. Runx Connect may still own OAuth, grants and
provider execution; that does not move this queue into the hosted control
plane.

Observations and resumable checkpoints live in `operator_inbox_scans`, partitioned
by query digest. Action snapshots live in `operator_inbox_actions`, with one
stream per stable thread digest. Queue reads use bounded `list_stream_heads`
pages; no command folds or transports the complete queue.

Direct invocation defaults to a bounded `list_actions` read against that local
source. To record a disposition or provider observation, select the exact write
runner; composed graphs can pass the prior action packet without re-reading or
re-fetching provider data.

## When to use this skill

- Build or revisit a local action queue from bounded connector observations.
- Preserve an explicit `resolved`, `dismissed`, `waiting`, or `followed_up`
  decision across repeated provider scans.
- Reopen a completed item when a newer external occurrence arrives.
- Inspect bounded action or scan state without handing queue ownership to the
  provider or hosted control plane.

## When not to use this skill

- Do not fetch provider data, reply, send, or mutate a remote account here.
- Do not infer that an item is complete from message text or provider state.
- Do not use it as an unbounded archive of raw messages or credentials.
- Do not place a private operator's routing policy or identity in this public
  package; pass normalized observations and explicit dispositions as inputs.

## Status rules

Items use `open`, `waiting`, `followed_up`, `resolved`, or `dismissed`.

- Provider observations never infer completion.
- A human disposition records actor, reason, time, the latest external
  occurrence it covers, and optional HTTPS evidence.
- Replaying old search history preserves the human status.
- An external message newer than the covered occurrence reopens the item to
  `open`, including unseen work that arrived before the disposition was saved.
- Scan coverage is explicit: `running`, `complete`, `truncated`, or `failed`.
- Direct mentions are actionable structural evidence. Author and keyword scans
  remain observation-only unless the operator explicitly marks the query
  actionable. The skill does not contain provider-specific keyword heuristics.

The provider-neutral thread locator is the item key. Stored previews are bounded;
credentials, tokens, and full provider response envelopes are forbidden.

## Procedure

1. Read the latest checkpoint for the bounded query digest.
2. Resume its provider cursor when the prior scan was interrupted or truncated.
3. Fetch one bounded provider page through the caller's authorized connector.
4. Record actionable messages against their per-thread streams and append the
   scan page with its next cursor.
5. On a version conflict, reload only the affected scan or action stream and
   retry the idempotent transition.
6. List queue state through bounded action-head pages and use
   `record_disposition` only for an explicit operator correction.

The loop is outside the kernel. Each page or disposition remains one governed,
receipt-backed Runx turn.

## Edge cases and stop conditions

- `needs_input`: missing query identity, observation, disposition,
  actor, reason, or scan coverage.
- `conflict`: the projection version is stale; reload before retrying.
- `provider_unavailable`: the caller cannot prove provider read coverage.
- `too_broad`: a page exceeds the bounded message count or contains unnormalized
  provider data.
- `refused`: a caller asks this skill to send, reply, broaden a grant, store a
  token, or silently claim complete coverage.

## Output schema

Write runners emit `runx.effect.transition.v1`, containing the effect family,
operation, expected projection version, idempotency key, and one normalized
event. Read and list runners return the corresponding bounded `data-store`
event result; they never synthesize provider coverage or completion.

## Worked example

Given a normalized direct mention from a teammate in one provider thread,
`record_action_observation` derives the stable action id from the provider-neutral
thread locator and appends an `open` action snapshot. If the operator later
records `resolved` with a reason, replaying that mention preserves `resolved`;
a newer external reply in the same thread appends a reopened `open` snapshot.

## Inputs

The default `list_actions` runner uses the local source when `data_source_ref`
is omitted. Other runners require an explicit source. Write runners take `expected_version`
and `observed_at`, plus exactly the normalized payload for their operation:
`scan` and `messages`, `message` and `triage`, `disposition`, or an imported
`action`. Action writes derive the stream id from the normalized thread
locator; callers never supply a second, drift-prone identity. Reads take
`action_id` or `query_digest`; list runners take bounded `limit` and optional
cursor or filter fields.
