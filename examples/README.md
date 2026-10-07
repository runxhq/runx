# Examples

Runnable reference skills that demonstrate each runx front. These are examples,
not catalog entries: `runx list skills|graphs|tools` scans `skills/`, `graphs/`,
and `tools/`, so the examples here are intentionally absent from that catalog.
Run them directly instead.

For a curated list of runnable proof demos and offline receipt verification, see
`docs/demos.md`.

Most need a receipt-signing identity (runx mandates signed receipts). A demo-only
identity:

```sh
export RUNX_RECEIPT_SIGN_KID=runx-demo-key
export RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64=QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=
export RUNX_RECEIPT_SIGN_ISSUER_TYPE=hosted
```

## Featured demos

The curated proof set is also machine-checked in
`../docs/demo-inventory.json`.

| Example | Front | Run |
| --- | --- | --- |
| `hello-world` | cli-tool (top-level runner) | `runx harness examples/hello-world` |
| `loop-orchestration` | outer loop over governed runx turns, with receipts, refusal, and agent context preview | `sh examples/loop-orchestration/run.sh` |

## Runnable previews

These are useful local proofs, but they are not the featured first-window demo
set.

| Example | Front | Run |
| --- | --- | --- |
| `managed-agent` | agent (host-drives default; yields `needs_agent` to the calling agent) | `runx harness examples/managed-agent` |
| `hello-graph` | graph harness baseline | `runx harness examples/hello-graph/harness.yaml` |

## Fixture support

`host-protocol` holds a Python host bridge sample for the OpenAI host adapter.
It is intentionally not a user-facing demo by itself.
