# runx-core

Pure Rust kernel for runx state-machine and policy decisions.

This crate owns the sequential graph state machine, scope-grant matching, and
authority derivation and minting, and checks them against the checked-in kernel
fixture set.

Scope names remain open, provider-neutral strings. `ScopeGrantPolicy` is the
single matching registry: exact-only for provider-native permissions,
delegated for one-segment `namespace:*` grants, and trusted for first-party
authority that may also carry the reserved universal `*` grant. Rust owns the
semantics; the published contracts package exposes the conformance-tested
TypeScript projection used by hosted surfaces.

`runx-core` must stay free of filesystem, network, subprocess, MCP, adapter,
and CLI presentation behavior.
