# Demo Gallery

These demos are runnable from this repository and produce signed receipts, sealed
under your runx home's own local key; `runx verify <receipt-id>` checks them. The
standalone verifier at `tools/verify/verify.mjs` checks the committed fixture
receipts against the fixture key in `tools/verify/runx-demo-jwks.json`. That key's
seed is public, so the file lists it only as a `local` fixture identity, never as
`hosted`.

`docs/demo-inventory.json` is the machine-checked source of truth for featured
demos, runnable previews, and fixture support.

## Shipped Demos

| Demo | Proof | Run | Gate |
| --- | --- | --- | --- |
| `examples/hello-world` | Native CLI top-level skill and harness baseline. | `runx harness examples/hello-world` | harness |
| `skills/data-store` | A provider-agnostic data operation appends and reads durable SQLite state through a governed adapter envelope. | `runx harness skills/data-store/fixtures/append-read-sqlite-event.yaml` | harness |
| `skills/nws-weather-forecast` | The official NWS skill reads through its `national-weather-service` connector and seals stable provider metadata. | `runx harness skills/nws-weather-forecast/fixtures/nws-forecast-washington-monument.yaml` | live external |
| `examples/loop-orchestration` | A bounded outer loop submits governed runx turns, prints receipt ids and next-turn reasons, demonstrates `context_skills`, and includes a refusal path. | `sh examples/loop-orchestration/run.sh` | harness |

## Payment Contract Proof

The `fixtures/skills/mock-charge`, `mock-pay` and `mock-refund` fixtures provide
the only local payment simulation and always report `money_moved: false`. Public
payment packages use hosted provider-response fixtures that prove approval,
idempotency, mutation, and readback contracts without embedding a rail
implementation in OSS. Real settlement dogfood belongs to Runx Hosted.

The upstream x402 conformance and interop scripts remain protocol checks only;
they are not local payment adapters and do not prove a Runx-hosted settlement.

## Verify A Receipt

The verifier is independent of runx runtime code. It checks the receipt's
shape, recomputes the canonical receipt body hash, checks the content-addressed
receipt id, verifies the Ed25519 signature, and can walk receipt ancestry from
top-level receipt-store artifacts.

```sh
node tools/verify/verify.mjs /path/to/receipt.json \
  --jwks tools/verify/runx-demo-jwks.json

node tools/verify/verify.mjs /path/to/graph-root-receipt.json \
  --jwks tools/verify/runx-demo-jwks.json \
  --walk-ancestry \
  --receipt-dir /path/to/receipt-store
```

All demos call `tools/verify/verify.mjs` directly; there is no second verifier
entrypoint to drift from the canonical implementation.
