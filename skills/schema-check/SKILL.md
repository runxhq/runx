---
name: schema-check
description: Compare current and proposed JSON Schema acceptance, validate real payloads natively, and return a path-specific compatibility verdict with a gated publication proposal.
---

# Schema Check

Check whether a contract change can reject payloads the current contract accepts.
Supply the current schema, proposed schema, real payload examples and your
compatibility policy. The default `check` runner returns the affected paths,
sample failures, unsupported proof cases and, when allowed, a publication proposal
bound to the exact schema and sample values. It does not publish.

## What the comparison means

The direction is **current accepted instances → proposed accepted instances**.
For example, widening integer to number preserves acceptance. Adding an optional
string field to an open object can break it: that key previously allowed a
number. Adding the same field to an object that previously forbade it expands
acceptance. Removing a property follows the new additionalProperties rule; a
removed declaration is not automatically a rejected payload.

This is an input/data acceptance check. It does not establish response-consumer
compatibility, output completeness, business meaning or behavior across two API
implementations. Set policy `required_fields` for declarations the proposed
contract must retain, and assess consumer expectations separately. It never
transforms or strips fields from the actual data.

## How it works

1. Native `data.digest` binds the exact supplied JSON values for both schemas
   and the sample array. These identify values, not an unprovided original file.
2. Native `data.validate` compiles each original schema with external retrieval
   disabled and validates every sample against both contracts. Formats are
   annotations. The native validation steps retain both sets of diagnostics;
   the result includes the proposed-schema errors by sample index.
3. The domain comparison checks implication for practical object properties,
   required declarations, schema-valued additionalProperties, homogeneous arrays,
   JSON types/unions, enum/const values, string and collection lengths, numeric
   endpoints and unchanged patterns. Integer is included in number. Integer
   multipleOf relaxations are proved; decimal-divisor changes remain conservative.
   Local JSON Pointer references resolve against each original schema, including
   escaped pointer segments. Equal ref strings do not hide a changed definition.
4. Potential narrowing is refused unless `breaking_allowed` is true. A policy
   exception is stated in the notes; it does not turn that break into an
   unchanged acceptance guarantee. Missing policy-required fields always refuse.
5. Passing samples supplement the implication check. They never prove a changed
   unsupported constraint. Changed composition/tuple/conditional applicators,
   changed regular-expression inclusion, recursive inclusion, ref assertion
   siblings, nested resource IDs and external references require separate review.
   Identical assertion sets need no new inference. A policy exception cannot
   bypass an unsupported proof case.

The proof is conservative: interacting constraints can make a reported potential
break unreachable. Inspect its path and the exact contracts rather than treating
a potential break as an exhibited failing payload. Unknown semantics hold the
proposal instead of disappearing behind the examples.

## Using the result

`schema_check` keeps the existing `runx/schema-check.check.v1` packet.
`breaking_changes` explains potential narrowing; `validation.findings` explains
unproven semantics. `migration_notes` records changes, policy exceptions and
which declared paths occur in samples, including homogeneous array items.
`proposal` is null when held. When present, it carries the native schema/sample
digests, affected paths and `live_write_performed: false`.

The same runner is reusable in a graph. A parent supplies its exact schemas,
samples and policy, then consumes this verdict and these digests without a second
comparison. No discovery, provider credentials, model turn or delivery is needed.
Publication belongs to the schema publisher under its own authorization and
readback. A compatible proposal is not publication approval.

For a normal list-contract change, widening a locally referenced item from
integer to number while lowering minItems preserves acceptance. Requiring a new
item field, tightening an endpoint or constraining a formerly open extra key
produces a path-specific refusal even when the examples happen to pass.
