---
name: rollback-judge
description: Read a public post-deploy monitor run and produce a bounded rollback, roll-forward, or escalation decision without performing the release action.
runx:
  category: ops
---

# Rollback Judge

Rollback Judge is a read-only review skill for post-deploy incidents. Its
default runner fetches a public GitHub Actions deployment-monitor run at
execution time, reads the deployment marker pinned to that run's commit, and
compares the observed error rate with its threshold. A rollback can be approved
only when the current monitor run failed and a separately supplied prior run
proves a healthy version.

The skill never deploys, rolls back, publishes, calls a write API, or mints
authority. Its default graph emits a typed answer for the existing
`release.publish.approval` gate, then passes that answer to a mock release rail
inside the same run. The mock rail consumes `approved:true` and seals a release
advance result so the receipt proves the decision was consumed; a real release
skill or project-declared release interface would own any live downstream
effect.

## Source Contract

`deploy_signal.evidence.source_url` must be a public GitHub Actions run API URL:

```text
https://api.github.com/repos/<owner>/<repo>/actions/runs/<run-id>
```

The judge fetches that URL during the run. It then uses the returned repository
and full `head_sha` to read `current_version.marker_path` from the immutable
commit on `raw.githubusercontent.com`. The marker must contain:

```yaml
schema: rollback-judge.deployment-target.v1
service: checkout
version: 2026.07.15.4
health: degraded
signal_kind: error_rate_spike
metric:
  name: http_5xx_rate
  observed_percent: 18.4
  threshold_percent: 2
  window: 15m
incident: post-deploy http_5xx_rate exceeded the release threshold
```

For rollback, `prior_version.source_url` must identify another completed monitor
run. The judge fetches its commit-pinned marker and accepts it as a target only
when the run concluded `success`, the marker is `healthy`, and its observed
metric does not exceed the threshold.

## Decision Rules

- Roll back only when the live current run is completed with `failure`, the
  current marker is unhealthy, and the measured error rate exceeds its
  threshold.
- Locate the rollback target only through `prior_version.source_url`; never use
  a hand-typed version or digest as proof of the prior release.
- Roll forward only when a failing live monitor is present and
  `forward_fix_evidence` includes test runs plus review signoff.
- Hold and escalate when the run is incomplete, the workflow does not match,
  the API result contradicts the marker, the source cannot be read, the current
  signal is nonfailing, or the prior release is not proven healthy.
- Never accept `severity: critical` by itself as evidence of failure.

## Typed Inputs

- `deploy_signal`: `{ severity, kind, evidence }`; evidence carries the public
  monitor `source_url` and optional expected `workflow_name`.
- `current_version`: deployment subject with `service` and `marker_path`.
- `monitor_run_ref`: public HTML URL of the current monitor run. The judge
  verifies that it equals the `html_url` returned by the API before sealing;
  stock runx binds this trusted URL as the review act target.
- `prior_version`: rollback candidate with its own public `source_url` and
  `marker_path`.
- `forward_fix_evidence`: `{ test_runs, review_signoff }`.

## Typed Output

```yaml
decision:
  action: rollback | roll_forward | hold
  reason: string
  version_target: object | null
escalation:
  required: boolean
  reason: string | null
  missing_evidence: [string]
release_publish_approval:
  gate_id: release.publish.approval
  approved: boolean
  reason: string
  answer:
    approved: boolean
    reason: string
release_execution_result:
  rail: mock-release
  gate_id: release.publish.approval
  consumed: boolean
  advanced: boolean
  target_ref: string
  command_digest: string
review_record:
  form: review
  signal: { severity, kind }
  evidence_used: [string]
  refused: { reason: string | null }
source_read:
  current: object
  prior: object | null
```

`source_read` records the API URL, fetched time, response SHA-256, ETag, monitor
run id, workflow, conclusion, commit SHA, public run URL, and the commit-pinned
deployment marker. `release_execution_result` records the same-run consumption
of `release.publish.approval` by the mock release rail and whether it advanced.
The sealed review act binds the derived decision, reason, release target, and
the consumed release effect; callers do not provide `act_decision` or
`act_target_ref`.

## Run

Install the published package, then pass the four typed objects:

```bash
runx skill <owner>/rollback-judge@<version> --registry https://api.runx.ai --json \
  --input-json deploy_signal='<json>' \
  --input-json current_version='<json>' \
  --input monitor_run_ref='https://github.com/<owner>/<repo>/actions/runs/<run-id>' \
  --input-json prior_version='<json>' \
  --input-json forward_fix_evidence='<json>'
```

The package includes `fixtures/critical-signal.json` with public, immutable run
references. Hosted harness replays expected answers tied to those references so
it remains deterministic; the default `judge` runner used for dogfood performs
the network reads and the same-run mock release consumption. The contradictory
case receives no answer, stops with `needs_agent`, and emits no decision or
approval.
