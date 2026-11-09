# Examples

Runnable reference skills that demonstrate each runx front. These are examples,
not catalog entries: `runx list skills|tools|packets` reads `skills/` and `tools/`,
so the examples here are intentionally absent from that catalog.
Run them directly instead.

For a curated list of runnable proof demos and offline receipt verification, see
`docs/demos.md`.

Each run seals its receipts under your runx home's own local key, created on
first use; `runx verify <receipt-id>` checks them.

## Featured demos

The curated proof set is also machine-checked in
`../docs/demo-inventory.json`.

| Example | Front | Run |
| --- | --- | --- |
| `hello-world` | process tool (one-step method) | `runx test examples/hello-world` |
| `loop-orchestration` | outer loop over governed runx runs, with receipts, refusal, and agent context preview | `sh examples/loop-orchestration/run.sh` |

## Runnable previews

These are useful local proofs, but they are not the featured first-window demo
set.

| Example | Front | Run |
| --- | --- | --- |
| `managed-agent` | agent (host-drives default; yields `needs_agent` to the calling agent) | `runx test examples/managed-agent` |
| `hello-workflow` | a method of two skill steps | `runx test examples/hello-workflow` |
