---
name: run-history
description: Read the workspace's runs and skill catalog, then return deterministic run outcomes, paused runs, catalog test coverage, and governance follow-ups without model-authored metrics.
---

# Run History

Read the workspace's runs through the native `run.list` service and the
installed catalog through `runx.skill.inspect`. The runner computes the report
itself; it does not ask an agent to invent commands, launch subprocesses, or
transcribe counts.

Use it for operational questions about recent Runx activity, failed or blocked
runs, paused runs, frequently run skills, and skill entries without fixtures or
inline harness cases. Use `receipt-audit` or `diagnose-skill-run` to
inspect one suspicious run, and `least-privilege` when receipt-backed authority
evidence is available for grant review; actual usage and policy semantics are still needed before attenuation.

## Evidence boundary

- Live execution reads only the native run list and catalog projections. These
  services read the same run records and catalog parser as the Runx CLI
  without routing back through the CLI as a subprocess.
- Run reads are bounded to 1,000 runs.
- `runs` and `catalog_items` are replay inputs for harnesses and controlled
  analysis, and must be supplied together; replay never silently mixes caller
  data with live native state.
- Empty history returns `needs_more_evidence` rather than a healthy verdict.
- `closed_rate` is the share of sealed runs whose disposition is `closed`.
- `refusal_rate` counts sealed runs that were `blocked` or `declined`. It is
  reported as an observation, not automatically treated as a defect.
- Catalog coverage is the number of native catalog entries declaring at least
  one fixture or inline harness case. It is not a maturity grade.

## Output

`history_report` contains:

- the resolved query and source labels (`run.list`, `runx.skill.inspect`, or
  `supplied_replay`);
- sealed and paused run counts, disposition counts, closed/refusal rates;
- the most frequently run skills;
- catalog entry and test-coverage counts;
- bounded recommendations routed to `diagnose-skill-run`, `receipt-audit`, or
  `skill-build harness`;
- limitations when native projections cannot support a stronger claim.

This skill never executes another skill, changes a grant, or changes a run.
