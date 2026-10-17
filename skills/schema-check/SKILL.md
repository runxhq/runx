---
name: schema-check
description: Check a proposed API or data contract against the current contract, real sample payloads, and a compatibility policy, and gate publication behind a refusable verdict with native digest evidence.
---

# Schema Check

Schema Check decides whether a proposed contract change can move forward
without silently breaking callers. It compares the current and proposed JSON
Schema contracts field by field, validates every supplied sample payload
against the proposed contract, applies the caller's compatibility policy, and
either refuses or emits a gated publish proposal. It never publishes anything
itself.

## Procedure

1. Native `data.digest` binds the exact current schema, proposed schema, and
   sample set before any judgment, so the verdict is tied to specific bytes.
2. Deterministic comparison walks both contracts: removed fields, type changes,
   fields that became required, new required fields, and narrowed enums are
   breaking; additive optional fields and expanded enums are notes.
3. Native `data.validate` compiles both schemas and validates every sample against
   each contract using JSON Schema 2020-12, with external retrieval disabled. Coverage is
   reported honestly: paths no sample exercises are flagged, not assumed
   covered.
4. The policy decides the verdict. Breaking changes are refused unless
   `breaking_allowed` is explicitly true, and policy `required_fields` must
   exist in the proposed contract.
5. A compatible verdict emits a `proposal` with the three digests and changed
   paths, gated on a schema publisher or human approver. A refused verdict
   emits no proposal. `live_write_performed` is always false.

Invalid requests refuse with `policy_result: invalid_input` and findings rather
than guessing. Missing required inputs fail at admission.

## Output

`schema_check` (`runx.schema_check.v1`) carries `decision`, `policy_result`,
`breaking_changes`, per-sample `validation_results`, `migration_notes`
including coverage, the gated `proposal` or null, and `validation`.

Inputs are `current_schema`, `proposed_schema`, `sample_payloads`, and
`compatibility_policy`.

## What compatibility means

Sample validity is not a proof that every existing caller remains compatible.
The inclusion comparison supports object properties, required fields, scalar
types, enums and boolean additionalProperties. It compares root constraints as
well as nested fields. A newly constrained enum and a newly closed object are
changes that may reject existing callers.

The native validator supports the full self-contained schema, including
minLength, numeric bounds, nested arrays, composition and local references. If
a changed contract uses constraints outside the inclusion comparison's supported
language, the result is refused with `compatibility.unsupported` findings, even
if its samples happen to pass or breaking changes are allowed. Identical schemas
need no inclusion inference but still compile and validate their samples.
Formats are annotations. This skill never fetches remote schemas.

For example, adding `minLength: 10` to a string with sample `"x"` fails native
validation; it cannot receive a compatible verdict. Missing required fields,
removed enum values and unknown comparison semantics remain visible. A proposal
is not publication approval. Its digests bind the exact proposed change; the
actual schema publisher owns authorization, publication and readback.
