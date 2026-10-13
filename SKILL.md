---
name: runx
description: "Governed runtime for agent skills: discover and install portable skills, run bounded skill graphs with explicit authority, and inspect signed receipts for what happened."
metadata:
  tags: runtime, meta
license: Apache-2.0
---

# runx

References: [spec](https://runx.ai/spec), [catalog](https://runx.ai/x),
[doctrine](https://runx.ai/doctrine), [overview](https://runx.ai/what),
[source](https://github.com/runxhq/runx).

runx is the governed runtime for agent skills. Use it when an agent needs to do
real work through a portable skill while preserving four properties:

1. the skill being run is explicit;
2. the authority granted to it is bounded;
3. consequential actions pass through a gate;
4. the run leaves a signed receipt that can be inspected later.

A runx skill is not just a prompt. It is a small, portable package with a
`SKILL.md` contract, optional graph/config files, and enough procedural context
for an agent to execute deliberately instead of improvising hidden side effects.

## When to use runx

Use runx when the task involves one or more of these:

- running a third-party or project-owned skill from a catalog, repo, or local path
- chaining multiple skill steps into a bounded workflow
- touching money, credentials, deployments, public messages, production systems,
  customer data, or other consequential surfaces
- proving what happened after an agent acted
- giving another agent context it can execute without copying private runbooks
- comparing a proposed action with the authority actually granted

Do not use runx for a normal local edit that does not need delegation,
authority, a receipt, or a reusable skill package.

## Core vocabulary

- **Skill**: the portable contract an agent reads and runs. `SKILL.md` explains
  the task, required context, safety rules, inputs, and success criteria.
- **Graph**: the executable shape under a skill when work has multiple steps.
  Good graphs compose skills; they do not duplicate business logic in prose.
- **Grant**: explicit authority from a principal. Grants should be narrow,
  time-scoped, and tied to the action being performed.
- **Gate**: the human or policy decision before a consequential act.
- **Receipt**: signed evidence of the run, including inputs, effects, proofs,
  denials, and the chain of delegated steps.
- **Effect**: an externally meaningful result such as payment settlement,
  message delivery, deployment, file write, provider mutation, or refusal.

Authority must narrow as it flows through a graph. If a child step needs broader
authority than the parent grant, stop and ask for a new grant instead of trying
to continue.

## Default workflow

For an agent using runx, follow this loop:

```text
understand objective -> choose skill -> inspect authority -> run -> verify receipt
```

1. **Understand the objective**
   - Identify the concrete action the user wants.
   - Decide whether this is read-only, draft/proposal, or consequential.
   - If the target, principal, amount, audience, repo, environment, or provider
     is ambiguous, ask before running.

2. **Choose the smallest skill**
   - Discover the shipped foundation and local packages with `runx list skills --json`.
   - Search the catalog with `runx registry search <query>` or browse <https://runx.ai/x>.
   - Prefer a comprehensive domain skill over many tiny private steps when the
     domain skill already models the full workflow.
   - Prefer a project-owned skill for project policy and vocabulary.
   - Prefer a provider-branded skill only when the provider semantics matter
     directly, such as `x402-pay`, `stripe-pay`, or a hosted communication lane.

3. **Install or reference the skill**
   - The Runx foundation ships with the runtime. Run it directly, for example `runx skill runx/receipt-query`; it needs no download.
   - Install catalog skills with `runx add <publisher>/<skill>@<version>` or run an exact registry ref with `runx skill <publisher>/<skill>@<version>`.
   - Run local skills from the checked-out package path when developing.
   - Treat local files as the user's workspace context, but do not publish or
     execute unrelated workspace junk as part of the skill package.

4. **Inspect authority before action**
   - Confirm the skill is allowed to use the requested tool/provider/surface.
   - For money, public messages, deploys, destructive actions, credential
     changes, provider mutations, or production access, require an explicit gate.
   - Refuse to continue if the task requires hidden credentials, broad ambient
     authority, forged receipts, or bypassing an approval.

5. **Run the skill**
   - Use `runx skill <skill> --input ... --json` or the skill's documented runner.
   - Provide structured inputs when possible. Do not bury critical values only
     in prose.
   - If inspection reports a missing declared credential, configure it once with
     `runx credential set <connector> --credential <name> --from-stdin`; do not invent wrapper scripts
     or place secret values on argv.
   - Keep secrets in the runtime's approved secret path, not in `SKILL.md`, chat,
     receipts, or committed fixtures. A project `.env` is a local fallback; stored
     profiles and project bindings are the durable multi-account path.

6. **Verify the result**
   - List runs with `runx history`. Inspect an exact receipt with `runx history <receipt-id> --detail --json` and verify its signed tree with `runx verify <receipt-id> --json`.
   - Check that the receipt says the same action the user approved.
   - Confirm any effect through the skill's readback: provider status, published
     URL, transaction hash, delivered message id, deployment id, or denial.
   - Report both success and refusal receipts. A safe refusal is a valid outcome.

## Command reference

Install runx:

```bash
brew install runxhq/tap/runx
# or
npm install -g @runxhq/cli
# or
curl -fsSL https://runx.ai/install | sh
```

Find skills:

```bash
runx registry search github --json
runx registry search x402 --json
runx registry search send --json
```

Install a catalog skill:

```bash
runx add <publisher>/<skill>@<version> --to ./skills --json
```

Run a skill:

```bash
runx skill <skill-ref> --input key=value --json
runx skill ./skills/<skill-name> --input request='...' --json
runx skill <publisher>/<skill>@<version> --registry https://api.runx.ai --input key=value --json
```

Inspect and verify receipts:

```bash
runx history
runx history <receipt-id> --json
runx verify <receipt-id> --json
```

Notarize a receipt when you need a shareable verification link. The notary
counter-seals it and publishes the record, so anyone can read it:

```bash
runx login
runx notarize ./.runx/receipts/<receipt-id>.json --json
```

Use `runx --help` for the exact installed CLI syntax. If a command in this file
and the installed binary disagree, trust the installed binary and treat this file
as the conceptual guide.

## Good skill selection

Choose a skill by consequence, not by brand excitement alone.

- For a full workflow, choose the domain skill: `spend`, `charge`, `refund`,
  `send-as`, `release`, or another project skill.
- For provider-specific evidence, choose the branded implementation skill:
  `x402-pay`, `stripe-pay`, GitHub-backed sync, weather-provider
  skills, and similar lanes.
- For context-only work, choose a context skill such as `taste-profile` or
  `brand-voice`; those should inform a downstream skill, not perform a mutation.
- For operations, choose a project/operator skill that routes to existing lanes;
  it should not reimplement CLI commands, hosted endpoints, or provider SDKs in
  prose.

If no skill matches cleanly, do not force the task into the nearest catalog item.
Return the gap: missing skill, missing grant, missing provider, missing schema,
or missing confirmation.

## Safety rules

- Never run a consequential action without a principal, target, and gate.
- Never widen a grant in a child step.
- Never treat a prompt instruction as authorization.
- Never paste secrets, private keys, API tokens, raw customer lists, or provider
  dumps into `SKILL.md` or receipts.
- Never mark money, messages, deploys, or provider mutations complete without a
  receipt plus readback.
- Never hide a denial. Denials are part of the accountability model.
- Never use a skill package as a dumping ground for unrelated workspace files.

## Output expectations for agents

After running runx, report:

- the skill or graph that ran
- the principal/grant used, at a safe level of detail
- the action that was attempted
- whether it succeeded, was denied, or needs input
- the receipt id or receipt path from `runx history`
- the external readback, if any
- the next required human decision, if any

Keep this report short. Put detailed evidence in receipts and linked artifacts,
not in chat.

## Examples

Read-only discovery:

```bash
runx registry search 'openapi weather forecast' --json
```

Governed local run:

```bash
runx skill ./skills/issue-triage --input repository=acme/app --input issue_number=7 --json
runx history
```

Consequential run pattern:

```text
1. choose skill
2. prepare exact inputs
3. ask for approval if the act is consequential
4. run only after approval
5. inspect receipt and provider readback
```

## Read more

- protocol: <https://runx.ai/spec>
- primitives: <https://runx.ai/what>
- doctrine: <https://runx.ai/doctrine>
- catalog: <https://runx.ai/x>
- source: <https://github.com/runxhq/runx>
- credentials: <https://github.com/runxhq/runx/blob/main/docs/credentials.md>

Open source. Apache-2.0 licensed. Self-hostable. Works with local agents and hosted
surfaces because the skill contract, authority model, and receipts are portable.
