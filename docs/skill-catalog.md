# Skill catalog and discovery

The runx catalog has two jobs:

- help operators find a skill they can run now;
- help contributors avoid opening a duplicate skill PR or publishing a package
  into the wrong category.

The live registry is the source of truth for published community and verified
packages. This page is the maintainer-curated map: it explains how to search,
which category slugs are canonical, and what evidence a new skill proposal must
include before it belongs in the runx repo.

## Runx foundation and optional packages

The maintained foundation is `skill-build`, `skill-review`, `skill-adopt`,
`diagnose-skill-run`, `run-history`, `receipt-query`, `receipt-audit`,
`least-privilege`, `policy-draft`, and `sign-receipt`, alongside the root
`runx` orientation manual. Their complete packages ship inside Runx and work
offline without a registry download or source checkout. Each uses the ordinary
package, execution, authority, and receipt contracts. Bundling adds no privilege.

Users can load their own skills and connectors directly from local files.
For example, `runx skill ./skills/my-skill` loads a local skill; its plugin can
carry connector manifests in `connectors/<name>/connector.yaml` and their
declared tool packages. A connector names itself in its manifest `id`, such as
`acme/notes`; the name grants no publisher authority, which publication checks
separately against the namespace's owner. Local use requires no publication or registry account. Bundled,
local and registry sources share native validation and execution contracts.
`RUNX_SKILL_ROOTS` adds directories containing local skill packages, separated
with the platform's path separator; execution and export read the same roots.
For example, a package under `<root>/my-team/review` can be invoked or exported
as `my-team/review`. Multiple installed versions require an exact package path.
Export the shipped orientation manual with `runx export codex runx`, or a core
package with `runx export codex runx/receipt-query`. Both resolve from the
installed runtime without a checkout or registry download.
CLI outputs identify the resolved source in `entry_provenance`; bundled
packages identify `bundled:runx` without implying a registry trust tier.

Public distribution of optional domain procedures and product integrations
uses the registry. Their publisher follows the actual maintainer: Runx-maintained packages can
remain under `runx/`; product-owned packages publish from their owning product
repository. Being optional does not make a skill personally owned. A connector
retains its own publisher even when a differently owned skill composes it.

Source location, CLI distribution, and registry publication are separate facts.
Keeping an optional package in this repository for maintenance does not require
bundling it into the executable. Removing it from a distribution does not
publish it. A source transfer needs the authenticated publisher, complete
package and connector closure, updated callers, and exact-version readback
before the old maintained source is removed. Existing suspended runs retain
their original bound closure.

The names above describe the maintained source. Registry availability and the
CLI packaging cutover require separate verification; this list is not a claim
that those changes are deployed.

## Where to look first

Use the public catalog when you want to browse or share a package:

```text
https://runx.ai/x
https://runx.ai/x?category=security
https://runx.ai/x/<owner>/<skill>@<version>
```

Use the CLI when you are preparing work or checking for overlap:

```bash
runx registry search "cve audit" --json
runx skill inspect runx/cve-audit@sha-e11c90bbeb16 --json
runx registry install runx/cve-audit@sha-e11c90bbeb16
runx skill runx/cve-audit@sha-e11c90bbeb16 --json
```

The command shape is deliberate:

| Need | Command or URL |
| --- | --- |
| Browse the catalog | `https://runx.ai/x` |
| Browse one category | `https://runx.ai/x?category=<slug>` |
| Search from a terminal | `runx registry search <query>` |
| Inspect one package | `runx skill inspect <owner>/<name>@<version>` |
| Install package material | `runx registry install <owner>/<name>@<version>` |
| Execute a local or registry skill | `runx skill <path-or-ref>` |

Running a registry ref fetches it, so most work needs no install.
`runx registry install` adds a package without running it.

Every package a project runs lives beside its source, under
`skills/<owner>/<name>/<version>/`: a registry skill when it is first run or
installed, the registry skills a graph names, and a bundled runx skill the
first time the project uses it (`runx new` uses `runx/skill-build`). Commit
them or ignore them as you would any other dependency.

## Operator-first package contract

