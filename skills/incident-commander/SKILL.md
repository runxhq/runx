---
name: incident-commander
description: Advance one declared incident through a fixed roster, approval-bound communications planning, and receipt-backed resolution while canonical agency owns durable state.
---

# Incident Commander

Use this skill for one command decision inside an incident that already belongs
to a Runx `agency` case. It checks the folded case and fixed roster, asks the
canonical `ops-desk` skill for one bounded decision, then enforces the incident
rules before returning `incident_turn`.

The result is one bounded, receipt-backed command turn: an exact dispatch,
approval escalation, verified resolution from a supplied member receipt, or
actionable blocker. This skill does not append an agency event, acquire a case
lease, send a message, call a provider, or close external state. The agency
driver remains responsible for the expected-version append and for binding
this run's sealed receipt to the case stream.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `ops-desk#advance`

## Required context

Supply `case_id`, `driver_id`, `incident_objective`, folded `case_state`, and a
fixed `roster`. The objective must be one of `begin`, `assign`, `send`,
`resolve`, or `postmortem`.

The roster has exactly three entries:

- `commander`, which normally names `ops-desk` and its incident command scope;
- `responder_lead`, which names the response skill and its allowed scope;
- `comms_lead`, which names `send-as` or `slack-notify` and its allowed scope.

Each entry includes a principal, skill, and nonempty scope ceiling. Roles and
principals must be unique. An `assign` turn also needs `case_state.named_owner`
to match one roster role or principal.

`case_state` is a bounded projection, not the incident event history. Durable
history stays behind the agency data source. Include declaration status,
severity, scope, current turn, and only the evidence or pending decision needed
for this command turn.

## Communications

A send starts from `case_state.pending_escalation.proposed_handoff`. That object
binds the exact `comms_lead` principal, `send-as` or `slack-notify`, runner
`plan`, channel, audience, and SHA-256 content digest.

The optional `approval` must contain that same principal and a nonempty reason.
Without approval, a valid handoff returns `awaiting_approval`. An approval from
another principal is refused. A matched approval can return a downstream
planning handoff with state `ready_for_planning`; `delivery_status` remains
`not_sent` and provider delivery remains `not_executed`.

Run the named skill separately. `send-as` plans a provider-neutral send.
`slack-notify` owns its governed Slack delivery lane. Return the later sealed
receipt through `member_result` and advance the agency case again. A delivery
claim without `runx:receipt:sha256:<digest>` is refused.

## Resolution and durable state

Only `resolve` may return `resolved`. It needs either
`case_state.resolution_evidence_ref` or a successful `member_result` with a
valid Runx receipt reference. The result still says `agency_state:
not_persisted`; the agency driver must append the turn under compare-and-swap.

The normal driver sequence is:

1. Open the case with canonical `agency open` if it does not exist.
2. Advance agency to obtain the folded state and bounded dispatch context.
3. Run Incident Commander with that context and keep its sealed receipt.
4. Pass the result and receipt back to agency for the expected-version append.
5. Run a named responder or communications skill only when the accepted turn
   calls for it, then feed that sealed member receipt into the next agency turn.

The agency receipt proves persistence. The Incident Commander receipt proves
the local command decision. A downstream provider receipt and its readback prove
delivery. Do not substitute one receipt for another owner's effect.

## Outcomes

`incident_turn.decision` is one of:

- `advanced`: a bounded dispatch or verified member result can continue;
- `awaiting_approval`: a valid communications proposal needs its exact roster
  principal's approval;
- `resolved`: linked resolution evidence permits the agency driver to persist
  closure;
- `needs_input`: required operator context, such as an assignment owner, is
  absent;
- `refused`: supplied state, roster, approval, scope, or evidence contradicts
  the incident rules.

Every result includes case and turn identity, a reason, validation findings,
and effect state. Missing information is not reported as a negative operational
fact. A refusal does not mutate the case.

## Recovery

For `needs_input`, correct the folded projection or name the roster owner and
rerun. For `awaiting_approval`, keep the proposed audience and digest unchanged,
obtain approval from the exact `comms_lead`, and rerun. For `refused`, resolve
the listed validation finding rather than weakening the roster or scope. For a
provider or responder failure, preserve its receipt and let agency decide the
next bounded turn.

Inspect and test the package with the repository-built CLI:

```text
./crates/target/debug/runx skill inspect ./skills/incident-commander --json
./crates/target/debug/runx harness ./skills/incident-commander --json
```

Registry publication is separate from authoring. Preserve the contributor
identity `mossony/incident-commander`; claim registry availability only after a
publish readback for the exact package digest.
