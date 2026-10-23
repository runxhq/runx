# Orchestrator Directory Listings

The orchestrator integration goal is distribution, not only connectivity. The
strong story is orchestrator-to-orchestrator handoff: runx is the governed execution orchestrator
for authority, secrets, policy, runtime, and receipts;
n8n/Zapier/Make remain workflow surfaces for triggers, canvases, schedules, and
cross-app branching.

For long-running agent systems, keep the same boundary described in
[Loop Orchestration](./loop-orchestration.md): n8n, Zapier, Make, Temporal,
LangGraph, or a hosted app may own the loop, while each consequential turn is a
normal governed runx skill/graph run with its own receipt.

- a runx listing on n8n's public integrations surface
- a runx app page in Zapier's public App Directory
- follow-on listings in adjacent automation, connector, CI, and MCP registries
- backlinks from those pages to runx-owned landing and support pages

Self-hosted n8n command nodes and webhook templates are useful dogfood, but they
do not earn those listings. A public listing needs an actual package/app that the
orchestrator can review and expose to users.

## Local CLI Operator Contract

The local CLI is the reference implementation for self-hosted orchestrators and
operator dogfood. It should feel direct, literal, and governed:

```bash
runx skill nws-weather forecast \
  -i office=LWX \
  -i grid_x=97 \
  -i grid_y=71
```

Operator rules:

- Bare local skill names resolve from the current workspace's `skills/`
  directory.
- `-i name=value` gives one input, read by the type its runner declares: a
  string as written, any other type as JSON. `--inputs file|-` gives the whole
  input object instead. A flag runx does not own is refused, never read as an
  input.
- `runx skill <skill> <runner>` selects a non-default runner without changing the skill
  package.
- `--json` prints the full machine contract. Without `--json`, the CLI prints a
  concise status view with run id, receipt id, and pending request ids rather
  than dumping large provider payloads.
- Exported Claude/Codex skills are shims. If invoked directly by path, the CLI
  resolves the generated source marker back to the governed runx skill; stale
  shims fail closed with an instruction to rerun `runx export`.
- Runnable registry skills use explicit `publisher/name@version` refs with optional
  `--registry`. A bare name resolves a local path, then an installed or
  workspace skill, then a bundled skill, then the registry's root name: one
  segment the registry assigns to a verified publisher's skill, resolved to its
  canonical `publisher/name`. A package's `registry:` refs always name `publisher/name`.
  Unsigned or digest-mismatched registry packages are search/read metadata only
  and fail before execution.
- `runx registry search` and `runx registry install <ref>` print a compact
  human view that names the selected source, skill id, version, digest, trust
  tier, signature key id when present, and destination or next action, and
  `runx skill inspect <ref>` shows one package. Use `--json` for the full
  registry contract.
- `runx doctor registry [--json]` reports the selected registry target, the
  project's skills directory and added skills, trusted manifest key readiness by key id, and
  remote install identity readiness. It names the env
  vars to set but never prints raw manifest public keys.

## Hosted Connector Contract

Cloud orchestrator packages should call the hosted API, not shell out:

- `POST /v1/skills/{skill}/run` is the connector-friendly `Run Skill` action.
  The `skill` path parameter is the skill reference; use a URL-encoded slash for
  `publisher/name` refs. The JSON body contains `inputs` and optional
  `idempotency_key`.
- `POST /v1/skills/{publisher}/{name}/run` is the clean publisher-scoped route for
  registry skills.
- `POST /v1/runs` remains the canonical hosted submission route when the caller
  prefers body-level `skill`.
- `GET /v1/runs/{id}` and receipt lookup are the poll/inspect surfaces returned
  to users and workflow branches.
- Public connector credentials should be scoped: `runs:write` to submit/rerun
  and resolve hosted work, `runs:read` to list/inspect/poll runs,
  `receipts:read` to retrieve receipts, `receipts:write` for trusted receipt
  ingest, and `signals:write` for trusted signal ingest. A typical n8n/Zapier
  v1 credential should start with only `runs:write`, `runs:read`, and
  `receipts:read`.
- Directory clients should call real runx skills, not privileged special-case
  routes. The local outbound skill is `webhook-handoff`; the
  hosted n8n/Zapier clients should submit the same kind of governed skill run
  through `POST /v1/skills/{publisher}/{name}/run`.
