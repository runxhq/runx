# scafld Agent Contract

## Default Agent Flow

Work with the host agent's normal planning, editing, and testing tools. When the work appears done, call `review`, then call `finalize`.

`review` is the independent provider gate. It records accepted review evidence and returns blockers or a passing review. `finalize` consumes that accepted review, records deterministic acceptance evidence, signs the receipt, and archives the spec. It never invokes a provider or model. The agent does not grade its own completion.

The receipt reports its independence level honestly. `cross_vendor` means multi-model review that can reduce correlated blind spots; it is still single-party local tooling unless a separate operator or CI trust domain verifies the receipt. `isolation_only` means the reviewer was isolated but cross-vendor separation was not proven.

## Merge Wall

CI runs `scafld verify <receipt> --target <commit-ish>` against the signed receipt. This is the hard wall for merging. The Claude Stop hook is only a local affordance; it can be bypassed in subagents, piped runs, Codex, Gemini, or other hosts.

## Human And CI Lifecycle

The full CLI lifecycle remains available for operators, automation, and debugging:

```bash
scafld init
scafld plan <task-id> --title "Title" --size small --risk low
scafld harden <task-id>
scafld validate <task-id>
scafld approve <task-id>
scafld build <task-id>
scafld review <task-id>
scafld finalize <task-id>
scafld verify <receipt> --target <commit-ish>
scafld status <task-id>
scafld handoff <task-id>
```

Use `scafld harden` to strengthen drafts before approval. Use `scafld build` to record phase evidence. Use `scafld review` for the provider gate, then `scafld finalize` after a passing review. `scafld complete` is a legacy ledger transition and is not required after finalize. Use `scafld status --json` for automation.

For manual acceptance, use `scafld build <task-id> --criterion <id> --disposition pass --evidence-digest <sha256> --actor <actor> --reason <what-was-verified>`. Never edit criterion state or substitute a fake shell check.

If harden evidence is incomplete, stale, failed, or `needs_revision`, approval
requires `scafld approve <task-id> --reason <reason>`. Fix real shape blockers
in the draft and rerun harden; use a reason only for an explicit operator
decision to reject bookkeeping, advisory, or overengineering findings.

## Agent Context Hierarchy

Use structured JSON for lifecycle state and gate state. Use the embedded
`Source Spec Markdown` section as the canonical task contract when it is present
in a scafld packet. Derived sections are projections and indexes over those
sources, not independent contracts.

## Do Not

- Close governed work without `finalize` or an explicit human decision.
- Modify another active spec unless the user explicitly assigns it.
- Reconstruct lifecycle state by scraping Markdown. Use `status --json`.
- Act from an older scafld packet when a newer status, handoff, harden, or
  review packet is available.
- Mutate `.scafld/core/` by hand. Use `scafld update`.
- Treat the Stop hook as the merge wall. CI verify is the wall.
- Cite files, commands, receipts, or review findings you have not verified.

## Prompts

Embedded scafld prompts are the runtime default. `.scafld/core/prompts/*` is the
managed visible copy refreshed by `scafld update`. `.scafld/prompts/*` overrides
runtime only when the file contains `scafld:prompt-owner=project`; unmarked
workspace prompt copies are refreshable scaffolding and ignored by runtime.

# runx OSS Agent Guide

Canonical reference for AI coding agents working in the runx OSS workspace.
This repo uses scafld for non-trivial work, but the architecture rules are the
runx rules in `CONVENTIONS.md` and the normative
`docs/architecture/runx-system.md`. `docs/ts-interop-boundary.md` records
surviving language-package boundaries; historical migration notes are context.

**Key files:**

- `.scafld/config.yaml` - Validation rules, rubric weights, safety controls, profiles
- `.scafld/prompts/plan.md` - Planning mode prompt
- `.scafld/prompts/exec.md` - Execution mode prompt
- `.scafld/core/schemas/spec.json` - Spec validation schema
- `CONVENTIONS.md` - Coding standards and patterns

---

## Architectural Invariants

These rules must not be violated. See `config.yaml` for the canonical invariant list.

