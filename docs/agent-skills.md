# Agent Skills: runx Inside Claude and Codex

runx skills are governed by the runx runtime, not by a model following prose.
`runx export` makes those governed skills available *inside* an agent such as
Claude Code or Codex: each runx skill becomes a native agent skill whose only
job is to call the runx binary. The agent brings the judgment; runx admits the
authority, performs the act, and returns a signed receipt.

## How It Works

`runx export <claude|codex>` generates orchestrator-native files that delegate
back to runx:

- **Claude** gets one `SKILL.md` shim per skill under `~/.claude/skills/<name>/`
  (or `./.claude/skills/` with `--project`). Each shim declares `allowed-tools`
  locked to the runx binary and carries a package- and execution-closure-bound
  `runx skill ... --json` command. The exact runner schema stays inspectable on
  demand instead of being copied into every agent context.
- **Codex** gets the same per-skill shims under `~/.codex/skills/<name>/` plus an
  idempotent managed allow block in `~/.codex/rules/default.rules`. Project-scope
  Codex export is deliberately refused until Codex project paths are stable.

The repository-root `runx` guide is the one exception to delegation: exporting
it copies its native operator instructions instead of recursively calling
`runx skill <runx-repository>`. Provider/domain skills still delegate through
the runtime.

When the agent runs the skill it shells out to runx. Execution, authority
admission, approvals, and the signed receipt all happen inside the runtime, so
the governance is real rather than narrated.

## Export

```bash
runx export claude                          # current workspace's public skills -> ~/.claude/skills
runx export claude --project                # -> ./.claude/skills (checked into a repo)
runx export claude nws-weather-forecast pay   # only the named skills
runx export codex                           # ~/.codex/skills plus managed rules
pnpm codex-export:check                     # verify the active managed Codex surface
```

Add `--json` for machine-readable output. Only public skills export; internal
runtime paths and harness fixtures are skipped.

A full export must run from a skill workspace containing a root `SKILL.md` or
`skills/`. It synchronizes that workspace's managed catalog and removes only
stale generated shims sourced from the same workspace; exports owned by other
repositories remain intact. An unrelated directory fails closed without
changing existing shims. Supplying one or more skill refs is additive: Runx
updates those shims without removing any other managed skills.

## What A Claude Shim Looks Like

`runx export claude` writes a compact shim whose important structure looks like
this for the `pay` skill:

````markdown
---
name: pay
description: Pay one external payment requirement within exact parent authority, through a payment method the payer holds, and read the paid resource back.
allowed-tools: Bash(/path/to/runx skill *) Bash(/path/to/runx resume *)
---
# pay - governed by runx

Run the declared runner through runx; do not bypass it by independently reproducing work that runner owns.

<!-- runx-source-manual-begin digest=sha256:<manual> package-digest=sha256:<package> bytes=<n> -->
<the exact canonical SKILL.md body>
<!-- runx-source-manual-end -->

## Default runner: `pay`

Inspect the exact input and effect contract on demand:

```bash
/path/to/runx skill inspect /path/to/skills/pay pay --json
```

```bash
/path/to/runx skill /path/to/skills/pay pay \
  <validated inputs> \
  --package-digest sha256:<package> \
  --execution-closure-digest sha256:<closure> \
  --json
```

Then surface the returned receipt id, status, and artifact ids.

<!-- runx-export:claude source=/path/to/skills/pay package-digest=sha256:<package> - generated, do not edit -->
````

For Claude, the `allowed-tools` line limits the shim to the runx binary. Codex
uses the generated rules block. Run and resume commands carry both exact
digests, so edited package bytes or a changed execution closure fail before
execution. Runx remains the authority boundary on both surfaces.

## Requirements

- **The runx binary.** The shim calls runx by path, so that binary must be
  present.
- **Receipt identity.** Local runs sign with the runx home's own key, created
  on the first run. Hosted and CI execution must configure the complete
  `RUNX_RECEIPT_SIGN_*` tuple; a partial tuple fails closed. See
  [Getting Started](./getting-started.md#production-receipt-signing).
- **Provider accounts.** Bind them once as connector instances with
  `runx connector use`. Exported shims do not add
  credential wrappers; the invoked Runx skill performs the canonical readiness
  check. See [Credentials](./credentials.md).

## Regenerating

Rerun `runx export` after you add, rename, or remove skills; the shims are
generated, so do not hand-edit them. If a shim's source skill moves, the stale
shim fails closed and instructs you to rerun the export, so a renamed skill
never silently runs the wrong thing. A full export also replaces digest-stale
Runx-managed shims from the same source workspace and prunes only obsolete
entries bearing that workspace's managed marker.

## Portability

A shim bakes the runx binary path resolved at export time. Exported from a
source checkout it points at the local debug build; export with the published
CLI on your `PATH` (the `@runxhq/cli` global) so the shim is portable across
machines.

## The General Agent Bridge

Per-skill exports are the right call for governed skills. The exported root
`runx` guide is the looser discovery bridge: it teaches the agent to find skills
in the [catalog](https://runx.ai/x), run them, and read the receipts without
wrapping the runtime in itself.
