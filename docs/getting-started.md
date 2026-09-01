# Getting Started

This walkthrough proves the local runx path with one small skill. It uses the
checked-in `examples/hello-world` package so the commands stay tied to the repo.

## Prerequisites

- Rust 1.97 or newer for the native CLI path.
- Node.js 20 or newer for the checked-in `hello-world` runner command. No
  TypeScript install is required for the native CLI path.
- pnpm 10 or newer only when exercising the npm wrapper or TypeScript package
  tests.

Build the native CLI from the OSS workspace:

```bash
cargo build --manifest-path crates/Cargo.toml -p runx-cli
```

On macOS 26, complete the
[Developer Tools permission prerequisite](../CONTRIBUTING.md#macos-developer-tools-permission)
before troubleshooting a stalled Rust build.

## Run The Example

Run the skill directly through the CLI:

```bash
export RUNX_RECEIPT_DIR="$(mktemp -d)"
crates/target/debug/runx skill examples/hello-world \
  --message "hello from docs" \
  --json
```

The JSON response should report `status: "sealed"` and include a receipt id.
For local development, no production signer is required: when the signer
environment is absent, runx seals local-development receipts. Publishing and
hosted verification still require real authority.
The npm wrapper may be used for package-distribution checks, but it should
delegate to the same Rust binary behavior.

## Inspect The Receipt

The quickstart writes receipts to the temporary directory stored in
`RUNX_RECEIPT_DIR`. Use the id from the previous command as a history query:

```bash
crates/target/debug/runx history <receipt-id> --json
```

The history projection should show a `runx.receipt.v1` receipt stamped by the
local issuer. It is durable local evidence that runx executed the skill,
recorded the input shape, and captured the output without relying on prose
claims.

## Production Receipt Signing

For production-trusted receipts, replace the demo key with an Ed25519 signing
key before running skills, graphs, harness replay, or MCP server calls:

```bash
export RUNX_RECEIPT_SIGN_KID="hosted-prod-key"
export RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64="<32-byte-ed25519-seed-base64>"
export RUNX_RECEIPT_SIGN_ISSUER_TYPE="hosted"
```

All three variables must be set together. `RUNX_RECEIPT_SIGN_ISSUER_TYPE` must
be `hosted` or `ci`; production receipts are never stamped as local issuers.
When configured, the runtime signs each receipt body digest with Ed25519 and
writes the matching public key hash in the issuer metadata. `runx history` and
`runx verify` derive the matching verifier from that complete signing identity,
so the operator that created local receipts does not configure the same key
twice.

For independent or read-only verification where the signing seed is
intentionally unavailable, provide the public verification key instead:

```bash
export RUNX_RECEIPT_VERIFY_KID="hosted-prod-key"
export RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64="<32-byte-ed25519-public-key-base64>"
crates/target/debug/runx history <receipt-id> --json
```

## Next

- Use `crates/target/debug/runx new docs-demo --objective "Create a bounded
  documentation decision skill"` to enter the canonical Skill Lab build lane.
  Runx inspects the catalog, returns an exact agent/resume handoff unless
  `--managed-agent` was explicitly authorized, and writes only the validated
  package. It never generates a placeholder module. To cold-start without
  installing Runx first, use the same command through `npx @runxhq/cli`.
- Compose the example into a graph with [Skill To Graph](./skill-to-graph.md).
- Configure provider-backed skills once with
  [Credential Resolution](./credentials.md); agents and MCP use the same
  readiness path automatically.
- Publish a ready skill from a public repo at https://runx.ai/x/publish, or run
  `crates/target/debug/runx login --for publish` followed by
  `crates/target/debug/runx registry publish ... --registry https://api.runx.ai`.
  See [Publishing](./publishing.md) for the full local and hosted paths.
- See [API Surface](./api-surface.md) for public package exports.
- Connect runx to real-world bounty work with [Frantic Bounty Guide](./frantic-bounty-guide.md).
