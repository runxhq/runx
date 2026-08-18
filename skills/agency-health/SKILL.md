---
name: agency-health
description: Assemble a typed health bundle for one running agency case by composing the registry-pinned data-store read_projection with cross-run ledger aggregates, grade findings against a declared baseline, and seal a read-only health_verdict plus named intervention findings; the lane itself moves no money, grants no authority, and routes consequences through separate governed runs.
runx:
  category: agency-ops
---

# Agency Health

`agency-health` reads one running agency end to end, grades its operational
health, and seals a typed verdict plus named intervention findings. It is
**read-only**: it appends nothing to the case stream, sends nothing, executes
nothing, and consumes no effect. Operators (or downstream drivers) use the
sealed findings to launch separate, scoped governed runs (a `policy-author`
tighten, an `improve-skill` debug, a `human ops` escalation); this skill never
mints, settles, sends, or widens authority itself.

Use this skill when one of your agencies has been running for at least a
couple of days and you need an at-a-glance signal about whether its stream is
healthy (turns advancing, refusals steady, spend well below cap) versus
needing an intervention (stuck turns, refusal spikes, cap pressure). Do not
use this skill to *resolve* an incident, refund a customer, or rewrite a
policy; the verdict names a lane to call next. `run-history-analyst` audits
the whole receipt ledger; `receipt-auditor` walks one receipt; this skill
walks one live agency case end to end.

## Operating model

The skill is composed of one read-only graph that runs in order:

1. **inspect-case** — read the agency case state by composing the
  registry-pinned `data-store` read_projection (C2) keyed on the agency
  case, returning events folded in turn order with each event's
  `case_id`, `turn`, `driver_id`, `event_kind`, and `version`.
2. **aggregate-ledger** — read cross-run aggregates (seal rate, refusal
  spikes) by composition with the `ledger` read runner (C7), referencing
  receipts by `id-stub` only (the ledger is audit-only and can never be a
  domain-keyed state read).
3. **grade** — fold the case projection and the ledger aggregates, grade
  the four canonical signals (seal_rate, stuck_case_count, cap_usage_pct,
  escalation_backlog) against `health_baseline` thresholds or the supplied
  defaults, and emit one `health_verdict.status` (healthy / degraded /
  at_risk) plus a list of typed `findings` that each ground a metric in a
  folded case_id and turn number or a referenced ledger id-stub.
4. **name-lane** — for each warrant of intervention, attach a
  `target_lane` (one of `policy-author`, `improve-skill`, `human ops`)
  and a one-line `reason` that the operator can act on. Lanes that would
  widen a cap or authority, and any critical finding, escalate to the
  `human ops` lane.
5. **seal** — emit a thin review act over the verdict (status `sealed`
  for `ready` and `sealed` for `needs_more_evidence` STOP case) without
  composing anything in the case stream.

The two harness cases correspond to the canonical decision branches:

- `concerning-agency-sealed` — running agency with stuck turns and cap
  pressure yields `decision=ready`, `health_verdict.status=degraded`,
  graded findings, and three typed intervention findings naming
  `policy-author`, `improve-skill`, and `human ops` lanes; expect
  `status: sealed`.
- `no-case-events-stop` — an `agency_ref` with no readable case events
  over the period yields `decision=needs_more_evidence`, no findings
  graded, no intervention emitted, and `status: sealed` for the
  deterministic conflict that still seals.

## Skill chain

- **Upstream:** the agency itself (the case stream this skill reads is the
  same stream the agency's runners append to).
- **Downstream:** `policy-author` (to tighten a policy or timeout),
  `improve-skill` (to debug a member behind a refusal spike), `human ops`
  (to escalate cap-widening or critical findings). Use `vault-unseal` to
  audit the cases this skill flags as `at_risk`.

## Inputs

The runner accepts these typed inputs:

```text
data_source_ref:    string   Provider reference, e.g. "registry-pinned/0.6.14".
store_id:           string   Registry store id (the pinned data-store).
agency_ref:         object   {case_id, driver_id, agency_charter_id, period}.
period:             object?  Optional {start_turn, end_turn, allow_partial=false}.
case_id:            string?  Optional override; defaults to agency_ref.case_id.
health_baseline:    object?  Optional {threshold_days_stuck=3, cap_pressure_pct=80, refusal_spike_rate=0.10}.
```

## Outputs

The runner returns one typed decision:

```text
decision:
  ready|needs_more_evidence|needs_human
health_verdict:
  status: healthy|degraded|at_risk
  period: {start_turn, end_turn}
  findings:
    - signal: seal_rate | stuck_case_count | cap_usage_pct | escalation_backlog
      assessment: pass | warn | fail
      measured: <typed>
      baseline: <typed>
      grounded_in:
        case_id: <opaque>
        turn: <int>      # for case projection reads
        ledger_id_stub: <opaque>   # for ledger aggregate reads
      reason: <one-line>
intervention_findings:
  - target_lane: policy-author | improve-skill | human ops
    reason: <one-line action prompt>
    grounded_in: { case_id, turn, ledger_id_stub }
    severity: warn | critical
```

This lane moves no money and grants no authority. The `human ops` lane is
the only escalation route for any cap-widening or authority-widening
remedy; this skill refuses to widen a cap or grant access itself.

## Refusals

The skill refuses to grade a signal not grounded in the folded case
projection or a ledger id-stub aggregate, refuses to invent a cap or
threshold it cannot read from the agency charter snapshot or the supplied
`health_baseline`, and never invents a turn state the sealed event order
does not show. A `needs_more_evidence` decision is a **deterministic**
sealed case, never a soft skip; the operator sees the refuse reason and
extends the period or supplies the missing baseline.
