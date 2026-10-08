---
name: diagnose-skill-run
description: Diagnose a failed or suspicious Runx skill execution from receipts and harness evidence, then propose the smallest bounded improvement without replaying the failed operation.
---

# Diagnose Skill Run

Diagnose what went wrong in a skill or graph execution and propose the
smallest change that fixes it.

Resolve `receipt_id` through the native `ledger read` runner, then combine its
redacted receipt detail with the supplied failure summary or harness output.
The native detail is authoritative for status, verification, authority, acts,
decisions, criterion status, references, and seal posture. It deliberately
excludes hydrated step output, stdout, stderr, credential values, context
bodies, and local paths; those details must come from supplied bounded failure
evidence when they are necessary.

Distinguish root cause from symptoms. A graph may report failure at step 4,
but the root cause may be bad output from step 2 that propagated through
context passing. Trace data flow backward through context edges to find
where the problem originated.

Classify the failure:

- **Input error** — required input missing or malformed. Fix: input
  validation or input resolution.
- **Scope denial** — step requested scopes outside the graph grant.
  Fix: scope declarations or grant configuration.
- **Tool failure** — CLI tool or adapter returned an error. Fix: tool
  invocation (args, env, cwd) or the tool itself.
- **Schema mismatch** — step output did not match expected shape for
  downstream context. Fix: output parsing or artifact contract.
- **Timeout** — step exceeded time budget. Fix: increase timeout,
  reduce work, or split the step.
- **Policy denial** — transition gate blocked the step. Fix: gate
  conditions or upstream output.
- **Review rejection** — adversarial review found blocking issues.
  Fix: the code or spec, not the review process.
- **Harness assertion** — fixture expectations did not match actual
  output. Fix: skill logic or stale fixture expectations.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `ledger#read`

## Agent-mediated suspension is not a failure

A receipt sealed with reason `needs_agent`, or whose graph status is `deferred`, denotes a healthy
agent-mediated suspension, not a defect. The runtime yielded to the
caller for missing agent or human input.
This is a normal part of graph execution, not one of the failure
classes above. When the only evidence is `needs_agent` without
any exit code, scope denial, schema mismatch, or other concrete
failure signal, return `verdict: pass` with an empty
`improvement_proposals` array and note that the graph is paused as
designed.

One failure, one fix. Propose the smallest change that addresses the root
cause. Do not bundle unrelated improvements.


## Output

The stable failure packet is consumed directly by `skill-lab improve`. Its
output shape is formalised as JSON Schema at
[review-receipt-output.schema.json](../../schemas/review-receipt-output.schema.json).
Agents should self-validate before returning, and downstream
consumers (notably the `skill-lab improve` runner) may validate on receipt.

- `verdict`: `pass`, `needs_update`, or `blocked`.
- `failure_summary`: which step, which failure class, what root cause.
  One to three sentences.
- `improvement_proposals`: array of bounded changes. Each:
  - `target`: what to change (SKILL.md, execution profile, graph step, input, fixture)
  - `change`: what specifically to change
  - `rationale`: why this fixes the root cause
  - `risk`: what could go wrong
- `next_harness_checks`: replayable checks that should pass after the fix.

## Inputs

Supply whichever evidence is available:

- `receipt_id`: receipt id to inspect.
- `receipt_summary`: sanitized receipt or harness summary.
- `receipt_details`: native redacted receipt projections for deterministic
  replay only; live runs resolve them from `receipt_id`.
- `harness_output`: failed harness output or assertion text.
- `skill_path`: path to the skill being improved.
- `receipt_rows`: native-projection rows for deterministic replay only; live
  runs resolve `receipt_id` from the configured receipt store.

## Agent task contracts

### `diagnose-skill-run`

Diagnose one failure from native redacted receipt detail plus the supplied summary or harness
output. Treat needs_agent or a deferred receipt as a healthy suspension unless another concrete
failure signal exists. Ground authority, acts, decisions, criteria, and seal posture in
receipt_evidence.receipt_details; treat caller summaries as supplemental evidence, never as a
replacement for native facts. Distinguish input, scope, tool, schema, timeout, policy, review,
and harness failures. If receipt_id was supplied but native evidence did not match it, return
blocked unless the supplied harness output alone proves the defect. Return verdict, a concise
failure_summary, at most three bounded improvement_proposals, and replayable
next_harness_checks. Each proposal must name target, change, rationale, and risk. Do not write
files or weaken a refusal.