A public package has one memorable capability name and two supported invocation
contexts. Direct use selects the truthful default described by its exact
`SKILL.md`; a parent graph selects an exact named runner and passes typed prior
results. Runx binds both the injected manual bytes and executable closure by
digest, so a host cannot silently operate from instructions that disagree with
the installed package.

Defaults follow the public promise:

- an operation such as send, sync, spend, refund, publish, or unseal executes
  through its real boundary or returns one honest, resumable blocker;
- a skill whose name promises a plan, draft, brief, review, or route returns
  that typed artifact without pretending an external effect occurred;
- explicit `plan`, `inspect`, `apply`, and `verify` runners remain available for
  composed use and do not compete as separate public skills;
- normal output contains one compact domain result and receipt references;
  intermediate graph context is available only with diagnostics.

For GitHub, the selected `github` instance decides the account: a signed-in
local `gh` session (`runx connector use github --session`), a stored token, or
a runx Connect grant. A run never falls back to another account, and a
connector account never determines the repository target. Reads do not acquire approval ceremony. Routine remote
mutations use their exact scoped grant, stable idempotency, recovery, and
independent readback regardless of transport. A skill adds one exact resumable
human approval only when the action itself warrants that extra decision.

Changing an implicit default is a versioned behavior change, not an immediate
release. Callers that need plan-only behavior select the named `plan` runner;
unqualified invocation follows the advertised default. Publication and deployment remain separate
operator decisions.

Package admission and `core-skills:audit` require an executable cold-selection
journey with at least two real public confusors, a standalone journey through
the actual default runner, and a composed journey that reuses named prior
evidence without repeating completed work. The native semantic report owns
only those operator-readiness facts. The separate native execution closure is
the source of truth for resolved runners, tools, package bindings, and
unresolved edges; authoring and review compare catalog promises with that
closure instead of guessing effects from runner or tool names. Internal rails
and harness fixtures remain executable through explicit identities but are
excluded from normal discovery. Deterministic harness replay and separately
identified live-provider evidence remain distinct proof levels.

## Naming a skill

Name a skill so an agent instinctively reaches for the right one:

- Name the **job**, not the mechanism. Drop `-auditor`, `-generator`,
  `-analyst`, `-pipeline` suffixes.
- One distinct **verb per pipeline layer** so sibling skills never blur. The
  security trio is the model: `cve-audit` (detect, deterministic) ->
  `vuln-triage` (assess, agent); publishing the advisory is the operator's step.
- Keep provider rails and integrations descriptive (`web-fetch`, `nws-weather-forecast`).
- Retrieval beats cleverness: the searchable term wins, and keywords the name
  drops go in the `description`.

Renaming an existing skill is a `git mv`, a token sweep, and a regenerate
(`packet-schemas`, `runx-maintained-lock`) plus `verify:fast` and the harness. For a
maintained `runx/*` skill the hosted registry row is preserved (its
`created_at` and version history carry across) during the operator deploy, so a
rename never resets the row.

## Canonical category slugs

Registry categories are maintained by runx, not invented per package. A skill
author requests a category with `catalog.category` in `run.yaml`, but the hosted
registry owns the final facet shown on `runx.ai/x`.

| Slug | Label | Use for |
| --- | --- | --- |
| `code` | CODE | Software lifecycle: implement, review, release, refactor, and repo evolution. |
| `payments` | PAYMENTS | Money movement: charge, refund, settlement, dispute, and billing rails. |
| `data` | DATA | Pipelines, querying, enrichment, analytics, and extraction. |
| `research` | RESEARCH | Bounded investigation and synthesis: briefs, market and ecosystem analysis. |
| `content` | CONTENT | Creation and publishing: drafts, documentation, posts, and marketing copy. |
| `security` | SECURITY | Risk and trust: vulnerability scans, advisories, audits, and compliance. |
| `ops` | OPS | Running systems: deploy, monitor, incident, infra, triage, and intake. |
| `growth` | GROWTH | Go-to-market: sales, outreach, CRM, SEO, and campaigns. |
| `authoring` | AUTHORING | Skill building: design, harness, evaluation, improvement, and registry trust. |

