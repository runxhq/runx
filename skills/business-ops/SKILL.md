---
name: business-ops
description: "Route one business signal through a replayable governed ops graph: classify, docs, release, work, outreach, spend, and proof, with consequential actions stopping at the right gate."
---

# Business Ops

Turn one business signal into a replayable operations graph.

`business-ops` routes one signal into one selected bounded lane without giving
the agent ambient authority. The default `route` runner returns that typed
lane immediately; it does not require event-store configuration or run
unrelated docs, release, work, outreach, spend, and audit branches. Select
`route_and_append` only when a chain needs a durable event plus projection
readback, and select `main` only when the full planning fan-out is the
requested artifact.

This is not a provider integration and not an operator dashboard. It is the
small core shape that teams copy when they want one objective to fan out into a
chain of skills, then replay that chain with receipts.

The explicit `route_and_append` runner takes the same selected lane, appends
its packet through `data-store`, reads back the projection, and returns one
`route_persistence` result that distinguishes a new commit from an idempotent
replay while preserving the selected `lane_packet`. The same graph
can use local JSON, SQLite, Postgres, D1, Redis, or a product adapter by changing
the `data_source_ref` binding. Its aggregate id, expected version, and
idempotency key stay explicit because guessing them would corrupt durable
workflow state.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `data-store#append_event`
- `data-store#read_projection`

## What this skill does

- Routes one business signal into one selected typed lane by default.
- Can explicitly fan the signal through representative lanes: docs, release,
  issue/PR, outreach planning, spend quoting, and proof audit.
- Produces structured lane packets with authority, gate, handoff, evidence, and
  readback fields.
- Demonstrates the runx split between proposal work and consequential action:
  drafts and plans can be produced, but sends, spend, merges, publishes, and
  deploys require a separate approval and execution lane.
- Gives downstream agents a clear handoff target instead of vague prose.
- Optionally persists the classified route for replay through `data-store`.

## What this skill deliberately does not do

- It does not call private providers, mutate a repo, post to GitHub, send email,
  schedule campaigns, move money, publish releases, or deploy services.
- It does not duplicate `ops-desk`, product operator skills, `send-as`,
  vendor-specific provider skills, `release`, `issue-to-pr`, `spend`, or
  receipt-audit skills.
- It does not turn "outbound marketing" into a hidden side effect. Outreach is
  a plan lane here; real delivery routes to `send-as` and then a provider
  adapter. Branded provider skills are concrete adapters, not branches in this
  core graph.
- It does not treat the graph receipt as proof that an external provider action
  happened. Provider actions need provider evidence and their own receipt.

## When to use this skill

- To show how runx chains skills into replayable business operations.
- To prototype a team-specific ops graph before wiring private provider tools.
- To route a product signal without giving the agent blanket repo, email,
  wallet, or deployment access.
- To explain why a governed workflow is more useful than a one-shot prompt:
  the route, stops, handoffs, and readbacks are explicit and replayable.
- To smoke-test graph execution and child receipts with no external account.

## When not to use this skill

- To run a production launch, incident, release, campaign, support reply,
  payout, or spend flow as-is. Replace fixture lanes with real skills first.
- To approve a live send, spend, merge, publish, deploy, or customer-visible
  action.
- To hide project policy, customer lists, credentials, wallet keys, provider
  dumps, or private review context in the signal.
- To claim external work completed when only this fixture graph ran.

## Mental model

```text
standalone: signal -> one typed lane -> downstream handoff
durable:    signal -> one typed lane -> append -> projection readback
fanout:     signal -> all planning lanes -> approval stops -> governed handoffs -> proof
```

The useful part is the reusable route. A direct call returns one packet instead
of forcing the operator through the whole operating system. A chain may persist
that packet or request the complete fan-out, where some lanes are read-only,
some draft-only, some blocked until approval, and one proof lane states how
later execution should be verified.

## How this maps to real runx work

- **Docs and public proof** route to a docs skill such as `sourcey` or a
  product-owned documentation lane.
- **Release preparation** routes to `release`, with publish held behind a
  release approval.
- **Code work** routes to `issue-to-pr` or a project-owned implementation lane,
  with merge held behind review.
