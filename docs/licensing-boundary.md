# Connect/Auth Licensing Boundary

Status: current boundary.

## Rule

OSS crates may consume local credential material, enforce declared authority,
redact credential references, write receipt-safe observations, and act as a
credential-free client of the hosted grant and provider-operation API. OSS
crates must not broker third-party OAuth, custody hosted secrets, issue verified
hosted grants, implement a provider gateway, or implement real payment planning,
reservation, rails, ledgers, refunds, recovery, or finality.

The local credential path is intentionally small: a connector declares its
credentials, an operator binds one account as an instance, and the kernel
delivers that instance's material to the operation it admits. Local material is
encrypted at rest and presented only at the operation's boundary. The
hosted/cloud layer owns OAuth brokerage and hosted credential custody.

## Current OSS Surface

- `runx-contracts` keeps provider-neutral credential envelope and credential
  delivery contracts.
- `runx-core` keeps policy and authority admission. It does not call providers
  or issue grants.
- `runx-runtime` keeps local credential consumption, resolved delivery, and
  redaction. Its provider-neutral hosted client authenticates the operator,
  reads opaque grant metadata, and invokes one registered operation without
  receiving provider credentials.
- `runx-cli` keeps the native OSS CLI shape. `runx connect` is a client of the
  hosted grant lifecycle; it does not perform OAuth brokerage. `runx credential`
  stores local profiles as connector instances.
- `runx-sdk` does not expose hosted connect-list APIs.
- Public payment skills keep only thin v1 hosted operation contracts. Internal
  `mock-*` packages are deterministic simulators and always report that no money
  moved. There is no `runx-pay` crate or real payment rail in OSS.

## Denied OSS Surface

The manifest at `docs/license-boundary.manifest.json` is the machine-readable
guard input. It blocks hosted connect client identifiers, legacy connection
keys, and private provider implementation names in Apache-2.0 Rust crates, with the
single negative fixture allowlist required by the guard tests.

For TypeScript/JavaScript, `scripts/check-boundaries.mjs` additionally blocks
hosted OAuth credential contract shapes in active OSS code, fixtures, and
generated schemas.

## Private Home

Hosted OAuth, provider gateway routing, credential custody, authoritative grant
resolution, issuance, and revocation live in `../cloud/packages/auth` and the
cloud/API wiring that depends on it. Public OSS documentation may describe the
provider-neutral client contract but not private provider implementation
details.

Real payment admission, SDK integrations, rail calls, reservations, ledgers,
refund accounting, recovery, and finality live in Runx Cloud payment packages.
The OSS public contract and the private hosted operation use the same v1 Runx
operation names; greenfield contracts are changed in place rather than forked
into internal v2 variants.

The complete public behavior is documented in
[Credentials](./credentials.md).
