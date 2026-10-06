# Delivery Report: crm-cleanup 0.3.0 for Frantic #79

## What was built

Improved the first-party `crm-cleanup` skill (0.1.0 to 0.3.0) with deterministic guarantees and a mock CRM transport:

- **Idempotent proposals**: identical updates deduplicated
- **Conflict refusal**: conflicting values for same record+field refuse the run (`update.conflicting_values`)
- **Typed values**: numbers and booleans preserved
- **Apply runner**: executes sealed proposals through a mock CRM transport, returning `write_result{before,after}`

## Verification

- **Harness**: 10/10 cases pass, 0 assertion errors
- **Registry**: published as `ahmeda-afk/crm-cleanup@sha-3c2d0012b6d7`, live at https://runx.ai/x/ahmeda-afk/crm-cleanup@sha-3c2d0012b6d7
- **PR**: https://github.com/runxhq/runx/pull/528 (head ed2a0148, DCO signed)
- **Dogfood**: web-fetch records, 3 updates proposed and executed, receipt `sha256:ebfebebc6bee12789c708d4f69d160e287d0edade4dad904862f05927769cdc6`
- **CLI**: runx-cli 0.9.1

## How to use

1. Install: `runx add ahmeda-afk/crm-cleanup@sha-3c2d0012b6d7 --registry https://api.runx.ai`
2. Reconcile: `runx skill ahmeda-afk/crm-cleanup@sha-3c2d0012b6d7` with transcript, records, schema
3. Apply: use the `apply` runner with the sealed proposal to execute via transport
4. Verify: check the receipt and write_result before/after

## Files

- `skills/crm-cleanup/X.yaml` (v0.3.0, 10 harness cases)
- `skills/crm-cleanup/SKILL.md` (operator docs)
- `skills/crm-cleanup/crm-cleanup.mjs` (deterministic finalize + apply)
- `skills/crm-cleanup/fixtures/` (dogfood fixtures)