`authoring` is a builder-surface category. It is reachable in the catalog and
API, but buyer-facing homepage rows can omit it to keep product discovery
focused on executable business domains.

## First-party skill map

The first-party map is intentionally smaller than the live catalog. It lists
skills the runx repo maintains directly, not every community package. Community
packages should be discovered through the live registry and promoted by
evidence, not copied into the repo by default.

| Category | Maintained packages |
| --- | --- |
| `authoring` | `skill-adopt`, `diagnose-skill-run`, `policy-draft`, `prior-art`, `skill-review`, `skill-build`, `work-plan` |
| `code` | `github-pr-review`, `release`, `schema-guard` |
| `content` | `brand-voice`, `content-pipeline`, `contract-drafter`, `ghostwrite`, `moltbook` |
| `data` | `product-analytics`, `run-history`, `sql-analyst` |
| `growth` | `cold-outreach`, `crm-cleanup`, `lead-enrichment`, `lead-qualify` |
| `ops` | `github-sync`, `issue-intake`, `issue-triage`, `message-send`, `ops-desk`, `postmortem-maker`, `webhook-handoff` |
| `payments` | `charge`, `dispute-respond`, `pay`, `purchase-approval`, `refund`, `settle-invoice` |
| `research` | `deep-research`, `research`, `web-search` |
| `security` | `cve-audit`, `sbom-maker`, `vuln-triage`, `data-subject-request`, `least-privilege`, `receipt-audit`, `redact-pii`, `sign-receipt` |

Graph stages, harness fixtures, context-only packages, and provider bindings are
not listed here unless they are meant to be run as catalog packages. Their
ownership lives with the parent skill or binding metadata.

## Duplicate check before opening a skill PR

Before opening a new first-party skill PR, do the overlap check in public:

1. Search the live catalog by name, category, and capability words.
2. Read the closest first-party and verified community packages.
3. Search open PRs and issues for the same category and runner shape.
4. Decide whether the work is a new skill, an improvement to an existing skill,
   a graph composition, or a community package that does not belong in the
   first-party repo.
5. Put that reasoning in the PR body.

Useful commands:

```bash
runx registry search "<capability>" --json
gh pr list --repo runxhq/runx --search "<capability> OR <category>" --state open
gh issue list --repo runxhq/runx --search "<capability> OR <category>" --state open
```

A strong proposal answers:

- What pain does this skill solve that the current catalog does not?
- Which existing packages did you compare against?
- Why is this a new first-party package rather than a community registry
  package, graph composition, or patch to an existing skill?
- What runner files, `SKILL.md`, `run.yaml`, fixtures, harness evidence, receipt,
  and public registry URL prove it works?
- What authority, network, filesystem, provider, or spend boundaries does the
  execution profile declare?

Do not bundle unrelated CLI, runtime, docs, or release changes into a skill PR.
A first-party skill PR should be narrow enough that a maintainer can review the
package, harness, and evidence without separating it from repo history.

## When a community package is enough

Most new skills should start outside the first-party repo:

```bash
runx new <skill-name> --objective "Describe the bounded operator outcome"
runx harness ./<skill-name> --json
runx login --for publish
runx registry publish ./<skill-name>/SKILL.md
```

That gives the package a durable URL, immutable digest, install path, and
publisher identity without making runx maintain it. Community packages can still
be used immediately:

```bash
runx skill <owner>/<skill>@<version> --json
```

Promote a community skill toward first-party only when it has clear adoption,
verified runs, a stable execution profile, and a maintainership reason that
belongs in runx rather than in the publisher's own repo.

## Discovery contract for maintainers

When reviewing a skill PR, maintainers should check:

- the requested `catalog.category` is one of the canonical slugs above;
- the public registry search does not already return a maintained equivalent;
- any overlap is explicitly explained in the PR body;
- the package contains only consumable skill material;
- `runx registry install` adds the same files that hosted publish validated;
- `runx skill` executes the package or fails closed with a useful receipt;
- public artifact URLs point to durable pages, not throwaway previews.

If a proposal is useful but not first-party, close the repo PR or issue with a
clear pointer to the live registry package and the evidence needed for future
promotion. That keeps the issue board clean without penalizing the contributor.
