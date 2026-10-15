# Operator Skills Index

This page lists every operator-style skill that ships under `skills/`, grouped by lane, so new contributors can discover what already exists without grepping the directory tree.

For the project-skill **pattern** (how to author a new one, the dogfood contract, and the "Add A Project Operator Skill Only When" decision rule), see [docs/operator-skills.md](./operator-skills.md). For the registry search path and canonical category slugs, see [docs/skill-catalog.md](./skill-catalog.md).

---

## Core Operator Skills

These three skills are the canonical examples referenced in the operator-skill pattern doc. They dogfood the governed lifecycle end-to-end.

| Directory | Purpose |
|---|---|
| `release/` | Prepare, gate, publish, and verify a versioned release of a runx project through governed phases. |
| `receipt-query/` | Answer a cross-run audit question against the receipt ledger and produce a sealed finding. |
| `message-send/` | Post one approved message or reply in one conversation on behalf of a principal, with readback. |
| `webhook-handoff/` | Validate a runx execution context and hand off a governed payload to an n8n workflow or a Zapier hook. |

## Agency & Operations

Skills that run a standing mandate, route work, or operate a project workspace.

| Directory | Purpose |
|---|---|
| `agency/` | Run a standing team with a mandate, advanced one governed case at a time. |
| `ops-desk/` | Operate a project, workspace, or account from an agent or machine principal. |
| `policy-draft/` | Turn a plain-English governance brief into one validated runx policy. |
| `lead-qualify/` | Assess supplied ICP criteria against factual lead context; keep contact eligibility separate. |
| `cold-outreach/` | Prepare relevant first-contact copy from evidence and scoped brand/taste context. |
| `lead-enrichment/` | Normalize fresh matched professional facts, preserving missing and conflicting fields. |
| `knowledge-router/` | Route a question or source event to the right knowledge source. |
| `issue-intake/` | Turn a noisy inbound request into a bounded intake artifact. |
| `github-pr-review/` | Review an exact PR head against bounded changed-line evidence without publication. |
| `issue-triage/` | Discover, analyze, and draft high-signal issue-thread responses. |
| `issue-to-pr/` | Implement and test an issue with normal host tools, finalize once, and optionally publish a governed PR. |

## Governance & Audit

Skills that enforce policy, audit receipts, or review skill quality.

| Directory | Purpose |
|---|---|
| `receipt-audit/` | Audit a sealed runx receipt for governance, comparing the authority chain to the evidence. |
| `run-history/` | Produce a read-only report over runx's own run history, summarizing sealed runs. |
| `skill-review/` | Inspect, safely test, and assess a skill package for capability, trust, and operator risk. |
| `diagnose-skill-run/` | Diagnose failed skill runs from receipts and harness evidence, then propose bounded improvements. |
| `least-privilege/` | Review exact attributable authority recorded in verified receipts; retain the grant until policy and usage evidence support narrowing. |
| `sign-receipt/` | Prepare an evidence-bound attestation of an off-runtime act. |

## Content & Research

| Directory | Purpose |
|---|---|
| `content-pipeline/` | Research a topic, draft the content, and package the approved output. |
| `ghostwrite/` | Turn evidence and operator intent into publication-ready drafts. |
| `deep-research/` | Produce an approved deep-research brief from bounded research. |
| `web-search/` | Discover bounded source candidates through an explicitly selected search provider. |
| `product-analytics/` | Interpret comparable supplied funnel and retention measurements. |
| `research/` | Produce bounded, source-backed research packets for product decisions. |
| `brand-voice/` | Build a scoped brand voice packet from source material. |
| `taste-profile/` | Build a scoped taste profile packet from examples and preferences. |
| `prior-art/` | Compare existing approaches, catalog surfaces, and domain patterns. |

## Code & Dependency Security

| Directory | Purpose |
|---|---|
| `cve-audit/` | Audit exact npm dependency versions against OSV and emit reproducible, independently verified CVE evidence. |
| `vuln-triage/` | Assess vulnerability risk and produce remediation guidance and an advisory draft. |
| `redact-pii/` | Scrub personal data out of content before it crosses a trust boundary. |

## Payments & Settlement

Public payment capabilities. Internal mock and rail packages are implementation
paths, not alternative operator choices.

| Directory | Purpose |
|---|---|
| `charge/` | Govern one inbound provider-side paid tool call through pricing and receipt. |
| `spend/` | Execute one governed outbound payment across a selected runtime. |
| `refund/` | Govern one refund linked to a sealed original charge receipt. |
| `settle-invoice/` | Settle a known, approved invoice under a spend-bounded grant. |
| `stripe-pay/` | Execute a governed Stripe Shared Payment Token spend by delegation. |
| `x402-pay/` | Execute a governed x402 payment by delegating to the canonical x402 runtime. |

## Data & Infrastructure

| Directory | Purpose |
|---|---|
| `github-sync/` | Read or execute a scoped pull or push of GitHub issues, threads, or PRs. |
| `web-fetch/` | Fetch and extract one web source within an explicit allowlist. |

## Communications & Campaigns

| Directory | Purpose |
|---|---|
| `dispute-respond/` | Prepare a governed dispute response artifact from a linked case. |
| `helpdesk/` | Classify a bounded support request and choose the safe next path. |

## Weather & External Data

| Directory | Purpose |
|---|---|
| `nws-weather-forecast/` | Fetch National Weather Service forecast evidence through the NWS API. |
| `open-meteo-weather-forecast/` | Resolve a place and fetch global forecast and air-quality evidence. |

## Skill Development & Testing

| Directory | Purpose |
|---|---|
| `skill-build/` | Design, build, improve, and add validated harness coverage to Runx skills. |
| `skill-adopt/` | Adopt a pinned third-party SKILL.md as a governed native Runx binding bundle. |
| `moltbook/` | Scan for posting opportunities and prepare governed Moltbook entries. |

## SQL & Structured Data

| Directory | Purpose |
|---|---|
| `sql-analyst/` | Turn a bounded data question, schema summary, and sample rows into findings. |
| `extract/` | Extract schema-validated JSON from messy HTML or text fixtures. |

## Planning & Work Management

| Directory | Purpose |
|---|---|
| `work-plan/` | Decompose a build objective into governed runx execution steps. |

---

## When to Add a New Operator Skill vs. Extending an Existing One

Refer to the ["Add A Project Operator Skill Only When"](./operator-skills.md) section in the operator-skills doc for the decision rule. In summary:

- **Add a new skill** when the work has a distinct lifecycle, its own evidence shape, or needs a separate trust boundary.
- **Extend an existing skill** when the new capability shares the same lifecycle phase, evidence type, and governance gate as an existing skill.
- **Mock lanes** (`mock-*`) should always be paired with their live counterpart — don't add a mock lane without a corresponding provider lane, and don't ship a provider lane without a deterministic mock.

## Mock vs. Live Lanes

The fixtures `fixtures/skills/mock-charge`, `mock-pay` and `mock-refund` are
deterministic local harness fixtures. They never move money and remain excluded
from ordinary operator discovery. Public payment skills are thin contracts that
require Runx Hosted for real execution; no private rail package remains in OSS.
