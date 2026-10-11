---
name: product-analytics
description: Analyze bounded product funnel and retention evidence with deterministic rates, explicit measurement definitions and source-bound recommendations; expose instrumentation and comparison gaps without claiming lift.
---

# Product analytics

Explain where a defined product journey loses people or accounts, what the
retention evidence shows, and which measurement or product change to consider.
This skill covers activation and product usage. `organic-growth` owns search and
acquisition strategy; it may consume the relevant findings.

`analyze` accepts supplied governed aggregate counts. Retrieve a saved insight
through a selected PostHog connector, or use another provider, before invoking
this skill. Preserve the actual report and its source digest; do not invent counts
to fit the input. This runner does not query a provider, execute SQL, capture
events, change instrumentation or create tasks.

## Measurement admission

Each report carries the definition its own source was computed under: product,
timezone, person/account identity, cohort, filters, funnel rule and retention
rule. Read it from that report's saved query; never copy one report's definition
onto another. Funnel counts must count the same eligible entities through
ordered steps, not independent event totals. Retention needs an explicit
eligible cohort and return period. Mark incomplete provider results incomplete.
Retain observation time; a cached PostHog result is not fresh merely because its
HTTP read happened today.

The pure calculation step preserves counts and computes rates. Zero eligible
entities produce `null`, not a measured 0% conversion or retention rate. Ordered
counts cannot increase and retained entities cannot exceed the eligible cohort.
Compare a baseline only when its definition equals the current definition field
for field, with the same ordered events and retention period over an
equal-duration, nonoverlapping window. Rate changes are percentage points, never
mislabeled relative percentage lift. Missing denominators, incompatible
definitions or incomplete reports withhold recommendations and expose the
measurement gap.

## Interpretation

Use the deterministic rates; do not replace or round them into different facts.
Distinguish observations, hypotheses and actions. Concentrate on the consequential
drop-off, plausible instrumentation problems and a small next experiment. A change
between periods does not establish causality or statistical significance. Counts
alone cannot establish sample comparability, seasonality or experiment validity.
Do not average cohort percentages or claim significance without the actual method
and experiment evidence. State those limits when relevant.

Each material finding and proposed action must cite admitted source digests.
Recommend a concrete measurement, owner role and stop/reconsider condition; do
not invent a named owner, deadline, revenue impact or work already completed.
Native evidence verification binds the resulting review to the exact measurement
context. The calling application owns continuing work, scheduling and approved
changes.

## Agent task contract

### `product-analytics-analyze`

Return `analytics_candidate` with `decision: ready`, the exact `objective`, a
concise `summary`, `findings` and `limitations`. Each finding has `claim`, `kind`
(`observed`, `hypothesis` or `action`) and `source_digests`. An action also has
`measurement`, `owner_role` and `stop_condition`. Every finding and action is
verified through the same native claim binding. Cite only admitted digests. Explain zero
denominators and uncertainty plainly. Return `needs_more_evidence` when the
bounded evidence cannot support useful recommendations. Never claim execution.
