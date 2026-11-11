# Composed journey coverage

[`0.12-composed-journeys.json`](../../plans/evidence/0.12-composed-journeys.json) retains the 109 operator journeys that depend on prior
evidence from the recorded pre-cutover source commit. A source file and JSON
pointer identify each journey; `eval_file` and `eval_id` identify its native eval.
Identical prompts in different source cases remain distinct scenarios.

Each native prompt preserves the original request using current vocabulary.
Its expected output retains the original outcome, with explicit refusal or
evidence limits where the source case requires them. Every original
`must_not_repeat` item is retained in `expectations`, with only the cutover
vocabulary for methods updated. The archive preserves the original wording,
pointers and outcomes verbatim. Prior evidence is carried in the eval's
declared local `files`.
The crosswalk contains grading criteria and is never attached to an attempt.

Attachments contain bounded fixture data, not a fresh provider read, a usable
credential, native approval, or a grant. Receipt projections retain replay
limits. The inventory fixtures were produced by native `inventory-file`, and
the vulnerability advisory receives a completed native source-bound review
with the composed test's supplied assessment. These captures establish local
mechanics; their source findings are synthetic. No target advisory draft is
included in that input.

This is definition coverage, not a 109-case model pass rate. Native inspection
checks the evals and admits their declared file contents; package tests exercise
the existing safe fixtures. Model evals use `skill-review` against a released,
digest-bound copy and require their own recorded attempts and grading evidence.
A denied effect does not satisfy a criterion requiring delivery and readback.
A JSON description of a paused run does not install that run's durable state.
