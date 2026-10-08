# runx-receipts

Pure Rust receipt verification for runx.

The receipt model treats a receipt as the sealed proof of a
harness node. This crate validates that shape against the shared
`runx-contracts` types, provides deterministic canonical JSON and digest
helpers, and keeps verification pure. It does not write receipt files or invoke
runtime adapters.

Current verification covers structural invariants and strict proof checks:
terminal harness seal presence, top-level seal mirroring, form-specific act
payloads, decision and seal criterion references, child receipt
references, authority attenuation proof presence, supplied child receipt
resolution, `sha256:` hash commitments, deterministic body commitments, and
injected signature verification through `SignatureVerifier`.

The crate remains IO-free. Persistent child receipt lookup, local receipt store
discovery, and full authority algebra verification are runtime integrations.
