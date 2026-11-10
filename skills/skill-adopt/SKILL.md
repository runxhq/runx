---
name: skill-adopt
description: Adapt a captured third-party skill into a useful, validated local Runx package through the canonical authoring engine, preserving source context and required resources. Use skill-build for first-party authoring and skill-review for assessment alone.
---

# Skill Adopt

Turn an upstream skill you want to use into a real local Runx package. Supply
its captured manual and supporting files, a destination in your repository,
and the operator job it must perform. Adoption carries that source context
into Skill Build, which designs the execution contract, writes the complete
package, and applies it only after native validation and safe isolated replay.
The upstream files remain untouched.

Use this when adapting third-party operating knowledge. Use skill-review to
judge a skill before choosing it, or skill-build when you own the capability
and do not need upstream-source intake. A source that is redundant, incomplete,
unlicensed for the intended use, or impossible to govern may correctly produce
no_skill, needs_core, or a blocked result instead of an adopted package.

## Composes

<!-- Generated from the native closure; run pnpm core-skills:composes:generate. -->

- `skill-build#build`

## Source and destination

The default method is adopt. Inputs are the actual objective, repo_root
(default .), target_dir, and the appropriate source entry:

- adopt takes source_paths: repository-relative files including the upstream SKILL.md.
  Native fs.read_bundle captures their contents and digests; native metadata
  validation holds any missing or truncated files. Include
  licenses and referenced scripts, templates or other resources you already
  have; the builder can inspect genuinely missing local dependencies.
- from-source (`runx skill skill-adopt#from-source`) takes source_snapshot: an
  existing runx.fs.file_read_bundle.v1 value, reused without another source
  read. This is useful after review or another workflow step.
  A caller-supplied packet does not make its provenance claims verified.

source_byte_budget controls the fresh native read (default 524288 bytes);
the native reader owns its upper limits. validation_byte_budget bounds the
serialized snapshot checked by native data.validate (default 32 MiB); that
capability owns its upper bound. The JavaScript context step receives only the
small validation result, counts and artifact descriptor, never the source body. Missing or truncated source holds
before the authoring engine. The caller can increase that budget or pass a
complete admitted snapshot. This is not a whole-repository crawler or a remote
Git checkout service: the host obtains remote source through its normal bounded
reader before adoption. Do not guess missing files, licenses or credentials.

Optional package_name chooses an explicit derivative identity. Otherwise the
builder makes the identity decision from the source and requested job.
project_context supplies product constraints. provenance records optional
source_url, revision, license and notes as claims; it grants neither registry
identity nor trust. The native receipt binds the source bytes supplied here.

For example, after capturing an upstream skill locally:

    runx skill skill-adopt --inputs source-adoption.json --json

The input object names objective, target_dir and source_paths such as
["vendor/tool/SKILL.md", "vendor/tool/LICENSE", "vendor/tool/references/checklist.md"].
Choose the target deliberately: updating an existing package uses Skill Build's
inspected base digest and exact transaction, not an unchecked overwrite.

## What happens

1. Select adopt for a fresh local capture or from-source for an existing snapshot. Fresh reads stay in the chosen
   repository; a prepared snapshot preserves earlier bytes and completed work.
2. The snapshot is stored as an immutable native project artifact. Only its
   descriptor and small source counts and its native validation result enter model context; native artifact
   open/read tools provide bounded pages of the captured bodies. Source
   context, dependency and license obligations enter the existing Skill
   Build architecture turn. That owner chooses native capabilities and canonical
   skill routes before its author writes anything.
3. The author retains useful source procedure and an unchanged upstream
   reference copy where permitted, includes required resources, and declares
   the exact runnable inputs, results, tools and effects. Unsupported dependencies
   or enforcement requirements hold rather than becoming a simulated feature.
4. Skill Build validates and safely replays the exact candidate before one
   atomic local application. Its apply_result identifies the actual destination,
   changed paths, package digest and proof. The adoption receipt includes that
   child execution; a ready context alone is not an applied package.

An agent continues the offered nested architect and author turns using their
actual request addresses and tool contracts. Do not substitute answers copied
from a fixture. Use the result's outcome and apply_result: no_skill, needs_core,
unchanged, failure and applied are different results. Inspect and invoke the
returned local package for its promised job before recommending adoption.

## Authority and continuation

Adoption does not execute the upstream operational workflow, obtain credentials,
assign a verified publisher tier, or publish anything. Generated tools and scopes
are requested authority for later runs, not grants. A filesystem prefix in prose
or an echoed act count is not enforcement; unsupported restrictions must stop
at their native owner. No second policy or approval system is introduced here.

Call from-source with an existing source_snapshot in a composed workflow to preserve the reviewed
source without reacquiring it. Reuse a completed application instead of running
adoption again: subsequent work normally invokes the returned package. After a
failure, inspect the native apply_result and current target before retrying.
Publication, when wanted, goes through the canonical registry operator with
independent identity/version/digest readback; a local package is not published.
