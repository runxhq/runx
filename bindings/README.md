# Upstream Bindings

Bindings connect a verified upstream `SKILL.md` to a runx execution profile.

The upstream repository remains the source of truth for the portable skill
document. This directory stores runx-owned binding data:

- `binding.json`: upstream repo/path/commit provenance, trust tier,
  registry publisher, publication state, and proof pointers.
- `run.yaml`: the execution profile artifact containing runner metadata,
  harness cases, policy, scopes, and receipt expectations.

Use `bindings/` only when runx does **not** own the upstream `SKILL.md`.
First-party runx packages belong in `skills/<name>/`. Product-specific operator
wrappers belong in the product repo that owns their policy.

Do not add candidate or placeholder directories here. A binding is eligible only
after the upstream repository has a merged, pinned `SKILL.md` that starts with
YAML frontmatter and a `name` matching `bindings/<publisher>/<skill>`.

Overlay produces an exact, digest-bound `binding.json` and `run.yaml` bundle from
the pinned upstream manual. Apply that bundle through the repository's normal
authoring lane, then publish the validated package through the native registry
command. No generated `dist/` mirror is a source of truth.

Example:

```bash
runx skill ./skills/skill-adopt generate --json \
  -i skill_path=vendor/acme/SKILL.md \
  -i upstream='{"host":"github.com","owner":"acme","repo":"project","path":"SKILL.md","commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","blob_sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","source_of_truth":true,"html_url":"https://github.com/acme/project/blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/SKILL.md","raw_url":"https://raw.githubusercontent.com/acme/project/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/SKILL.md"}' \
  -i registry='{"publisher":"acme","trust_tier":"community","version":"upstream-aaaaaaaa"}'
```

If the run yields `needs_agent`, continue with the exact `runx resume` command
from the envelope. The resulting files remain a proposal until the owning
authoring lane applies them.

Validate the checked-in binding catalog:

```bash
pnpm bindings:check
```

## Candidate Queue

These are good candidates once their upstream repositories own the source
`SKILL.md`; they must not appear as binding directories before that happens.

- `nilstate/scafld-operator` — scafld governs agent work. Add this after
  `nilstate/scafld` owns a merged `SKILL.md` at a pinned commit.
- `sourcey/<project-operator>` — only if Sourcey moves the portable
  instructions into a Sourcey-owned upstream repo. If runx owns the skill,
  keep it under `skills/sourcey`.
- Real OSS project operator skills such as `contentauth/c2pa-rs`,
  `napi-rs`, `hono`, or `drizzle` — only after the project accepts or hosts
  its own `SKILL.md`.
