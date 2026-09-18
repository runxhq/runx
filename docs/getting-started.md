# Getting Started

This walkthrough proves the local runx path with one small skill. It uses the
checked-in `examples/hello-world` package so the commands stay tied to the repo.

## Prerequisites

- Git and Node.js 20 or newer. Node supplies both npm for installing Runx and
  the checked-in `hello-world` runner command.
- Rust 1.97 or newer only when building the CLI from source.
- pnpm 10 or newer only when running repository checks.

Install the native CLI and check that it starts. These commands are the same in
Bash on macOS/Linux and in PowerShell on Windows:

```bash
npm install --global @runxhq/cli
runx --help
git clone --depth 1 https://github.com/runxhq/runx.git
cd runx
```

Contributors building from source can instead run
`cargo build --manifest-path crates/Cargo.toml -p runx-cli` and replace `runx`
below with `crates/target/debug/runx`. On macOS 26, complete the
[Developer Tools permission prerequisite](../CONTRIBUTING.md#macos-developer-tools-permission)
before troubleshooting a stalled Rust build.

## Run The Example

On macOS or Linux, choose a temporary receipt directory and run the skill:

```bash
export RUNX_RECEIPT_DIR="$(mktemp -d)"
runx skill ./examples/hello-world \
  --message "hello from docs" \
  --json
```

The equivalent PowerShell commands on Windows are:

```powershell
$env:RUNX_RECEIPT_DIR = Join-Path ([System.IO.Path]::GetTempPath()) "runx-first-receipt"
New-Item -ItemType Directory -Force $env:RUNX_RECEIPT_DIR | Out-Null
runx skill .\examples\hello-world --message "hello from Windows" --json
```

The `runx.skill_run.v1` response should report `status: "sealed"` and include a
`receipt_id`.
For local development, no production signer is required: when the signer
environment is absent, runx seals local-development receipts. Publishing and
hosted verification still require real authority.

## Inspect The Receipt

The quickstart writes the sealed receipt and its local index inside the
directory stored in `$RUNX_RECEIPT_DIR` in Bash or
`$env:RUNX_RECEIPT_DIR` in PowerShell. Use the id from the previous command as
a detailed history query:

```bash
runx history <receipt-id> --detail --json
```

The `runx.receipt_inspection.v1` projection should show the receipt status,
authority, decisions, acts, seal summary, and local issuer verification. It
does not reproduce the execution input or output body.

The receipt you just sealed was signed by the local runtime, so its
`verification` block reports:

```json
"verification": {
  "status": "unverified"
}
```

That is the expected starting point rather than a failure. No trusted verifier
is configured yet, and `runx verify` does not choose a trust level for you. Ask
it to verify the id from the previous command and it stops with:

```text
runx: runx verify requires a trusted receipt verifier. Set both
RUNX_RECEIPT_VERIFY_KID and RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64,
configure a complete RUNX_RECEIPT_SIGN_* identity, or pass
--allow-local-development-signatures for local fixture receipts only.
```

To check the receipt you just sealed, accept the local-development signature
explicitly:

```bash
runx verify <receipt-id> --allow-local-development-signatures
```

```text
signature mode: local-development
note: local-development signatures were accepted only because --allow-local-development-signatures was set; set RUNX_RECEIPT_VERIFY_KID and RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64 to verify production signatures
tree sha256:<receipt-id> (1 receipt): ok
verification: ok
```

Read that verdict as scoped: `verification: ok` covers the receipt tree, the
signature, and the digests under the local-development trust level. The flag
only accepts local-development signatures. A trusted verdict for a production
receipt comes from the verification key in
[Production Receipt Signing](#production-receipt-signing), which is the only
path to a production-trusted verdict.

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
runx history <receipt-id> --detail --json
```

## Next

- Use `runx new docs-demo --objective "Create a bounded
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
