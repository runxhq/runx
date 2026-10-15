---
name: sql-analyst
description: Answer a bounded data question from one declared governed read; use the explicit plan runner when only a schema-validated analysis plan is wanted.
---

# SQL Analyst

Use this skill to answer a bounded data question from one declared, typed data
source operation. The default validates the plan, performs the exact `store`
read, and interprets only that returned evidence. The explicit
`plan` runner stops at a reviewable read-only plan.

The schema and optional sample snapshot must carry stable upstream SHA-256
digests and observation times. Runx treats them as caller-supplied provenance,
not provider verification. Deterministic admission rejects stale or malformed
sources, write intent, invalid identifiers or dialects, unknown allowed tables,
and unbounded row requests before the model runs. The model designs a plan
against a normalized table/field index. A deterministic finalizer then rejects
invented tables and fields, untyped joins, unstructured filters, literal filter
values, invalid limits, incomplete interpretation, and write tokens.

This skill never emits or executes raw SQL. The default requires a validated
`execution_context` naming the `store` operation `projection.read`,
`events.read`, or `heads.list`; it performs that read and returns an
interpretation. The
explicit `plan` runner may omit the context and returns `planned_only`.

## Inputs

- `question`: bounded analysis question.
- `schema_summary`: source-bound available tables and fields.
- `dialect`: `postgres`, `sqlite`, or `mysql`.
- `as_of` and `max_schema_age_days`: deterministic source-freshness boundary.
- `sample_rows`: optional source-bound snapshot containing at most 20
  non-sensitive rows.
- `constraints`: allowed tables, maximum rows, and privacy limits.
- `execution_context`: required for the default; an exact `store` read
  operation and bounded resource inputs. It is optional only for `plan`.

## Output

The default returns `analysis_result` from the validated plan and actual
store read evidence. `plan` returns the existing
`runx.data.sql_analysis_plan.v1` packet.

## Agent task contracts

### `sql-plan`

Produce sql_plan_draft using only analysis_context tables and qualified fields. Return decision,
query_plan, validation_checks, interpretation, and residual_risks. The plan is read-only and
does not execute. Use the declared dialect and bounded limit. Do not invent schema, request
credentials, expose PII, or emit write SQL.

### `sql-interpret`

Answer the supplied question using only `validated_plan` and `governed_data`.
Return `analysis_result` with a direct answer, material observations, caveats,
and the data operation evidence reference. Never claim access to rows, fields,
or freshness not present in the governed result.