- Do not add a new durable packet family just for orchestrator handoff. The
  handoff skills emit a receipt-backed `handoff_context` artifact and the
  outbound HTTP step supplies effect evidence. If receiver replies or
  post-handoff state need modelling later, the skill that tracks them owns
  that packet; core carries no handoff lifecycle packet.

The remaining directory blocker is production posture: deployed HTTPS,
reviewable credentials/test accounts, docs, support pages, and a conservative
public v1 skill policy.

## Target Surfaces

### n8n

Target: n8n's public integration library, especially the partner-built/verified
community node surface at `https://n8n.io/integrations/partner-built/`.

The practical route is a verified community node package, not local command
wiring. Current n8n docs require community node packages to:

- use a package name beginning with `n8n-nodes-` or scoped as
  `@<scope>/n8n-nodes-`
- include the `n8n-community-node-package` npm keyword
- declare nodes and credentials in the package `n8n` attribute
- pass lint/local tests
- publish to npm

For verification, n8n currently requires GitHub Actions publishing with npm
provenance. n8n also says verified community nodes must follow technical and UX
guidelines, have proper README/docs, and must not use runtime dependencies.

Proposed package:

- `@runxhq/n8n-nodes-runx`
- Node name: `Runx`
- Credential: `Runx API`
- Initial operation: `Run Skill`, with `runx/webhook-handoff` as the canonical
  self-referential dogfood skill once hosted registry publication is ready.
- Secondary operation after receipts API exists: `Get Receipt`
- Backlink target: a stable runx-owned n8n integration page, not a GitHub file

Status: clean GitHub repo name reserved at
`https://github.com/runxhq/n8n-nodes-runx`. No npm package should be published
until the hosted API is deployed with stable credentials, docs, and a reviewable
test account.

Real blocker: a verified n8n Cloud-usable node needs the production HTTPS runx
API, not just source-level routes. The local CLI/MCP path cannot be the verified
listing path because n8n Cloud cannot run a local shell or reach localhost.

## Zapier

Target: a public runx app in Zapier's App Directory.

Zapier distinguishes private integrations from public integrations. Public
integrations can be published in the App Directory, join the Partner Program, and
expose Zap templates. Public publishing currently requires:

- app/API ownership or permission proof
- production HTTPS endpoints
- secure credential handling through Zapier authentication configuration
- a publicly launched production app, not a beta or sandbox-only service
- documented APIs
- successful enabled test Zaps with Zap history available for review
- listing name/description/homepage/logo that follow Zapier conventions
- an admin team member using the app/API domain
- a non-expiring test account for `integration-testing@zapier.com`
- passing Zapier validation checks and publishing tasks

Zapier's publishing requirements prohibit integrations that facilitate financial
transactions, transfer assets, or process payments. Public runx v1 on Zapier
must therefore exclude payment, token-transfer, and settlement actions even if
runx can govern those skills elsewhere.

Proposed public Zapier v1:

- App name: `runx`
- Authentication: API key/OAuth against hosted runx
- Action: `Run Skill` for non-payment skills only, with `runx/webhook-handoff`
  as the canonical self-referential dogfood skill once hosted registry
  publication is ready.
- Action: `Get Receipt`
- Search: `Find Run`
- No trigger in v1 unless a production webhook/resume surface exists and passes
  Zapier's public-trigger constraints
- Backlink target: runx marketing homepage plus a stable Zapier integration
  support page

Real blocker: Zapier public listing requires a production HTTPS runx API and
reviewable test account. Webhook templates alone do not qualify.

## Other Registries We Overlooked

n8n and Zapier are still the first backlink targets because they map cleanly to
the product story: no-code workflow triggers call a governed runx step. The
next surfaces should be prioritized by whether they create an indexed public
listing and whether they can reuse the same hosted run-skill and receipt APIs.

### Priority 0: Same Buyer, Same API

These should sit directly behind n8n and Zapier once the hosted API is deployed
and ready for third-party review.

**Make**

Target: Make's public integrations surface and community/approved app path.

Make says community apps can be public, appear with other public apps on the
Integrations page, and can receive a Make landing page with a link to the
partner's site. A public app can be shared by invite link; an approved app is
available to all Make users after review.

