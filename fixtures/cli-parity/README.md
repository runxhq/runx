# CLI Feature Parity Matrix

Rust's `CommandSpec` catalog is the sole source of command syntax and help.
`tests/cli-feature-parity-contract.ts` reads that catalog directly from
`runx --help --json`, then binds test-only effect, surface, and oracle
annotations in memory.

## Fixture

The one executed runtime fixture is a test of the echo fixture package,
`fixtures/skills/echo/tests/cli-parity-echo-skill.yaml`, since a test lives in
the package it tests. Command metadata and derived parity cases are not
checked in, so they cannot drift from the native catalog.

## Parity Rules

- JSON output and receipt behavior are schema-exact.
- Executable cases assert the exact exit code they exercise.
- Human output is semantic and may be normalized for timestamps, paths,
  receipt ids, and platform-specific wording.
- Live providers are replaced by deterministic mocks, fixtures, or local
  protocol servers.
- Native CLI candidates must pass this matrix before packaging.
