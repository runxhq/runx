# @runxhq/contracts

Published TypeScript package for runx machine-facing JSON contracts.

After the Rust takeover this package remains the TypeScript view of
`runx-contracts`. Contract drift is controlled through fixture
cross-validation, and consumers should treat this package as the stable
TypeScript import surface for host protocol and other public wire shapes.

It also exports the pure Runx scope-grant policy projection used by hosted
TypeScript consumers. Rust remains authoritative; generated kernel fixtures
fail the TypeScript test wall if exact, delegated, or trusted wildcard
semantics drift.

See the [TypeScript interop boundary](../../docs/ts-interop-boundary.md) for
the package outcome and ownership rules.
