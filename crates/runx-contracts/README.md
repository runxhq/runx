# runx-contracts

Shared Rust contract types at runx JSON and protocol boundaries.

This crate owns stable serde shapes and JSON Schema emission for runx JSON and
protocol boundaries. It does not execute skills, evaluate policy, perform IO,
or host runtime behavior.

The surface is contract-only:

- `json`: contract-owned JSON values backed by deterministic `BTreeMap`
  objects.
- `act`: governed act payloads.
- `receipt`: signed governed proof records.
- `authority`, `decision`, `verification`, and governance objects: spine
  contracts used at governed boundaries.
- `schema_artifacts`: the Rust-owned manifest that emits `oss/schemas/*.json`
  and the generated TypeScript schema artifact table.
- `schema_definitions`: publishes that set with each named type defined once,
  in its own document or in `definitions.schema.json`, and referred to with
  `$ref` everywhere else.

SDKs and TypeScript packages consume these schemas and generated artifacts;
they must not hand-maintain mirror schemas for Rust-owned contracts.

Workspace fixtures under `fixtures/contracts` are not vendored into this crate.
The Cargo package `include allowlist` ships only `Cargo.toml`, `README.md`, and
`src/**/*.rs`, so fixture files are excluded from the packaged crate.

New modules belong here only when they define a portable wire contract. Runtime
services, adapters, fixture methods, and presentation helpers belong in their
own crates.