- **Outreach and customer communication** route first to `send-as`, then to a
  provider connector that implements the send lane. Branded provider skills are
  the right place for vendor-specific compose, test, review, schedule, or send
  details. Broad outbound marketing should be its own skill or product broadcast
  skill, not extra logic hidden in this graph.
- **Spend and payments** route to quote or payout skills with caps, recipient,
  rail, and settlement proof separated from the planning lane.
- **Proof** routes to receipt/history/audit skills and provider readbacks.

The fixture `ops-lane` step simply returns these packets without performing the
handoff. In a real project, replace each fixture lane with the named governed
skill runner or provider tool.

## Procedure

1. Receive one concise `signal` and optional `operator_context`.
2. Route the selected `lane`, which defaults to `classify`.
3. Mark the lane as read-only, draft-only, approval-required, or proof-only and
   name its exact downstream owner.
4. Return the typed lane packet and seal the graph receipt.
5. If durable state is actually required, select `route_and_append`, supply the
   data source, aggregate, expected version, and idempotency key, then read back
   the projection.
6. If the complete planning artifact is actually required, select `main` to
   fan out docs, release, issue, outreach, spend, and proof packets.

## Edge cases and stop conditions

- **Missing signal:** return `needs_input`. There is no safe route.
- **Vague objective:** return a narrow classify packet and ask for the missing
  product, audience, repo, release, amount, or provider context.
- **Live send without principal, audience, consent, digest, and approval:** stop
  at the outreach lane and route to `send-as`.
- **Spend without amount, cap, recipient, rail, and approval:** stop at the
  spend lane and route to a quote or payment skill.
- **Merge, publish, deploy, or destructive mutation without approval:** stop at
  the relevant lane and name the missing gate.
- **Provider success without provider evidence:** do not mark complete. Route to
  proof audit.
- **Secret or private data in the signal:** refuse to echo it into outputs;
  require redacted context or a provider-side readback instead.

## Output schema

The default graph's public result is one `lane_packet`, ready for the named
downstream skill. The explicit `main` runner instead returns the complete
`lane_packets` artifact shown below. Runtime graph context retains producer
outputs for nested execution, while machine-readable CLI output omits exact
result-producer duplicates and child receipts bind the execution lineage.

```yaml
lane_packets:
  schema: runx.business_ops_route.v1
  signal: string
  lanes:
    classify: lane_packet
    docs: lane_packet
    release: lane_packet
    issue: lane_packet
    send: lane_packet
    spend: lane_packet
    audit: lane_packet
```

Each `lane_packet` has this shape:

```yaml
lane_packet:
  schema: runx.business_ops_lane.v1
  lane: string
  signal: string
  status: ready | awaiting_approval | needs_input | refused
  decision: route | prepare | draft | quote | verify | stop
  kind: router | docs | release | work | outreach | spend | proof
  consequence: read_only | draft | live_mutation | public_send | money_movement | proof
  summary: string
  why: string
  authority:
    requested: [string]
    provided: fixture_only
  gate:
    approval_required: boolean
    approval_gate: string | null
    stop_reason: string | null
  handoff:
    interface: skill | graph | cli | hosted_api | workflow | provider_tool
    lane_ref: string
    runner_ref: string | null
    command_hint: string | null
  evidence:
    inputs_required: [string]
    readbacks: [string]
    receipt_refs: [string]
  risks: [string]
  next: [string]
```

## Worked example

```bash
runx skill business-ops \
  -i signal="launch readiness for API v2: docs, release, customer comms, and spend checks" \
  --json
```

The default graph returns one classify lane packet with its authority class,
evidence requirements, and next governed handoff. It does not configure a data
store, run the other planning lanes, or call an external provider.

## Inputs

- `signal` (required): concise business operations signal to classify and route.
- `lane` (optional, default `classify`): the one bounded lane to return.
- `operator_context` (optional): product policy, project topology, audience
  constraints, or known provider state. Context only, not authority.
- `route_and_append` additionally requires `data_source_ref`, `resource`,
  `aggregate_id`, `expected_version`, and `idempotency_key`; these identify
  durable state and therefore are never guessed.
- `main` accepts the signal and context only, and must be selected explicitly.