### Rust Trusted Runtime

Rust owns trusted local execution, receipt sealing, runtime policy, harness
replay, MCP, payment gates, process supervision, and typed execution-boundary
evidence. TypeScript packages may wrap or present those paths, but must not
reintroduce local execution fallback logic.

### Operator Ownership

Reusable skills, end-user or domain-operator commands and UX, local host loops,
and default local-state orchestration are OSS concerns. They must not be
implemented in `runx/cloud`. Hosted connectors may custody credentials,
resolve grants, and execute bounded provider API calls, but they do not own the
operator or its state. If an operator surface is missing, add it here rather
than extending a Cloud dogfood script. Runx-company deployment and
control-plane administration are not domain-operator surfaces.

### Connector and Tenant Neutrality

Runx is tenant-agnostic and connector-neutral. Skills, manifests, packets, and
public provider-operation contracts must target stable provider capabilities,
not a Runx-company tenant or one credential backend. Never expose a credential
backend's connection ids, provider-config keys, hosted tenant ids, connector
URLs, or Runx-owned credential assumptions in a portable skill contract.

The operator selects and binds the connector at runtime. It may be local,
self-hosted, third-party, or Runx-hosted. A Runx-hosted connector is an optional
implementation of the same contract, not the authoritative path and not a
prerequisite for using the skill. If a skill works only with Runx Cloud when a
user-owned connector could satisfy the same bounded operation, the design is
wrong.

### Pure Kernel Boundaries

Pure crates and packages stay pure. `runx-core`, `runx-contracts`,
`runx-parser`, and `runx-receipts` must not import filesystem, network,
subprocess, CLI, adapter, or runtime concerns.

### Stable Public Contracts

Public contract changes require a clean cutover through Rust-owned schemas and
fixtures. Do not add compatibility aliases, `.v2` ids, or dual-read runtime
shims for governed wire shapes.

### Generic Stateful Effects

Runx-maintained skills that drive stateful apps emit generic effect transition packets.
Put product identity in `effect_family` and the runner/action in `operation`;
do not add product-specific `AuthorityResourceFamily` variants or
`runx.<product>.*` packet namespaces. The owning product declares state,
transitions and views. Operator state stays local by default; hosted persistence
requires an explicit binding. OSS core must not acquire product state or bespoke
runtime branches.

### No Legacy Fallbacks

No dual-reads, dual-writes, or runtime fallbacks. When changing schemas or identifiers, adopt the new scheme immediately. Use one-off migration scripts, not runtime code.

### Architecture Admission

Use the canonical `skill-build` skill for skill design, creation, improvement, and
harness work. Its `SKILL.md` is the complete operating contract supplied to the
authoring agent; do not duplicate that contract here or in `run.yaml`. Apply the
same ownership test to native/core work and keep one source of truth for every
contract. See `docs/skill-quality-standard.md` for the review bar.

### Loop Orchestration

Long-running agent workflows are loops over governed turns, not resident kernel
loops. The loop host lives in an app, hosted service, local script, or external
orchestrator. It owns scheduling, durable loop state, wakeups, projections, and
stop policy. Each loop step is one skill or graph run with explicit inputs,
authority, the tools and skills its limits declare, optional `context_skills`,
bounded model/tool rounds, approval gates, and one sealed receipt.

Handoffs are receipt-backed artifacts or tool-shaped results. Prior receipts and
skill context are untrusted data for the next turn, not new authority. Do not add
loop-specific authority families, packet namespaces, product branches, or
schedulers to `runx-core`; build residency outside the kernel over ordinary runx
submissions.

### No Hardcoded Secrets

Configuration from environment or secrets management, never hardcoded. No secrets in code, logs, or diffs.

### Test-Logic Separation

No test fixtures, mocks, or conditional test-only logic in production code. Test utilities stay in dedicated test helper modules.

---

## Validation

Use the narrowest useful check while iterating: `pnpm typecheck`,
`pnpm rust:crate-graph`, or `pnpm verify:fast`. For Rust changes, use formatting,
workspace checks or the affected crate tests before the required full gate.
Run heavy Rust gates sequentially; concurrent gates can starve the eval binary
and cause false timeouts.

