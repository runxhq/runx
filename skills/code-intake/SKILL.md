---
name: code-intake
description: Turn a noisy inbound request into a bounded intake artifact and an explicit next runx lane.
---

# Code Intake

Convert an inbound thread, support report, or operator request into one
explicit intake decision plus the parent change artifact that downstream
planning or mutation lanes must share.

This skill does not mutate code, open tickets, or publish replies directly. Its
job is to classify the report, summarize it, draft the next helpful response,
and recommend the next governed lane. That next lane must be explicit:
`github-issue-to-pr`, `code-plan`, `reply-only`, or `manual-review`.

In supervisor-style flows, `code-intake` is also the commencement gate. It
decides whether work may start at all, whether the next step should stop at a
review comment first, and whether mutation is justified yet. A recommended lane
is not the same thing as build permission.

Use `github-issue-to-pr` only when the requested change is bounded enough for one
governed remediation lane. Use `code-plan` for larger or multi-step
work. Use `reply-only` when the right answer is guidance rather than mutation.
Use `manual-review` when the report is ambiguous, risky, or missing key context.

Ground category, severity, and routing in the visible request and supplied
product constraints. Put uncertainty in `operator_notes` instead of inventing
confidence. The suggested reply should sound like the project owner and lead
with the decision or next action, not read like a ticket macro.

## Output Contract

`intake_report` must contain:

- `category`: one of `bug`, `feature_request`, `docs`, `billing`, `account`,
  `question`, or `other`
- `severity`: one of `low`, `medium`, `high`, or `critical`
- `summary`: concise summary of the actual request or report
- `suggested_reply`: a user-facing reply draft or operator handoff note
- `recommended_lane`: `github-issue-to-pr`, `code-plan`, `reply-only`, or
  `manual-review`
- `rationale`: why that lane is the right next step
- `needs_human`: boolean
- `operator_notes`: array of caveats, missing context, or escalation notes

`intake_report` may also include supervisor-facing control fields:

- `commence_decision`: `approve`, `hold`, `reject`, or `needs_human`
- `action_decision`: `proceed_to_build`, `proceed_to_plan`,
  `request_review`, or `stop`
- `review_target`: `thread`, `outbox_entry`, or `none`
- `review_comment`: markdown comment body for the supervisor to post before the
  next lane proceeds

When present, these fields mean:

- `commence_decision` gates whether the supervisor may start any downstream
  work at all
- `action_decision=proceed_to_plan` means the supervisor may open a planning
  lane such as `code-plan`, but still may not start repo mutation
- `action_decision=request_review` means the supervisor should post
  `review_comment` to the chosen `review_target` and stop there until a later
  approval or rerun authorizes mutation
- `review_target=outbox_entry` only makes sense when a current
  outbox entry already exists. If no draft change, message surface, or
  other outbox entry exists yet, the supervisor should fall back to the
  source thread and say that clearly in the posted comment
- `action_decision=proceed_to_plan` should usually still result in a public
  supervisor comment so the hold/plan decision is visible outside the raw
  receipt stream
- `recommended_lane=github-issue-to-pr` alone does **not** authorize a build lane

Always emit `change_set` alongside `intake_report`.

Also emit `signal` when a source event is admitted. `signal` must follow
`runx.signal.v1` and carry the source reference, authenticity or trust level,
dedupe fingerprint, evidence references, and source-thread preview. This packet
is the portable world-before-action state that `code-plan`, `github-issue-to-pr`,
hosted queues, and source-thread projections preserve.

Close the Runx turn after intake is complete with terminal `closure` control
metadata: a disposition, stable reason code, and concise summary. Closure is
receipt control state, not part of the code-intake artifact, and must not
pretend the recommended downstream lane has already executed.

When an adapter has provider context beyond the visible thread text, attach it
to `signal.evidence_refs` or a referenced artifact. Source adapters own
provider-specific fetching and redaction before calling this skill; this skill
only reasons over the supplied, reviewer-safe signal and artifacts.

Hydration is a gate, not a best-effort decoration. If supplied signal or
artifact metadata says provider context is still needed, do not select
`action_decision=proceed_to_build`. Use `manual-review` or `request_review` and
explain the missing adapter context in `operator_notes`. If provider context is
unavailable, use the remaining signal only when it is still concrete enough for
a bounded reply, plan, or PR; otherwise stop for human review.

The `change_set` is the parent artifact for any later planning or worker
fanout. It is what keeps multiple repo-scoped lanes aligned to one shared
objective.

`change_set` must contain:

- `change_set_id`
- `thread_locator`
- `summary`
- `category`
- `severity`
- `recommended_lane`
- `commence_decision`
- `action_decision`
- `target_surfaces`: array of objects with:
  - `surface`: repo, product surface, or bounded target name
  - `kind`: one of `repo`, `package`, `docs`, `support`, or `other`
  - `mutating`: boolean
  - `rationale`: why this surface is implicated
- `shared_invariants`: array of constraints that all downstream lanes must
  preserve
- `success_criteria`: array of concrete outcomes that define success for the
  whole change
- `outbox_entry` (optional): current outbox entry for status
  updates, replies, or draft-change refreshes when the caller already knows it

When `recommended_lane=github-issue-to-pr`, also include `thread_change_request` with:

- `task_id`
- `thread_title`
- `thread_body`
- `thread_locator`
- `thread` (optional)
- `outbox_entry` (optional)
- `size`: one of `micro`, `small`, `medium`, or `large`
- `risk`: one of `low`, `medium`, or `high`

When `recommended_lane=code-plan`, also include
`workspace_change_plan_request` with:

- `change_set_id`
- `objective`
- `project_context`
- `thread_locator`
- `thread` (optional)
- `target_surfaces`
- `shared_invariants`
- `success_criteria`

Do not emit both `thread_change_request` and `workspace_change_plan_request` for
the same report.

Prefer conservative routing:

- if the report is bounded and well-understood, use `commence_decision=approve`
  and `action_decision=proceed_to_build`
- if the next step should be planning instead of mutation, use
  `commence_decision=approve` and `action_decision=proceed_to_plan`
- if the likely next lane is clear but mutation or planning should wait for
  maintainer confirmation, use `commence_decision=approve` and
  `action_decision=request_review`
- if the report is ambiguous, under-specified, or risky, use
  `commence_decision=hold` or `needs_human`

## Inputs

- `thread_title`: canonical thread title
- `thread_body`: canonical thread body or request text
- `thread_locator` (optional): canonical locator for the bounded thread,
  such as an issue, chat thread, ticket, or local agent session
- `thread` (optional): provider-backed thread for the current
  thread
- `outbox_entry` (optional): current outbox entry for replies, draft changes,
  or refreshes
- `signal` (optional): provider-neutral `runx.signal.v1` observation gathered
  by the source adapter before decision
- `artifact` (optional): bounded, reviewer-safe source evidence supplied by the
  source adapter; provider fetching and redaction must already be complete
- `product_context` (optional): product-specific constraints or routing hints
- `operator_context` (optional): maintainer or support posture guidance
- `source_event` (optional): admitted Slack, Sentry, GitHub, file, API, or
  other provider event. Consuming repos decide source filters before calling
  this skill.
- `source_policy` (optional): source admission and routing policy. Do not
  hardcode channel names, Sentry projects, or owners in this skill.
- `operational_policy` (optional): `runx.operational_policy.v1` packet used by
  downstream repo-changing lanes for source, target, runner, and source-thread
  admission.
