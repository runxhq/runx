---
name: skill-review
description: Inspect, safely test, and assess one Runx skill package for capability, trust, and operator readiness. Use when deciding whether to adopt, improve, reject, install, or publish a skill; its evidence-only assess method is available when native test evidence already exists.
---

# Skill Review

Evaluate whether one skill actually helps an operator. The default method passes
a local package to native `runx.skill.validate`, which parses and inspects the
exact package and safely replays eligible planning/read tests without operator
credentials. Its single assessment turn can read relevant implementation through
`fs.read_bundle`; execute-capable targets are inspected, never run automatically.
A passing authored test is mechanical evidence, not a usefulness verdict.

This skill does not mutate the package, publish a report, install a package, or
manufacture missing evidence. Use `assess` directly when a caller already has a
bounded evidence pack. That phase offers no filesystem or provider tools and does
not replay the test. Source references without their contents or meaningful
execution evidence may require more evidence; they are not permission to fetch.

## Composes

<!-- Generated from the native closure; run pnpm core-skills:composes:generate. -->

- `agent#read`
- `agent#write`

## Procedure

1. Resolve a workspace- or owning-skill-relative package path and capture its
   native capability and readiness envelope. Registry or marketplace material
   must first be installed or supplied as a bounded evidence pack; this skill
   does not fetch it implicitly.
2. Run the native test only when catalog execution is `read` or `plan`.
3. Compare the advertised job with the actual default inputs, implementation,
   deliverable and recovery path. In the direct method, read the relevant manual,
   manifest, domain modules and contracts named by the native source closure
   through `fs.read_bundle`. Use the native resolved package path and normal
   workspace/owning-skill containment. Preserve each read digest and partial flag;
   do not read unrelated data or credentials. Reuse source already present in the
   evidence pack instead of reading it again. Catalog descriptions alone do not
   establish implementation behavior.
4. Separate native test results, provider readback, supplied assertions, and
   unverified claims.
5. Start from an ordinary request. State what the caller must still supply or do,
   what useful work the skill actually finishes, and whether a downstream workflow
   can reuse its output without duplicate reads, judgment, effects or authority.
   Identify a thin wrapper, missing advertised operation, unused result or invented
   prerequisite explicitly. Assess stop behavior, authority, provenance and recovery.
   Separate declared evals and supplied fixture answers from actual
   completed jobs. Native receipts establish recorded execution, not semantic truth.
6. Return `needs_more_evidence` when the evidence cannot support a trust
   decision. Never upgrade a parse result or prose claim into execution proof.
7. Recommend `adopt`, `adopt_with_caveats`, `improve`, or `reject`,
   naming the evidence and blocking gaps behind the decision.

## Measuring selection

`triggers` measures how often a host selects one package for each request in
its `eval_queries.json`. Each request is worked by fresh turns of the `agent`
skill's `read` method, `repeats` of them (3 when absent), on whatever host and
model work this run's turns: run it with `--drive` and the model you name in
`runx config`, or from a host attached to the run. A turn selected the package
when the run's record shows a load in that turn that bound the package's
exact digest, whether the load completed or waits on the person's trust. What
a turn says it did is never the evidence, and a load of another package
doesn't count.

A request passes when the package is selected at least half the time and it
should be, or less than half the time and it shouldn't. The result is each
request's rate and the totals for each kind. A rate belongs to the host and
model that produced it, and is never blended with another's. The turns compete
against the whole catalog the host sees: the workspace's packages and the
registry `RUNX_REGISTRY_URL` names. Matching by digest means a turn that loads
an older published copy of the package hasn't selected the candidate, whose
description is what's measured.

## Running evals

`evals` runs each eval in `evals/evals.json`, `repeats` times (once when
absent), in a fresh turn of the `agent` skill's `write` method under this
run's authority. No eval writes to the world: a scripted model's run uses
provider fixtures, and a real model's runs under `effects: none`, with its
reads live and recorded as live, so a write is refused at any depth.

