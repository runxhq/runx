# Upstream Binding Records

This directory contains checked-in upstream provenance and profile records.
A binding.json and run.yaml pair is not a complete runnable skill package:
it does not contain the upstream manual or its required resources. These
records do not constitute registry publication or an installation.

For a new adoption, use skill-adopt with captured local source and a deliberate
package destination, or its from-source runner with an existing native source
snapshot. It passes an immutable artifact to canonical skill-build, which
plans, validates, safely replays and atomically applies the complete package.
The upstream manual, licenses and necessary resources remain part of that
source context. Only the actual apply_result establishes local application.

First-party packages use skill-build directly. Publication uses the canonical
registry operator after the complete package is ready, with independent
identity, version and digest readback. No metadata-only binding generator or
hidden materialization step is part of adoption.

The existing record catalog is checked with pnpm bindings:check.

## Candidate Queue

These are good candidates once their upstream repositories own the source
`SKILL.md`; they must not appear as binding directories before that happens.

- `auscaster/scafld-operator` — scafld governs agent work. Add this after
  `auscaster/scafld` owns a merged `SKILL.md` at a pinned commit.
- `sourcey/<project-operator>` — only if Sourcey moves the portable
  instructions into a Sourcey-owned upstream repo. If runx owns the skill,
  keep it under `skills/sourcey`.
- Real OSS project operator skills such as `contentauth/c2pa-rs`,
  `napi-rs`, `hono`, or `drizzle` — only after the project accepts or hosts
  its own `SKILL.md`.
