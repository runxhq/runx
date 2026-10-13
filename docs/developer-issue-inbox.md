# Developer Issue Inbox

The runx developer inbox is a harness context queue, not another chat stream. Slack,
Sentry, GitHub, file, and API adapters may submit source events, but every
accepted event must become one `runx.receipt.v1` packet with an explicit
state, dedupe fingerprint, triage action, and source-thread locator.

## Source Command Normalization

Normalize source commands at the adapter edge before calling `issue-intake`.
The adapter-local normalizer parses GitHub, Slack, Sentry, file, API, and
manual source references into a provider-neutral command shape:

- canonical `source_locator` and, when available, `thread_locator`
- target repo hint from concrete GitHub URLs
- stable source dedupe key
- `source_event` input for `issue-intake`
- operational-policy admission request fields
- chat-safe command response text for accepted, blocked, unsupported, or failed
  command paths

This helper does not fetch Slack threads, Sentry packets, GitHub bodies, or
support tickets. Hydration, redaction, channel/project filters, owner routing,
credentials, and provider mutation stay in the consuming adapter. If the source
needs provider context and no adapter supplied it, the command should stop at a
visible blocked response rather than pretending mutation was dispatched.

## Admission Policy

Adapters must evaluate source policy before invoking `issue-intake`. A message
containing a word such as `BUG` is not enough to trigger mutation.

The reusable policy packet is `runx.operational_policy.v1`
(`operational-policy.schema.json`). Repo adapters can keep product-specific
values such as Slack channel ids, Sentry projects, and owner names in their own
config, but the shape is shared: allowed sources, actions, target repos,
runners, owner routes, dedupe, source-thread publishing, outcomes, and
automation permissions are all explicit.

Use `validateOperationalPolicySemantics` before enabling mutation. Schema
validation proves the packet shape; semantic validation proves referenced
runners and owner routes exist, target repos are covered, source-thread
publishing fails closed, and available runners can perform the declared target
actions.

Use `projectOperationalPolicyReadback` for operator and admin displays. It exposes
source ids, locator counts, runner state, target repos, owner coverage, outcome
settings, permissions, and validation findings without echoing raw provider
locators.

Use the CLI gate before wiring a policy into a live runner:

```bash
runx policy fixtures/operational-policy/provider-like.json --json
```

`runx policy` returns a redacted readback shape for admin
surfaces. It is safe to show to developers because it reports locator counts
instead of raw Slack channels, Sentry projects, or provider thread locators.

Required policy fields:

- `sources[]`: source id, provider (`slack`, `sentry`, `github`, `file`,
  `api`, or `other`), locator allowlist, allowed actions, source-thread
  policy, optional confidence threshold, and provider-specific filters such as
  Sentry production/unresolved/regressed gates
- `runners[]`: runner id, kind (`local`, `github-actions`, or another
  product-defined identifier), availability state, allowed actions, target repos, and whether
  scafld is required
- `owner_routes[]`: owner sets and the target repos they cover
- `targets[]`: repo slug, allowed runners, allowed actions, default owner
  route, scafld requirement, and optional base branch
- `dedupe`: strategy, key fields, and duplicate behavior
- `outcomes`: provider observation, verification requirement, source issue
  close mode, and final source-thread publishing policy
- `permissions`: mutation permission, required human merge gate, and explicit
  `auto_merge=false`

Use `admitOperationalPolicyRequest` at mutation-adjacent boundaries. It
evaluates a concrete `source_id`, `target_repo`, `action`, `runner_id`, and
source-thread locator against one validated policy packet. Unknown targets,
unknown or unavailable runners, disallowed actions, and missing source-thread
routes deny before PR packaging or provider dispatch.

Terminal post-merge observation must seal a receipt with closure and
verification proof before publishing the final source-thread update or closing
the source issue.

Non-trigger cases:

- general support chatter without a reproducible issue
- Slack keywords outside an allowlisted source
- Sentry alerts below configured frequency or severity thresholds
- reports missing a stable source locator or dedupe fingerprint
- ambiguous requests that need a human target decision
- duplicate events already attached to an open harness
- source events whose configured source thread cannot be recovered

## Queue States

Developer views should group by `harness_id` and show the next useful gate:

- needs triage: `accepted`, `hydrated`, `triaged`
- needs evidence: `blocked`
- ready to plan: `spec_ready`
- ready to build: `build_started`
- review running or failed: `review_requested`, `review_fixup`
- PR ready: `change_request_created`
- waiting for human merge: `human_gate`
- done: `outcome_observed`, `final_outcome`, `no_action`, `monitor`

List views should stay compact: id, state, status summary, source, dedupe
fingerprint, triage action, issue/PR refs, duplicate counts, latest transition,
and timestamps. Full receipts and ledger artifacts remain the evidence layer.

Inbox adapters should treat core story output as provider-neutral markdown/text.
They may translate it into GitHub issue comments, Slack blocks, or support notes,
but the provider-specific ids, buttons, channels, and raw payloads remain
adapter-owned. Public rows should show source-thread continuity, result refs,
publication refs, and the next human action; private receipts and artifact refs
hold raw provider context for audit.

## Routing

runx core may store suggested owner and target repository metadata, but it must
not hardcode people, Slack channels, Sentry projects, or customer repository
names. Consuming repos own those policies.

Core can still enforce the policy shape. A PR-producing lane should require:

- a source locator and source-thread locator
- a dedupe fingerprint
- an allowed target repo
- an available runner
- a target repo that is scafld-ready when mutation is requested
- an owner route for reviewer assignment
- explicit human merge gate policy

The PR packaging boundary stores a redacted policy admission summary in the
draft packet and outbox metadata: policy id, source id, target repo, runner id,
owner route id, owner count, dedupe strategy, outcome close mode, source-thread
requirement, mutation permission, and human merge gate. It does not echo raw
Slack, Sentry, GitHub, or local path locators.

`issue-intake` chooses the next lane:

- `reply-only`: answer or support guidance, no mutation
- `manual-review`: human decision needed before planning or mutation
- `work-plan`: bigger change, planning first
- `issue-to-pr`: bounded fix, governed PR lane

`issue-to-pr` must preserve the same `harness_context` packet through PR packaging
and source-thread story updates. It must stop at the human merge gate.

Pull-request outbox entries must include `metadata.dedupe` with the selected
strategy, key, and whether the PR packet was created or reused. Retrying the
same source thread should refresh the existing branch/comment/PR path, not open
a parallel review path.
