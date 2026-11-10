---
name: diagnose-skill-run
description: Diagnose a failed or suspicious Runx skill execution from receipts and test evidence, then propose the smallest bounded improvement without replaying the failed operation.
---

# Diagnose Skill Run

Diagnose what went wrong in a skill or workflow execution and propose the
smallest change that fixes it.

The default resolves an exact `receipt_id` through native `receipt-query#read`, then combines its
redacted receipt detail with the supplied failure summary or test output.
The native detail is authoritative for status, verification, authority, acts,
decisions, criterion status, references, and seal posture. It deliberately
excludes raw execution bodies, stdout, stderr, credential values and context
bodies. When a retained declared output matters, use the offered native
`receipt.prove`/`receipt.read` tools to follow verified child references and read
only the exact relevant claim or JSON pointer. Read permitted source and
explicitly supplied ordinary logs with bounded `fs.read_bundle`/`fs.search`.
Those tools retain the existing workspace and private-path guards. A denied,
unretained or omitted diagnostic is an evidence gap, not permission to inspect
private runtime files or reconstruct the failed operation. Raw transport logs
are not exposed by the claim reader; request the specific bounded sanitized
material if it is needed.

Distinguish root cause from symptoms. A workflow may report failure at step 4,
but the root cause may be bad output from step 2 that propagated through
context passing. Trace data flow backward through context edges to find
where the problem originated.

Classify the failure:

- **Input error** — required input missing or malformed. Fix: input
  validation or input resolution.
- **Scope denial** — step requested scopes outside the workflow grant.
  Fix: correct an overbroad request or the intended binding under operator
  authority; never silently widen the grant.
- **Tool failure** — CLI tool or adapter returned an error. Fix: tool
  invocation (args, env, cwd) or the tool itself.
- **Schema mismatch** — step output did not match expected shape for
  downstream context. Fix: output parsing or artifact contract.
- **Timeout** — step exceeded time budget. Fix: increase timeout,
  reduce unnecessary work or preserve progress across bounded pages. Inspect
  the owning operation before increasing a timeout or imposing a product cap.
- **Policy denial** — transition gate blocked the step. Fix: gate
  conditions or upstream output.
- **Review rejection** — adversarial review found blocking issues.
  Fix: the code or spec, not the review process.
- **Test assertion** — fixture expectations did not match actual
  output. Fix: skill logic or stale fixture expectations.

## Composes

<!-- Generated from the native closure; run pnpm core-skills:composes:generate. -->

- `receipt-query#read`

## Agent-mediated suspension is not a failure

A receipt sealed with reason `needs_agent`, or whose workflow status is `deferred`, denotes a healthy
agent-mediated suspension, not a defect. The runtime yielded to the
caller for missing agent or human input.
This is a normal part of workflow execution, not one of the failure
classes above. When the only evidence is `needs_agent` without
any exit code, scope denial, schema mismatch, or other concrete
failure signal, return `verdict: pass` with an empty
`improvement_proposals` array and note that the workflow is paused as
designed.

A ledger answer of `needs_agent` because no audit question was supplied is not
proof that a different run is paused. Check the actual run/receipt state; do not
classify from a matching word in a summary. Separate observed failure facts,
causal interpretation and hypotheses. If the required body or diagnostic is
unavailable, return `blocked` with the exact next evidence instead of a plausible
root cause. Do not equate valid receipt signatures with provider or test truth.

One failure, one fix. Propose the smallest change that addresses the root
cause. Do not bundle unrelated improvements.


## Output

The stable failure packet, `runx/diagnose-skill-run.diagnosis.v1`, is consumed directly by
`skill-build improve`. Agents should self-validate before returning, and
downstream consumers (notably the `skill-build improve` method) may validate on
receipt.

- `verdict`: `pass`, `needs_update`, or `blocked`.
- `failure_summary`: which step, which failure class, what root cause.
  One to three sentences.
- `improvement_proposals`: array of bounded changes. Each:
  - `target`: what to change (SKILL.md, execution profile, workflow step, input, fixture)
  - `change`: what specifically to change
  - `rationale`: why this fixes the root cause
  - `risk`: what could go wrong
- `next_test_checks`: replayable checks that should pass after the fix.

## Inputs

Supply whichever evidence is available. Without a receipt ID, the default does
not scan the ledger: it diagnoses only the bounded supplemental material. A
completed `receipt_evidence` packet skips lookup; `from-evidence` accepts it
explicitly for a workflow. Preserve the source receipt and its verification
posture; reuse is not independent verification in this phase. Neither path
replays the failed work.


- `receipt_id`: exact receipt id to inspect.
- `receipt_evidence`: completed `runx/receipt-query.answer.v1` to reuse.
- `receipt_summary`: sanitized receipt or test summary.
- `receipt_details`: native redacted receipt projections for deterministic
  replay only; live runs resolve them from `receipt_id`.
- `test_output`: failed test output or assertion text.
- `skill_path`: path to the skill being improved.
- `receipt_rows`: native-projection rows for deterministic replay only; live
  runs resolve `receipt_id` from the configured receipt store.

## Agent task: diagnose-skill-run

Return a `diagnosis` object with verdict, failure_summary, improvement_proposals
and next_test_checks. Optional `evidence` entries are exact source references;
they are reviewer references, not self-authenticating proof. The deterministic
finish preserves the existing diagnosis packet and its evidence posture.

Diagnose one failure from native redacted receipt detail plus the supplied summary or test
output. Treat needs_agent or a deferred receipt as a healthy suspension unless another concrete
failure signal exists. Ground authority, acts, decisions, criteria, and seal posture in
receipt_evidence.receipt_details; treat caller summaries as supplemental evidence, never as a
replacement for native facts. Distinguish input, scope, tool, schema, timeout, policy, review,
and test failures. If receipt_id was supplied but native evidence did not match it, return
blocked unless the supplied test output alone proves the defect. Return verdict, a concise
failure_summary, at most three bounded improvement_proposals, and replayable
next_test_checks. Each proposal must name target, change, rationale, and risk. Do not write
files or weaken a refusal.

Read only what the question requires. Do not echo secret values from a retained
claim or supplied log. Report a sensitive field/locator and the required handling
without claiming the entire omitted log is secret-free. If the fault is in caller
inputs, a stale fixture or an owning native boundary, name that owner; do not add
a compatibility alias or weaken the skill to make the bad invocation pass.
