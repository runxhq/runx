---
name: agency
description: "Record a bounded multi-turn case and propose one roster-constrained member dispatch at a time; a trusted external driver must execute and verify each member."
runx:
  category: ops
---

# Agency

Record and plan a standing case toward a mandate, one turn at a time.

This skill stores a fixed roster, mandate, limits and event stream through
`data-store`. Each `advance` asks `ops-desk` for one next move and checks that a
proposed member and scope strings fit the stored roster. That check does not
grant authority to a child run. The skill neither executes members nor verifies
caller-supplied `member_result` receipts. Its own turns are receipted; member
results remain assertions from the external driver until independently checked.

It is not a durable-execution engine or an autonomous daemon. An external
trusted driver must run each member under an actual grant, verify its receipt
and provider effects, then supply the result on the next `advance`. The current
implementation does not enforce a pending-dispatch/result handshake, so do not
use it as an unattended team runner or combine it with another operator's work
queue. The local personal assistant does not compose this skill.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `data-store#append_event`
- `data-store#read_events`
- `ops-desk#advance`

## What this skill does

- `open` starts a case: it appends `opened` with the mandate, the roster, and the
  cumulative limits snapshot, so the charter travels with the case.
- `advance` runs one turn: it folds the case from its event stream, asks `ops-desk`
  for the single next move constrained to the roster, enforces the measurable gate,
  records one turn event whose append is the contention lease, and names the member
  to run. The member runs as a separate governed run; its outcome is fed back to the
  next `advance` as `member_result`. The skill does not verify that result against
  the previous dispatch or native receipt ledger.
- `status` folds and returns the current case state.

The case reducer is the agency's own code, because `data-store` carries events but
does not fold domain state. Everything else is delegation.

## When to use this skill

- A trusted external driver already owns execution, approval and recovery and
  needs a bounded, reviewable case decision/event log.
- The driver can independently verify member outcomes before feeding them back.

## When not to use this skill

- One-shot or interactive work. Call the member skills directly; this case log
  is overhead when the operator is already the loop.
- An unattended assistant with its own goal, assignment and recovery state.
- To compute proposals (that is `ops-desk`) or product domain logic such as
  claim and clock rules (that is the product's own governed skill from the
  registry). Compose them.
- To bake a storage backend. The case lives in `data-store` via `data_source_ref`.
- To let the model invent the roster, the mandate, or the limits. They are operator
  config, snapshotted into the case at `open`.

## Procedure

1. `open` the case with the mandate, roster, and limits.
2. `advance` the case. Read the turn packet:
   - `advanced`: run the named member under its scope, then `advance` again with the
     member's outcome as `member_result`.
   - `awaiting_approval`: stop. The current package has no authenticated
     approval-resolution runner; an external trusted driver must resolve and
     record the decision before any further advance.
   - `resolved` or `failed`: the case is closed.
3. Repeat until the case resolves. The driver, not this skill, decides the cadence.

## The measurable gate

`advance` folds submitted member results and compares their count and reported
spend with configured caps. `max_turns` currently counts submitted member acts,
not all stored turn events. Reported spend is caller-supplied, so these are
planning checks, not authoritative effect limits. The reducer does not enforce
TTL. Real money movement must route through the hosted `spend` contract, where
the payment runtime owns authoritative reservation and aggregate limits.

## Contention

Each turn appends a single event keyed
`case_id:turn:driver_id` at the folded `expected_version`. Two drivers racing the same
turn carry different keys, so the loser hits a hard version conflict rather than
replaying the winner. This protects one append race; it is not an execution
lease. A later `advance` can propose another dispatch before the previous member
returns. The external driver must prevent that until a pending-dispatch check
is implemented.

## Edge cases and stop conditions

- No case at `case_id`: open the case first. The current graph does not
  deterministically return the documented `needs_input` packet.
- A cumulative cap is reached: the turn is `failed` with the breached predicate named.
- The best move is consequential and unapproved: `awaiting_approval` with the prompt.
- No roster member can act and nothing is escalatable: escalate to the configured
  human with the missing input named.
- The graph reads at most 500 events and does not check continuation metadata.
  A longer stream cannot be trusted as a complete case projection.

## Output schema

The planner emits an `agency_turn` artifact; the graph's direct result is the
`data-store` append result. Inspect the artifact and append receipt together:

```yaml
agency_turn:
  schema: runx.agency.turn.v1
  status: advanced | awaiting_approval | resolved | failed
  case_id: string
  turn: number
  dispatch:                 # present when status == advanced
    member: string
    skill: string
    task: string
    needed_scope: [string]
  approval_prompt: string | null
  resolution: object | null
  predicates: object        # the measurable over_limits booleans
  reason: string | null
  next: string
```

## Inputs

- `open`: `data_source_ref`, `case_id`, `agency_ref`, `mandate`, `roster`, `limits`,
  optional `signal`.
- `advance`: `data_source_ref`, `case_id`, `driver_id`, optional `member_result`.
- `status`: `data_source_ref`, `case_id`.

## Worked example

Open a docs case with a researcher, writer, and reviewer and a 50-turn limit.
`advance` folds an empty-but-opened case, ops-desk picks the researcher, and the turn
returns `advanced` naming the researcher. The driver runs the researcher and calls
`advance` again with its result; ops-desk now picks the writer to draft. When the
reviewer approves and the projection shows the docs current, `advance` returns
`resolved`.

## Turn rules

- Fold every turn from the sealed stream; never infer state the events do not show.
- Enforce the measurable gate before the model's judgment; never widen a cap.
- Name the member and verification expectation on every dispatch. The trusted
  external driver must verify the child receipt and effect before reporting it;
  this skill does not enforce that check.
- Compose ops-desk, data-store, and the members; never reimplement them, and never
  invent the roster or the mandate.
- Stop cleanly with needs_input, awaiting_approval, refused, or failed; never a fake
  ready.
