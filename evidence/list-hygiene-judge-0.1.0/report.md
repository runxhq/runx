# Frantic bounty #68 delivery report: list-hygiene-judge

A published runx graph-runner skill that makes the dangerous part of list hygiene — the durable
consent-state transition — a sealed, auditable judgment. It reads the contact through the pinned
data-store first, decides verify / suppress / re-permission / stop deterministically, and records
the transition with exactly one compare-and-set `append_event`. It never sends.

- **Package and publish.** Exact name `list-hygiene-judge`, published by `antheducation` as
  `antheducation/list-hygiene-judge@sha-bcb77b9828ed` (X.yaml 0.1.0) via
  `runx login --provider github --for publish` + `runx registry publish
  ./skills/list-hygiene-judge/SKILL.md --registry https://api.runx.ai` with runx-cli 0.8.2.
  Live listing: https://runx.ai/x/antheducation/list-hygiene-judge@sha-bcb77b9828ed.
- **Sealed read before verdict.** The graph's first step is `data.read_projection` on the contact
  entity (`aggregate_id`) at the pinned `data_source_ref`; the judgment consumes that sealed read,
  cross-checks the caller's typed `engagement_history` against it, and never invents metrics.
- **Ungated compare-and-set write.** Suppress and re-permission each append one event under
  `idempotency_key` + `expected_version` (dogfood: version 0 → 1, event_ref
  `contact_events:contact:dana@example.co:1`), then the recorded transition is read back from the
  projection. No `operational_proposal` envelope, no minted grant. A retry with the same key
  returns the recorded version instead of double-applying.
- **Refusal and stop paths are proven, not asserted.** The inline harness seals
  `sealed_decay_re_permission`, `sealed_hard_bounce_suppress`, and
  `stop_missing_or_stale_evidence` (stale `expected_version` → decision `stop`, `append_status`
  `skipped`, no append). The judgment also refuses suppression without hard-bounce evidence,
  escalates ambiguous bounce recovery, and refuses to re-permission a contact carrying an active
  unsubscribe marker — all landing in the `human_list_hygiene_review` lane with no write.
- **Hosted harness green.** After publish, the hosted harness passed 3/3 —
  https://api.runx.ai/v1/skills/antheducation/list-hygiene-judge/harness — with sealed receipts.
- **Production-signed dogfood.** A real post-publish run of the published version (clean
  `runx add`, then `runx skill antheducation/list-hygiene-judge@sha-bcb77b9828ed ...`) decided
  `re_permission` for a decayed contact and sealed root receipt
  `runx:receipt:sha256:fe371df690d45826806023848743dc191f677a3902d135ae0ae5e2efdc10ef19`. Strict
  `runx verify` (no development allowance): valid, signature mode **production**, kid
  `antheducation-hosted-1`; the full tree of 7 receipts verifies valid.
- **Send separation.** Every sealed packet carries `downstream_send_performed: false`. The live
  send is dispatch-by-naming to the separate governed `send-as` run, which re-reads the recorded
  consent state at send time and gates delivery — the suppressed contact cannot receive a campaign.
- **Reproducible without private context.** Install, run, and verify commands are recorded in
  `evidence.json`; unbound `local://` refs default to durable native SQLite, so any reviewer can
  replay the dogfood and check the receipt chain end to end.
- **Operator value.** Any team running mailing lists on runx gets a drop-in, receipt-audited
  hygiene judgment: engagement decay and bounces become recorded, reversible-by-append decisions
  instead of silent CRM edits, and the human lane catches every ambiguous case.