Each attempt is judged by `grade`, a fresh turn given the eval, the attempt as
its record holds it (its outcome, the calls it made and how each ended, its
receipt), and the eval's criteria set: its expected output, then each of its
expectations, every one required, made by a deterministic step, never by the
attempt or the grader. The grader returns a `runx.verdict.v1`, and the runtime
names `grade` as its evaluator. The run decides each verdict by the one rule
against the package at its digest and that criteria set: `met`, `not_met`, or
`inconclusive` when a required check is left unanswered. A verdict that judges
something else, answers another set, or names another evaluator is refused,
and a refused verdict answers nothing, which isn't `inconclusive`.

## Stop conditions

- Refuse evidence containing secrets, raw credentials, private customer data,
  private inbox content, or provider dumps.
- Do not recommend provider readiness without provider readback.
- Do not recommend publication from private previews, placeholder hosts, dead
  links, or unrelated parent domains.
- Return `needs_more_evidence` when no native inspection, test, receipt, or
  equivalent bounded source evidence is supplied.
- Reject capabilities that cannot be bounded, audited, or assigned a truthful
  terminal state.

## Output

- `capability_profile`: bounded capability, execution shape, and claimed effect.
- `trust_assessment`: evidence tier, caveats, and unsupported claims.
- `test_matrix`: passed, failed, skipped, and still-required checks.
- `recommendation_report`: decision, rationale, blockers, and next action.

## Inputs

- `skill` (required): local package path for the default method; any stable
  reference may label an evidence-only `assess` run.
- `evidence_pack` (optional for the default method, required for `assess`):
  inspection, test, receipts, docs, or source evidence. References are
  preferable to copied private bodies.
- `objective` (optional): decision the evaluation should support.
- `test_constraints` (optional): time, environment, or safety limits.

## Agent task: skill-review-assess

Assess only the supplied evidence for one skill. Separate declared capability, native
inspection, test evidence, provider readback, supplied assertions, and unverified claims.
Check the happy path, stop or refusal path, authority, artifact or effect, provenance, and
recovery posture appropriate to the capability. Return needs_more_evidence when the packet
cannot support a decision. Never infer provider readiness from a parse result, local supplied
answer, or prose claim. Recommend adopt, adopt_with_caveats, improve, or reject with
concrete blockers.


The assessment output makes product value explicit. `capability_profile` includes
`skill`, `summary`, `operator_job`, `default_result`, `caller_work`,
`value_judgment` (`useful`, `thin_wrapper`, `incomplete`, or `unknown`),
`value_reason` and `composition`. `trust_assessment` separates `tier`, `caveats`,
`implementation_evidence` and `product_evidence`. Each `test_matrix` entry names
a check, its `passed`, `failed`, `skipped` or `required` status, and actual evidence.
`recommendation_report` includes `recommendation`, `rationale`, `blockers` and
`next_action`; use `needs_more_evidence` when source or useful-result evidence is
missing. Copy native evidence identifiers rather than inventing them. Do not use
`adopt_with_caveats` to hide a missing advertised core capability. A useful local
draft or analysis may be a complete job without a provider write, but its value
must be visible in the artifact and caller journey.

## Agent task: grade

Judge the attempt against each criterion in the criteria set, from the
attempt as its record holds it: its outcome, its result or failure, and each
call it made and how that ended. Answer every criterion: `met` when the record
shows the question answered yes, `not_met` with what was expected, what
happened and where when it shows no, and `inconclusive` when the record can't
say. Never answer from what the attempt says it did where its calls say
otherwise. Return one `verdict`: `schema` `runx.verdict.v1`, the `subject` and
`criteria_set` exactly as given, an `assessments` entry per criterion with the
attempt's receipt as its evidence when it has one, and the whole by the rule:
`not_met` if any required criterion is, `inconclusive` if any is unanswered or
inconclusive, otherwise `met`. Don't name an evaluator: runx names it.