Proposed Make v1:

- App name: `runx`
- Module: `Run Skill`
- Module: `Get Receipt`
- Optional later module: `Resume Run` only after external resume exists
- Backlink target: `https://runx.ai/integrations/make`

Real blocker: Make cloud cannot call localhost. The Make app is gated on the
same production HTTPS run-skill API as Zapier.

**Pipedream**

Target: Pipedream's Marketplace and source-available component registry.

Pipedream verified components are sources and actions curated through a GitHub
PR process; registered components appear in Pipedream's Marketplace and in the
workflow builder UI. Private or non-conforming components do not earn the same
registry value.

Proposed Pipedream v1:

- App integration: `runx`
- Action: `Run Skill`
- Action: `Get Receipt`
- Source later: `New Run Completed` after webhook/resume/event delivery exists
- Backlink target: `https://runx.ai/integrations/pipedream`

Real blocker: same as Zapier and Make: deployed production HTTPS API, stable
auth, and reviewable component behavior.

**Microsoft Power Platform**

Target: certified connector pages across Power Automate, Power Apps, Logic
Apps, and Copilot Studio.

Microsoft connector certification makes a custom connector publicly available,
adds it to official connector documentation, and gives each connector its own
public page. This is higher effort than n8n/Zapier, but it is likely the
strongest enterprise directory surface.

Proposed Power Platform v1:

- Connector name: `runx`
- Action: `Run Skill`
- Action: `Get Receipt`
- No payment/asset-transfer action in public v1
- Backlink target: `https://runx.ai/integrations/power-automate`

Real blocker: certification needs a production API, stable OpenAPI/connector
definition, publisher identity, support process, and Microsoft review.

### Priority 1: Developer And OSS Distribution

These are useful backlinks and developer discovery surfaces, but they should not
pull product/API work ahead of the hosted run-skill seam.

**Node-RED Flow Library**

Target: `flows.nodered.org`.

Node-RED nodes are npm packages and must be manually submitted to the Flow
Library; publishing to npm with the old keyword path is not enough by itself.

Proposed package:

- `@runxhq/node-red-runx`
- Node: `runx skill`
- Node: `runx receipt`
- Backlink target: `https://runx.ai/integrations/node-red`

This can support self-hosted/local runx better than cloud-only registries, but a
public package still needs clear credential handling and docs.

**Activepieces**

Target: Activepieces pieces ecosystem.

Activepieces supports publishing pieces by contributing back to the main
repository, publishing a community npm package, or publishing privately. Pieces
are TypeScript packages and can expose actions and triggers.

Proposed piece:

- Package: `@activepieces/piece-runx`
- Action: `Run Skill`
- Action: `Get Receipt`
- Trigger later: `Run Completed`
- Backlink target: `https://runx.ai/integrations/activepieces`

Real blocker: a cloud-usable community piece still needs the hosted runx API.

**GitHub Actions Marketplace**

Target: GitHub Actions Marketplace.

This is CI distribution rather than workflow-orchestrator distribution, but it
is a low-friction backlink surface for developers. GitHub requires a public
repository, a single root `action.yml` or `action.yaml`, and a unique action
metadata `name`.

Proposed action:

- Repository: `runxhq/runx-action`
- Action name: `runx`
- Operation: run a governed skill in CI and upload/print receipt metadata
- Backlink target: `https://runx.ai/integrations/github-actions`

Status: public repository and `v0.1.0` release exist at
`https://github.com/runxhq/runx-action`. Final Marketplace publication still
requires the GitHub release UI checkbox and any required Marketplace Developer
Agreement acceptance.

This can start as a CLI wrapper before hosted APIs are complete, but the public
copy must be explicit that cloud orchestrator use is still hosted-API gated.

**Official MCP Registry**

Target: `registry.modelcontextprotocol.io`.

The official MCP Registry hosts metadata, not artifacts. A server package must
be published elsewhere first, then described with `server.json` and published
with `mcp-publisher`.

Proposed MCP listing:

- Package/server name: `io.github.runxhq/runx`
- Artifact: npm, PyPI, Docker, or a hosted MCP server depending on the packaging
  decision
- Backlink target: `https://runx.ai/integrations/mcp`

