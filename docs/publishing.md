# Publishing a skill to the runx registry

A runx skill is portable on its own, but to make it discoverable and installable
by others it has to be **published to a registry**. There are two registries, and
they behave differently.

## The two registries

- **Local workspace registry.** Lives under your workspace (`.runx`). No
  credentials. Use it to test the publish path and to resolve a skill locally
  before sharing it.
- **The hosted runx registry** (`runx.ai/x`). The shared, public, signed catalog.
  Publishing here is an authenticated write under *your* publisher identity, so it
  requires a publish credential.

## Before you publish

The skill must be real and runnable:

- A valid `SKILL.md` (frontmatter `name`, `description`, and `source`) and an
  execution profile (`X.yaml`) when the skill has a runnable path. Format:
  https://runx.ai/SKILL.md
  Use `runx.category` for the runx catalog category/facet; runx categories are
  maintained slugs, not arbitrary tags. See
  [skill-catalog.md](skill-catalog.md) before choosing one.
- It passes the harness:
  ```bash
  runx harness ./skills/<your-skill> --json
  ```

`X.yaml` is the canonical v1 filename, but the artifact is an execution profile:
runner wiring, typed inputs and outputs, tool/context refs, authority and receipt
mapping, side-effect posture, and harness cases. Do not use it as a strategy
document, target registry, copy deck, generated state file, or secret container.
Keep profile YAML explicit: anchors, aliases, merge keys, custom tags,
multi-document markers, duplicate mapping keys, and unknown profile fields are
not supported.

## Publish locally first

```bash
runx registry publish ./skills/<your-skill>/SKILL.md
```

This writes the skill into your local workspace registry. It takes no credentials
and is the fast way to confirm the package resolves and installs before you push
it to the shared catalog.

## Publish to the hosted registry

The hosted registry is open by default and conservative in discovery. A new
public package starts as a community row, is reachable by URL and exact search,
and earns broader ranking through identity, harness evidence, and hosted
verified runs.

For humans, start at https://runx.ai/x/publish. There are two publish lanes:

- **Public URL publish.** Paste a public repository URL. runx indexes every valid
  `SKILL.md`/`X.yaml` package it finds and leaves the source in the upstream repo.
  This is the fastest way to get a community listing into the catalog.
- **Authenticated CLI publish.** Use this for publisher-controlled releases. Sign
  in once, then publish the package directly from your local checkout. Hosted
  runx derives the owner from your connected identity, reconstructs the submitted
  package, reruns the publish harness, rejects failed cases, and starts the row
  at community trust.

The CLI form keeps the public API token out of command lines:

```bash
runx login --for publish
runx registry publish ./skills/<your-skill>/SKILL.md --registry https://api.runx.ai
```

If the browser consent step is unavailable, pipe a token from the provider
instead, for example from GitHub CLI:

```bash
gh auth token | runx login --provider github --for publish --token-stdin
```

This verifies the provider identity once and stores only the resulting
purpose-scoped runx credential. It does not store the provider token.

For remote publishes the CLI sends a bounded skill package:

- `SKILL.md` is the portable skill contract and is sent as the primary document.
- `X.yaml`, when present, is the execution profile and is sent as the profile
  document.
- Package files are the exact parser-admitted material for the package:
  - process entrypoints and their complete static local source closure;
  - deterministic modules reachable from execution profiles;
  - nested `SKILL.md` / `X.yaml` files for graph stages, context skills, and
    local sub-skills called by the graph;
  - harness-only support files explicitly declared in `harness.files`, confined
    to the owning profile's `fixtures/` directory and staged profile-relative
    into the isolated harness workspace;
  - `references/**/*.md` advisory markdown;
  - bundled tool manifests and their complete static local source closure.

Publishers that need the exact artifact without writing a registry row use the
same native materializer:

```bash
runx registry package ./skills/<your-skill> --json
```

The command is read-only. Its `markdown`, `profile_document`, and
`package_files` output is the sole portable package shape used by hosted
tooling; hosted code must not rediscover those files with a parallel crawler.

