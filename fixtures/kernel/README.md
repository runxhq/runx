# Kernel Fixtures

These fixtures are regression evidence for `runx-core`, the trusted kernel.

Each fixture has:

- `name`: stable kebab-case identifier used as the filename.
- `input.kind`: public operation under test.
- `expected`: output or error discriminator.

Regenerate fixtures with:

```sh
pnpm fixtures:kernel:generate
```

Check committed fixtures without rewriting them:

```sh
pnpm fixtures:kernel:check
pnpm fixtures:kernel:validate
pnpm fixtures:kernel:keys
```

Fixture schemas intentionally use a small local JSON Schema subset while the
contract is being stabilized. Supported keywords are `$id`, `$schema`,
`additionalProperties`, `anyOf`, `const`, `items`, `oneOf`, `pattern`,
`properties`, `required`, and `type`; every other keyword is rejected so
schema changes fail closed.

The fixture envelope's `$schema` field is a local in-tree discriminator, not a
public JSON Schema meta-schema URI. Runners must accept only the canonical
relative refs under `fixtures/kernel/schema/` that the in-tree validator
recognizes.

All fixture JSON is sorted lexicographically at every object boundary. The
kernel uses deterministic serde boundary types such as `BTreeMap` for
object-keyed maps so fixture comparison is stable.

Deduplicated arrays preserve first-seen insertion order. The kernel uses
insertion-preserving deduplication for arrays such as `requestedScopes`, `grantedScopes`, `stepIds`, and `contextFrom`; do not use
`HashSet` or `BTreeSet` at serialized array boundaries.

Optional fields whose TypeScript value is `undefined` are omitted from the
serialized JSON. Rust serde fields for those values must use
`skip_serializing_if = "Option::is_none"`.

The Rust state-machine fixture runner lives in
`crates/runx-core/tests/state_machine_fixtures.rs`. It dispatches by
`input.kind`, deserializes into typed Rust structs/enums, serializes the result
back to JSON, and compares that JSON to the TypeScript-generated `expected`
value. Public Rust payload fields use `runx_contracts::JsonValue` and
deterministic `BTreeMap`-backed objects rather than public
`serde_json::Value`.

The Rust policy fixture runner lives in
`crates/runx-core/tests/policy_fixtures.rs`.
Current policy fixtures cover scope-grant matching, which the TypeScript
contracts mirror; state-machine fixtures cover the sequential graph.

Fixtures under `runner/` pin fixture-runner ingestion behavior rather than a
trusted-kernel decision. They exist to keep the cross-language fixture harness
fail-closed for malformed-but-schema-shaped inputs. Rust runners may reject
those cases during typed deserialization, as long as the fixture harness maps
the rejection to the same `kernel.fixture.evaluation_failed` envelope. Runner
fixtures are identified by `expected.kind === "error"` and
`expected.code === "kernel.fixture.evaluation_failed"`, must use the
`runner-` filename prefix, and pin both the error `code` and the literal
wrapper `message` (`"kernel fixture evaluation failed"`).