This is not an n8n/Zapier replacement. It is agent-tool discovery and should be
listed separately from workflow automation directories.

### Priority 2: Enterprise Or Fit-Dependent Surfaces

These are worth tracking, but only after the smaller public app/package surfaces
prove demand.

- **Workato**: community connectors and partner connectors can surface in
  Workato's connector directory/community library. Strong enterprise audience,
  but partner-led and higher support burden.
- **IFTTT**: services get dedicated IFTTT service pages and Applets, but the fit
  is weaker unless runx has consumer/IoT-style triggers and at least a dozen
  useful Applets ready for review.
- **Tray.ai**: custom connectors can be built and published/reviewed, but the
  public backlink route is less direct than Make, Pipedream, or Power Platform.
- **UiPath Marketplace**: useful if runx becomes an RPA/governed-automation
  control point. Connector Builder has a publish-to-marketplace flow, but this
  should follow enterprise demand.
- **MuleSoft Anypoint Exchange**: relevant for API/enterprise integration and
  now agent/MCP asset discovery, but mostly valuable when runx has enterprise
  customers asking for Anypoint assets.

## Backlink Pack

Before submitting listings, runx needs stable public pages:

- `https://runx.ai/integrations/n8n`
- `https://runx.ai/integrations/zapier`
- `https://runx.ai/integrations/make`
- `https://runx.ai/integrations/pipedream`
- `https://runx.ai/integrations/power-automate`
- `https://runx.ai/integrations/node-red`
- `https://runx.ai/integrations/activepieces`
- `https://runx.ai/integrations/github-actions`
- `https://runx.ai/integrations/mcp`
- `https://runx.ai/docs/orchestrators`
- `https://runx.ai/security`
- `https://runx.ai/support`

These pages should be seeded alongside the existing provider-catalog integration
pages, not bolted on as a second website section. The cloud site already has a
public integrations catalog fed by the generated provider snapshot. Keep that
snapshot as the long-tail provider list and add runx-owned orchestration pages as
an explicit custom overlay:

- custom overlay leads the integration wall by `featured_rank`
- provider-catalog entries remain searchable and listed
- custom pages upsert by slug when a provider entry already exists, so
  `/integrations/zapier`, `/integrations/make`, and `/integrations/pipedream`
  can explain runx handoff rather than generic provider authentication
- planned directory clients render as `catalog` until the package/app/listing is
  actually public; only local or CI surfaces that work today can render as
  `byo-ready`
- `runx connector use` is reserved for provider credentials and grants. It is
  not the way a workflow orchestrator connects to runx. n8n/Zapier/Make/Pipedream
  use scoped runx API credentials to call hosted run-skill endpoints; runx then
  uses provider grants internally. Custom directory slugs are reserved from
  provider connect routing, so a `zapier` connector is never a second way for
  Zapier to reach runx.

The pages should explain:

- governed skill execution
- signed receipts
- policy and secret ownership
- non-payment limitation for Zapier public v1
- support contact and status page
- API docs for hosted run-skill and receipt lookup once those APIs exist

## Outstanding External Setup

The repo can seed pages, package code, and tests. These steps still require
account access, production deployment, or third-party review.

**n8n**

- Own the npm package namespace for `@runxhq/n8n-nodes-runx`.
- Configure npm Trusted Publisher for the GitHub Actions `publish.yml` workflow.
- Publish from GitHub Actions with provenance; n8n requires provenance for
  verification submissions.
- Keep the package compliant with community-node metadata, no-runtime-dependency
  verified-node constraints, UX guidelines, and README/support expectations.
- Submit through n8n Creator Portal only after the production hosted runx API,
  test credentials, support URL, and integration page exist.

**Zapier**

- Create the Zapier Platform app privately first.
- Complete ownership, branding, homepage, logo, intended audience, role, and
  category setup in Zapier.
- Configure authentication as scoped runx API credentials. Do not ask Zapier
  users for downstream provider secrets.
- Ship only review-safe v1 actions: `Run Skill`, `Get Run`, `Get Receipt`.
  Payment, asset transfer, and pause/resume workflows stay out of public v1.
- Create test Zaps and a reviewable test account against production HTTPS.
- Pass Zapier validation and publishing tasks before App Directory submission.

