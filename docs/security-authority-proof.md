# Security Authority Proof

Runx receipts must explain the authority boundary without becoming a secret
side channel. A receipt's `authority` section records the execution terms
admission derived with their subset proofs, the grants that admitted each
effect and the execution boundary the runtime observed
(`receipt.authority.enforcement`). All of it is sealed into the signed receipt
body rather than kept as unsigned read metadata.

Allowed public fields:

- the admitted execution terms and their subset proofs
- granted scopes and grant references
- the connector, the instance and the credential delivery observation with its
  `material_ref` hash
- the typed execution-boundary observation and approval decisions

Banned fields:

- raw access tokens, refresh tokens, API keys, passwords, client secrets, and
  provider credential bodies
- full private stdout or stderr bodies in public projections
- ambient environment dumps or unbounded local command logs
- unchecked provider output bodies in comments, public evidence, or ledgers

Credential material is represented by hashed opaque handles such as
`material_ref_hash`. Receipt writers still hash stdout and stderr, and metadata is
passed through the receipt redactor before signing. Local secret handoff is owned by the connector instance and the Rust
`CredentialDelivery` boundary. Hosted provider execution keeps material in runx
Connect, which renders only the connector's declared requests. In both paths,
secret values may cross only the trusted driver or supervisor delivery channel,
not authority proofs, receipts, invocation metadata, adapter observations, or
public provider evidence. See [Credentials](./credentials.md).

## Ownership Boundary

`ExecutionBoundaryObservation` and the receipt's authority shapes live in
`runx-contracts`, which owns their portable JSON shape and generated schemas.
`runx-core` alone derives execution terms and their subset proofs. The runtime
adapter alone owns the observed execution-boundary value and binds it into the
receipt at seal time. Neither a skill nor an agent may self-attest that
observation.

The local runner applies authority in this order:

1. Manifest validation identifies the selected runner and the connector
   operations it calls.
2. Structural policy admission resolves the exact authority grant.
3. Each operation is admitted against the connector's selected instance, which
   supplies its credential as a redacted delivery.
4. The adapter or driver receives only the declared environment and that
   credential, executes through the lane's canonical boundary, and redacts
   captured output.
5. The signed receipt records the observed boundary, public observations, and
   output hashes without raw material.

## Hosted Grants

Connector `operation` steps on the hosted placement run under the connector
instance the operator confirmed with `runx connector use <connector> --hosted`
or `runx connector use <connector> <grant-id>`. Confirmation reads the grant from Connect and refuses one that is
not active or is bound to another connector revision: another name, another
manifest digest or another connector digest. Admission authenticates
the operator and reads that grant from Connect again for every call; a grant
that was revoked, rebound or is now held by another principal no longer admits
the call. No provider token or credential body crosses into the skill, and
granted scopes come only from grant evidence, never from the skill.

An operation step can also require exact `expected_result` identity fields
and project only declared `result_fields` before the result enters a receipt.
This prevents a correctly scoped call from being mistaken for the wrong
resource and keeps undeclared secret-adjacent material out of skill output.

The admitted grant is sufficient for a routine connector write. A
consequential write carries a typed `approval` request, which the connector
can make mandatory with `approval: required`. The kernel binds that request to
the exact intent and suspends for host-attested human resolution; the package
must not place a second graph approval beside the same effect. Approval is
therefore an explicit property of one act, not a generic consequence of write
scope.

When a provider-permission effect is admitted, the sealed step receipt records
the operator grant as a typed `runx:grant:*` reference under
`receipt.authority.grant_refs`. The grant id is evidence of the authority that
admitted the effect; it is not a credential body and does not carry provider
token material.

## Hosted Payment Authority

OSS payment skills cross the same generic provider-permission effect boundary
as other hosted operations. Their receipts record the grant that admitted the
hosted call and the provider evidence returned to the graph. They do not prove
the private payment runtime's internal reservation or ledger state.

Runx Hosted owns payment-specific attenuation, aggregate caps, reservations,
single-use capabilities, idempotency, recovery, finality, and private ledger
state. A hosted payment operation must fail closed before calling a rail when
those checks cannot be proven. OSS never substitutes local effect state or a
caller-authored settlement claim for hosted admission and provider readback.

## Offline Receipt Verification

`runx verify [receipt-id] [--receipt-dir dir] [--receipt <path|->] [--trust home|configured] [--json]`
re-checks sealed receipts from disk with no runtime or network dependency:
canonical body digests, content-addressed ids, linked-tree parent/child
integrity, scope adherence for privileged effects, and Ed25519 signatures
against trusted keys. A trusted key is named by issuer type, key id and public
key: the selected runx home's own key, a complete `RUNX_RECEIPT_SIGN_*`
identity's public key, and the key `RUNX_RECEIPT_VERIFY_ISSUER_TYPE`,
`RUNX_RECEIPT_VERIFY_KID` and `RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64`
name. `--trust configured` leaves out the home's key, as a host verifying
receipts it did not issue does. A receipt verifies only under a trusted key of
its own issuer type and key id; a verified `local` issuer means a runx home's
own key signed it, and whether that suffices is the reader's call. Store
mode groups receipts into
trees by lineage; a chain that points at a receipt missing from the store is
reported as incomplete and fails verification. Single-receipt mode emits one
`runx.verify_verdict.v1` JSON verdict suitable for hosted notaries and other
embedding surfaces. Because a single document cannot prove tree membership,
lineage is reported as `unverified` without failing an otherwise valid
receipt. The command exits non-zero on invalid receipts, so it can gate
automation.

`fixtures/receipt-verify/` is the conformance corpus for machine consumers.
Every embedding surface that claims to verify a runx receipt must replay those
fixtures through the pinned `runx` binary and match the expected verdicts
instead of carrying a second verifier implementation in another language.

Scope adherence is intentionally pure and offline. A receipt whose acts or
seal carry typed `EffectEvidence` without corresponding
`receipt.authority.grant_refs` produces `EffectGrantEvidenceMissing`, fails
verification, and exits non-zero. An act whose legitimacy names an execution
term (`runx:authority_term:<id>`) that `receipt.authority.terms` does not hold
produces `ActLegitimacyTermMissing`. This is the boundary between a signed
activity log and a governance proof: the receipt must show both the
privileged effect and the operator-granted authority that admitted it.

## Operator Authority Diagnostics

`runx doctor authority [--json]` gives operators a redacted authority readiness
view before exercising privileged effects. It reports:

- receipt signer readiness, naming `RUNX_RECEIPT_SIGN_KID`,
  `RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64`, and
  `RUNX_RECEIPT_SIGN_ISSUER_TYPE`
- receipt verification readiness and whether it resolves from the explicit
  `RUNX_RECEIPT_VERIFY_*` pair or the complete signing identity
- whether runx Connect is configured, which hosted operations need to read
  the grant their connector instance was confirmed with

The diagnostic may show key ids and resolved filesystem paths. It must not show
signing seeds, public key material, provider scope values, grant ids, or
credential bodies.
