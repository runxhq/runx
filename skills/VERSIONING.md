# Skill Versioning

Every skill in this directory declares a `version:` field at the top
level of its `run.yaml`. The field is semver.

## When to bump

Bump the version in the same commit as the change.

- **Patch (`0.1.X`):** SKILL.md prompt tweaks, harness-case additions,
  harness-fixture tightening, doc-only edits.
- **Minor (`0.X.0`):** new runner, new input field, new output field,
  new harness-case shape, backward-compatible output
  extension.
- **Major (`X.0.0`):** graph redefinition, runner renaming, input or
  output removal, any change that would break an existing caller's
  invocation shape.

## Starting point

Skills that existed before this convention landed are initialised at
`0.1.0`. Bump from there.

## Why

Dogfood cycle 1 surfaced that skills had no version field, so the
`version_bump_on_change` invariant of the dogfood campaign fell back
to the `oss/` submodule git sha. Sha-based tracking couples every
skill's evolution to the monorepo commit pointer — fine for audit,
useless for callers that need to know "does my pin of this skill
include the pass-verdict harness case".

With an explicit `version:` field, callers can pin, detect drift,
and reason about upstream changes without reading commit logs.

## Tooling

No CI enforcement yet. The dogfood campaign's `version_bump_on_change`
invariant asks cycles to bump manually; future tooling may automate
the bump check.
