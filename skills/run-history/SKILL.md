---
name: run-history
description: Read the workspace's runs and skill catalog, then return deterministic run outcomes, paused runs, catalog test coverage, and governance follow-ups without model-authored metrics.
---

# Run History

Read the workspace's runs through the native `run.list` service and the
installed catalog through `runx.skill.inspect`. The method computes the report
itself; it does not ask an agent to invent commands, launch subprocesses, or
transcribe counts.

Use it for operational questions about recent Runx activity, failed or blocked
runs, paused runs, frequently run skills, and skill entries without fixtures or
inline test cases. Use `receipt-audit` or `diagnose-skill-run` to
inspect one suspicious run, and `least-privilege` when receipt-backed authority
evidence is available for grant review; actual usage and policy semantics are still needed before attenuation.

## Evidence boundary

- Live execution reads only the native run list and catalog projections. These
  services read the same run records and catalog parser as the Runx CLI
  without routing back through the CLI as a subprocess.
- Run reads are bounded to 1,000 matching runs. Reaching that bound is explicit;
  the reported rates never claim whole-ledger coverage. An explicit `as_of` is
  also passed as the native upper bound, excluding later runs.
- `runs` and `catalog_items` are replay inputs for tests and controlled
  analysis, and must be supplied together; replay never silently mixes caller
  data with live native state. Supply already-windowed projections and omit
  `period`/`as_of` for replay; time filtering belongs to `run.list`, not a second
  JavaScript history engine. Skill scope is applied before the replay limit,
  preserving the original supplied order.
- Empty history or a native list containing unreadable records returns
  `needs_more_evidence`. Unreadable run ids, update times and reasons remain in
  `runs.unreadable`; they count toward the native limit and are excluded from
  outcome rates. Native skill/time filters cannot attribute unreadable records
  to a narrower selection, so filtered reports do not establish their absence.
- `completed_rate` is the share of sealed runs whose outcome is `completed`.
- `refusal_rate` counts sealed runs that were `blocked` or `declined`. It is
  reported as an observation, not automatically treated as a defect.
- Catalog coverage is the number of native catalog entries declaring at least
  one fixture or inline test case. It is not a maturity grade.

## Output

`history_report` contains:

- the resolved query and source labels (`run.list`, `runx.skill.inspect`, or
  `supplied_replay`);
- sealed and paused run counts, outcome counts, completed/refusal rates;
- the most frequently run skills;
- catalog entry and test-coverage counts;
- bounded recommendations routed to `diagnose-skill-run`, `receipt-audit`, or
  `skill-build test`;
- limitations when native projections cannot support a stronger claim.

This skill never executes another skill, changes a grant, or changes a run.
