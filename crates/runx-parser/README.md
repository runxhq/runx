# runx-parser

Canonical pure parser and validation crate for Runx package boundaries. Every
runtime consumer receives its package model from this crate; JavaScript test
and generator code may call the native parser but does not reimplement it.

The implementation covers:

- execution graphs
- skill markdown frontmatter and body preservation
- method manifests and test cases
- tool manifests from YAML and JSON
- skill install envelopes
- aggregate validated skill packages, including manuals, executable bundles,
  references, digests, and test definitions

The crate intentionally stays pure: it parses and validates typed intermediate
representations, uses `runx_contracts::JsonValue` and the
`runx_contracts::execution` semantic types at public parser boundaries, reuses
pure execution-requirement validation, and has no filesystem,
environment, network, or provider SDK dependencies.

`scripts/generate-rust-parser-fixtures.ts` batches fixture documents through
the native parser and records its owned output. Rust tests assert byte-level
stability against `fixtures/parser/**`; the script is transport and artifact
generation, not a second parser.