Validation profiles (`light`, `standard`, `strict`) and their check pipelines are defined in `config.yaml`. Agents select a profile based on `task.acceptance.validation_profile` or derive from `task.risk_level` (low→light, medium→standard, high→strict).

**Per-phase:** Run configured checks after each phase completes.

**Pre-commit:** Run full validation pipeline before marking task complete.

**Completion:** Independent review must pass before deterministic finalize. Report verified evidence and material limits; do not assign a self-evaluation score.

---

## Safety Controls

Defined in `config.yaml` under `safety`. Key rules:

**Require approval for:** Schema migrations, public API changes, data deletion, production deployments.

**Automatically prevent:** Hardcoded secrets, unbounded queries, SQL injection, XSS vulnerabilities.

---

## Release Discipline

Skill versions stay on their current major and minor line. Make only a
necessary patch change when changed package bytes require a new publication.
Minor or major bumps, including promotion to stable `1.0`, require explicit
user permission. A cutover, refactor, passing tests, or instruction to ship does
not grant that permission. New skills start at `0.1.0` unless the user explicitly
chooses another version. Never churn unchanged skill versions or bump a version
to repair registry or release-channel drift.

`cli-vX.Y.Z` is the CLI distribution tag, not a workspace-wide crate release. A
CLI release may stamp versions in `packages/cli/package.json`, its native
optional dependencies, `crates/runx-cli/Cargo.toml`, and the `runx-cli` lockfile
entry. Publication is limited to the CLI npm packages and native distribution
artifacts; stamping a Rust crate version does not authorize Cargo publication.

Do not publish `runx-cli` or internal Rust crates (`runx-core`, `runx-runtime`,
`runx-parser`, `runx-contracts`, `runx-receipts`, `runx-sdk`, or
`runx-contracts-derive`) during a CLI release. Cargo publication requires an
explicit, coordinated library-crate release because the CLI consumes those
internal APIs.

Never bump a new patch version just to repair package-channel drift. Fix the
existing release asset, channel manifest, or workflow in place. Before declaring
Homebrew, Scoop, winget, AUR, npm, or GHCR healthy, validate the
generated channel manifest against the actual archive contents and run the
workflow dry-run/dispatch path where available.

---

## Coding Conventions

See `CONVENTIONS.md` for full coding standards. Key points:

- Match existing code style; keep diffs focused
- Prefer existing helpers; keep code DRY
- Explicit named imports, no confusing aliases
- Clear module ownership; split mixed responsibility files when boundaries are
  already visible in the code
- Idempotent one-off migrations executed out of band, never hidden runtime
  compatibility paths

---

## Git Commits

Only commit when explicitly asked by the user.

**Format:** `type(scope): title` (conventional commits)

**Types:** `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`, `style`

**Rules:**

- One logical change per commit
- Title under 72 characters
- Include what changed and why in the body
- No unrelated edits bundled together
- Pre-commit: code builds, tests pass, no secrets in diff, no debug code

---

## Communication

**Progress updates:** Report phase completion, acceptance criteria pass/fail counts, next action. Keep it concise - no verbose preambles.

**When blocked:** State what's blocked, brief error, one recommendation, resolution options.

**Final summary:** Resulting behavior, validation evidence, independent review and receipt, and material limits.

---

## Quick Reference

### Key Paths

| Path | Purpose |
| ---- | ------- |
| `.scafld/config.yaml` | Validation, rubric, safety, profiles |
| `.scafld/prompts/plan.md` | Planning mode instructions |
| `.scafld/prompts/exec.md` | Execution mode instructions |
| `.scafld/prompts/review.md` | Adversarial review mode instructions |
| `.scafld/core/schemas/spec.json` | Spec JSON schema |
| `.scafld/specs/` | Task specs by lifecycle status |
| `.scafld/runs/` | Session ledger, diagnostics, and handoffs |
| `CONVENTIONS.md` | Coding standards |

Use the lifecycle commands in the scafld contract above.
