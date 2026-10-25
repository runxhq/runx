---
name: slack
description: Read bounded Slack search, thread, link and file evidence through any compatible Slack binding, ready for an operator's triage and for message-send to reply.
---

# Slack

Use Slack as a governed provider boundary without turning Slack—or Runx
Cloud—into the owner of the operator's workflow. This skill owns the reusable
Slack reading mechanics that an agent should not have to reconstruct: bounded
message search, bounded thread hydration, permalink resolution and image reads.
Delivering a post or a reply is `message-send`'s job.

The operator-selected provider binding has one narrow role in this flow: retain
credential custody and execute a fixed Slack operation. It may be local,
self-hosted, third-party, or Runx-hosted. The skill, its procedure, approval
point, retry identity, and completion rule remain in Runx OSS. Any queue,
team-specific routing rule, or durable action state belongs in a higher-level
operator skill.

## Runners

`search` reads one bounded Slack search page. Supply a `query` object with at
least one of `author_external_id`, `mentions_connected_subject`, or `keywords`.
The exact fields are:

- `author_external_id`: one Slack member id string, such as `U123ABC`
- `mentions_connected_subject`: a boolean selecting messages that mention the
  member bound to the selected provider account
- `keywords`: one keyword-mode search phrase string, not an array and not raw
  Slack search syntax
- `after` and `before`: ISO-8601 timestamp strings, with `after` earlier than
  `before`
- `channel_types`: a non-empty array drawn from `public_channel`,
  `private_channel`, `mpim`, and `im`
- `limit`: an integer from 1 through 20
- `cursor`: the opaque `next_cursor` from the preceding page

The provider rejects embedded Slack modifiers in `keywords`, enforces the
maximum page, and returns normalized locators and bounded previews rather than
a raw Slack response. Continue only from `next_cursor`; one page is never proof
that a workspace scan is complete.

`read_thread` reads one bounded thread page from an exact
`slack://workspace/channel/timestamp` locator. The limit is capped at 15 because
Slack applies a low limit and, for some non-Marketplace installations, a very
low request cadence to `conversations.replies`. Hydrate only threads that are
actually needed, preserve `next_cursor`, and do not fan out speculative reads.
Thread messages include bounded attachment metadata and opaque
`slack-file://workspace/file` locators; private Slack download URLs never cross
the provider boundary.

`read_link` accepts one exact Slack archive permalink and reads the same bounded
thread page. Use it when an operator pastes a normal
`https://workspace.slack.com/archives/channel/message` link and therefore does
not have the workspace id required by a canonical locator. The provider binding
validates the permalink, binds it to the authenticated workspace identity, and
returns the canonical `slack://workspace/channel/timestamp` locator. A reply
permalink's `thread_ts` identifies the root; Slack's redundant `cid` must match
the path channel. This runner requires only `thread.read`, never a search grant.

`read_file` reads one image attachment from an exact file locator returned by
`read_thread`. It requires `file.read`, verifies the file through Slack,
restricts downloads to Slack's private file origin, accepts images only, and
caps decoded bytes at 5 MiB. The result contains the MIME type, content digest,
and base64 bytes so the caller can inspect the image without receiving the
Slack credential or private URL.

Replies go through `message-send`: `plan_reply` binds a proposed reply to the
exact canonical thread and its digest, and `deliver_reply` posts it once after
approval and reads it back; `reply` delivers a reply bound to a source packet.
Use `message-send` `send` for a proactive top-level channel post. A
product-owned skill may add team-specific triage and compose these skills, but
must not copy their provider transport, approval, idempotency, readback, or
queue logic.

## Authority and privacy

Reads resolve a compatible Slack binding for `messages.search`, `thread.read`,
or `file.read` and do not ask for human approval. Runx supplies the idempotency key once, and
the selected transport verifies the registered operation and its read/mutate
class against the bound provider authority.

The skill never receives a Slack token, constructs HTTP, or stores a raw
provider envelope. Search and thread outputs contain bounded previews because
the operator needs context. Reply receipts contain only stable locators,
timestamps, and a content digest; they do not echo the delivered message.

## Stop conditions

- Refuse a missing, ambiguous, revoked, wrong-provider, or insufficient-scope
  Slack binding. Never fall back to a token, webhook, browser, or
  provider-specific script.
- Stop when a search has no structural selector, uses an unsupported modifier,
  or asks for more than one bounded page per turn.
- Stop when an attachment is not an image, exceeds 5 MiB, belongs to another
  workspace, or resolves outside Slack's governed private-file origin.
- Stop on malformed permalinks, unsupported permalink query fields,
  cross-workspace locators, or thread/message channel mismatch.
- Do not infer that a request is resolved from Slack prose. Record resolution,
  waiting, follow-up, or dismissal explicitly in the owning operator workflow.
- Do not claim complete scan coverage while a cursor remains.

## Worked flow

An operator can search for direct mentions or start from a pasted Slack
permalink. They hydrate only the actionable thread, preserve its canonical
locator, then pass that normalized observation to a team-specific triage skill. If a reply is needed, `message-send` `plan_reply` binds the proposed text
to the exact canonical thread. A changed draft fails before approval. An
approved unchanged reply is posted once, then read back from the exact returned
message locator before Runx seals the effect.

## Operator conversation handoff

`assistant_scan` returns one bounded search page and its continuation cursor;
`assistant_observe` binds each message to its source identity. Hydration checks
that identity against the canonical open action before reading the thread.
The resulting `runx.source_packet.v1` supplies `workflow_inputs.thread` with
the exact source and thread bindings, message evidence and coverage flag, plus
`source_complete` for `conversation-review#operate`. Missing source messages,
truncated content or another page keep coverage incomplete. The consumer holds
that work rather than treating a partial thread as resolved.