**Make**

- Build the custom app modules and connection on the hosted API.
- Remove test modules/connections before publishing because public app shape is
  hard to undo after review.
- Request review only after support docs, production API, and modules are stable.

**Pipedream**

- Build a component directory with app metadata, actions, README, and registry
  versioning.
- Publish or submit via the Pipedream component workflow; actions should wrap
  the same `Run Skill` and receipt lookup semantics.

**GitHub Actions and MCP**

- Complete GitHub Marketplace publication UI/agreement for `runxhq/runx-action`.
- Pick the MCP artifact shape before registry submission. The MCP Registry
  publishes metadata; the package/server artifact must already exist.

## Listing Copy

n8n short description:

> Hand off n8n workflow steps to runx for governed skill execution and signed
> receipts.

Zapier app description:

> runx is a governed execution orchestrator for agent and automation work. It
> runs skills under policy, keeps sensitive provider credentials out of zaps,
> and returns signed receipts for audit and replay.

Avoid claims that n8n or Zapier endorse runx before approval. Avoid saying runx
is listed, verified, public, or available in either directory until the listing
is live.

## What The Local Work Is For

The existing local n8n guidance remains useful as dogfood:

- self-hosted n8n can call `runx skill ... --json`
- self-hosted n8n can consume local MCP HTTP on loopback
- runx can call n8n/Zapier-style webhook URLs as outbound effects through the
  `webhook-handoff` skill

That work proves workflow value and receipt shape. It is not the backlink path.

## Execution Order

1. Build stable runx integration landing/support pages.
2. Build hosted non-pausing run-skill and receipt lookup APIs.
3. Build `@runxhq/n8n-nodes-runx` using n8n's node tooling and publish with
   GitHub Actions provenance.
4. Submit the n8n package for verification through the Creator Portal.
5. Build a private Zapier integration against production HTTPS APIs.
6. Run validation, turn on test Zaps, prepare test account, and submit for
   public Zapier App Directory review.
7. Build Make and Pipedream clients on the same hosted API.
8. Build Node-RED, Activepieces, GitHub Actions, and MCP packages as developer
   distribution surfaces.
9. Evaluate Power Platform certification, Workato, IFTTT, Tray.ai, UiPath, and
   MuleSoft after the smaller public listings prove demand.
10. Add templates, embedded links, and co-marketing copy only after each listing
    is approved or live.

## Source Links

- n8n submit community nodes:
  `https://docs.n8n.io/integrations/creating-nodes/deploy/submit-community-nodes/`
- n8n verification guidelines:
  `https://docs.n8n.io/integrations/creating-nodes/build/reference/verification-guidelines/`
- n8n partner-built integrations:
  `https://n8n.io/integrations/partner-built/`
- Zapier publishing requirements:
  `https://docs.zapier.com/integrations/publish/integration-publishing-requirements`
- Zapier private vs public integrations:
  `https://docs.zapier.com/integrations/quickstart/private-vs-public-integrations`
- Zapier integration checks:
  `https://docs.zapier.com/integrations/publish/integration-checks-reference`
- Make community apps FAQ:
  `https://developers.make.com/custom-apps-documentation/community-apps/how-does-it-work`
- Make app visibility:
  `https://developers.make.com/custom-apps-documentation/create-your-first-app/app-visibility`
- Pipedream components:
  `https://pipedream.com/docs/components`
- Microsoft connector certification:
  `https://learn.microsoft.com/en-us/connectors/custom-connectors/submit-certification`
- Node-RED packaging and Flow Library submission:
  `https://nodered.org/docs/creating-nodes/packaging`
- Activepieces sharing pieces:
  `https://www.activepieces.com/docs/build-pieces/sharing-pieces/overview`
- Activepieces publish custom pieces:
  `https://www.activepieces.com/docs/build-pieces/misc/publish-piece`
- GitHub Actions Marketplace publishing:
  `https://docs.github.com/en/actions/how-tos/create-and-publish-actions/publish-in-github-marketplace`
- MCP Registry quickstart:
  `https://modelcontextprotocol.io/registry/quickstart`
- Workato community connectors:
  `https://docs.workato.com/developing-connectors/community/community`
- IFTTT build your integration:
  `https://ifttt.com/docs`
