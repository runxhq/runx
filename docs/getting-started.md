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

The `runx.skill_run.v1` response should report `status: "sealed"` and include a
`receipt_id`.

Runx signs every receipt. With no signer configured, it signs with this runx
home's own Ed25519 key, which it creates at `~/.runx/keys/local-receipt-ed25519.json`
on the first run, readable only by you, and never replaces. Publishing and
hosted verification still require a hosted issuer.

## Inspect The Receipt

The quickstart writes the sealed receipt and its local index inside the
directory stored in `$RUNX_RECEIPT_DIR` in Bash or
`$env:RUNX_RECEIPT_DIR` in PowerShell. Use the id from the previous command as
a detailed history query:

```bash
runx history <receipt-id> --json
```

The `runx.receipt_inspection.v1` projection should show the receipt status,
authority, decisions, acts, the seal's reason code and criteria, and local
issuer verification. It
does not reproduce the execution input or output body.

## Production Receipt Signing

For receipts a hosted or CI identity signs, configure its Ed25519 signing key
before running skills, graphs or MCP server calls:

```bash
export RUNX_RECEIPT_SIGN_KID="hosted-prod-key"
export RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64="<32-byte-ed25519-seed-base64>"
export RUNX_RECEIPT_SIGN_ISSUER_TYPE="hosted"
```

All three variables must be set together. `RUNX_RECEIPT_SIGN_ISSUER_TYPE` must
be `hosted` or `ci`; a local identity is only ever the home's own key. When
configured, the runtime signs each receipt body digest with Ed25519 and writes
the matching public key hash in the issuer metadata. `runx history <receipt-id>`
and `runx verify` trust this home's own key and the configured signer's public
key, so the operator that created the receipts does not configure the same key
twice.

To verify receipts another home or a hosted issuer signed, configure its public
key, named by issuer type and key id:

```bash
export RUNX_RECEIPT_VERIFY_ISSUER_TYPE="hosted"
export RUNX_RECEIPT_VERIFY_KID="hosted-prod-key"
export RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64="<32-byte-ed25519-public-key-base64>"
runx history <receipt-id> --json
```

## Next

- Use `runx new docs-demo --objective "Create a bounded
  documentation decision skill"` to enter the canonical Skill Lab build lane.
  Runx inspects the catalog, returns an exact agent/resume handoff unless
  `--managed-agent` was explicitly authorized, and writes only the validated
  package. It never generates a placeholder module. To cold-start without
  installing Runx first, use the same command through `npx @runxhq/cli`.
- Compose the example into a graph with [Skill To Graph](./skill-to-graph.md).
- Bind provider accounts once as described in
  [Credentials](./credentials.md); agents and MCP use the same readiness path
  automatically.
- Publish a ready skill from a public repo at https://runx.ai/x/publish, or run
  `crates/target/debug/runx login --for publish` followed by
  `crates/target/debug/runx registry publish ... --registry https://api.runx.ai`.
  See [Publishing](./publishing.md) for the full local and hosted paths.
- See [API Surface](./api-surface.md) for public package exports.