Nothing else is package material. Undeclared fixtures, build output,
`node_modules`, assets, dotfiles, local registry state, repo metadata, and
random helper files are not uploaded. A `src/` directory is not special: only
files proven reachable from an admitted entrypoint are included. Secret-looking
admitted file names such as `.env`,
`.npmrc`, credentials JSON, private keys, and certificate/key bundles still fail
the publish before any remote upload.

This lets hosted runx rerun the harness from the same consumed skill material
instead of trusting a client-supplied summary, while keeping local credentials,
fixtures, source trees, and build trash out of the registry. The local harness
still runs first for fast feedback.

`runx login --for publish` opens the hosted sign-in flow. Add `--provider` and
`--token-stdin` to sign in with a provider token instead of the browser consent
step. Both paths store
a purpose-scoped public API token in the encrypted local config at
`public.publish_api_token`. The token can publish and report skills, but it
cannot move money, mutate hosted billing state, or operate unrelated hosted
surfaces. Hosted CLI commands take the token from `RUNX_PUBLIC_API_TOKEN`, else
the stored token from `runx login --for publish`; no command takes a token as an
argument, so it never appears in a process listing. The default operator credential is stored separately;
signing in for publication cannot replace or attenuate it.

After a public URL publish, use the claim flow from the registry listing to prove
control of the source repo and move matching versions toward verified discovery.

### Why sign in?

Publishing writes a signed package into a shared public catalog under your
publisher identity. That is an authenticated write to an external authority, so
runx treats it like every other governed action, with no special-casing:

- The **connected identity** proves the publisher namespace. Hosted runx derives
  the owner from that identity; the request body cannot spoof it.
- The **public API token is stored encrypted locally** and masked by `runx config`.
  Use `runx login --for publish` for human publishing, or
  `RUNX_PUBLIC_API_TOKEN` for CI when you intentionally inject the same narrow
  credential.
- New publishes start as **community**. Verification and evidence promote
  discovery; publisher declaration alone never does.
- Hosted publishing is rate-limited per publisher identity. A noisy publisher
  cannot churn public versions indefinitely, even though the on-ramp stays open.
- Hosted publishing reruns the submitted package harness. Failed harness cases
  stop the write before a registry row is created.
- The registry row stores immutable `digest`, `profile_digest`, and
  `package_digest` values for the published package. `runx add` installs the
  same allowlisted package files that hosted runx validated. Signed run receipts
  and hosted verified-run evidence are separate signals recorded when the skill
  is executed.
- When a contributor-owned package is accepted into the Runx repository,
  `registry_owner` in `SKILL.md` frontmatter preserves that contributor's
  namespace for later hardened versions. Repository inclusion does not silently
  transfer the registry row to `runx/*`; such a transfer must be explicit.

## After you publish

- Your skill appears as a live registry row on your publisher profile at
  `runx.ai/x/<publisher>`.
- New publishes start at **community** trust tier. Promotion to **verified**
  requires claimed source identity, passing harnesses, and signing evidence
  (shown on the publisher trust panel).
- Community rows with no install or run evidence stay out of broad discovery by
  default, but they remain reachable by direct URL and exact-name search. Use
  `?include=unverified` when intentionally browsing the full frontier.
- `first_party`, `verified`, and established `community` rows appear in normal
  discovery; ranking prefers hosted verified-run evidence first, then trust tier,
  install evidence, and recency. A verified run is counted only from a sealed
  hosted-issued receipt that carries runx-written registry metadata for the
  skill/version. Publisher self-runs are excluded when the publisher id is known.
- Confirm it resolves:
  ```bash
  runx registry search <your-skill>
  runx registry read <publisher>/<skill>@<version> --json
  runx add <publisher>/<skill>@<version>  # the friendly install path
  ```

Before opening a first-party skill PR, check the live catalog and the
maintainer-curated first-party map in [skill-catalog.md](skill-catalog.md).
Most new skills should start as community registry packages; first-party
inclusion needs a clear maintainership reason and a narrow PR with no unrelated
repo history.

## Links

- Skill format: https://runx.ai/SKILL.md
- Catalog: https://runx.ai/x
- Publish surface: https://runx.ai/x/publish
- Catalog and discovery guide: ./skill-catalog.md
- Quickstart: https://runx.ai/docs/quickstart
