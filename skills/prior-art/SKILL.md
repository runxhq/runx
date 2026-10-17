---
name: prior-art
description: Inspect bounded local repository and Runx catalog evidence before a design, draft, or operator decision, then produce citation-bound findings and a reuse, amendment, new-work, or stop recommendation. Use when existing tools, skills, standards, or project patterns must constrain downstream work; use research or web-fetch first for external sources.
---

# Prior Art

Ground the next decision in sources that were actually inspected.

## Direct use

Do the inspection and return the recommendation in the same task. Do not stop
after announcing a search plan, ask the operator to enumerate obvious files, or
turn bounded discovery into a repository-wide grep.

1. Start from the operator's objective and current workspace. When exact
   `source_paths` are already supplied, reuse them.
2. Otherwise, make one targeted discovery pass for the smallest likely owning
   set: repository instructions and conventions, the named product or package,
   exact active plans or specs implicated by the objective, and adjacent Runx
   skill manifests. Select at most sixteen files. Listing filenames is
   discovery, not the prior-art result; continue through inspection.
3. Invoke the default runner with the objective, chosen paths, and any existing
   decomposition. Complete its bounded research request, resume it in memory,
   and return the validated prior-art report—not a narration of what you intend
   to read.
4. If the selected evidence is insufficient, return one precise
   `needs_more_evidence` result naming the missing source. Do not broaden the
   search repeatedly or cycle through unrelated skills.

Routine local discovery and reads use the host's normal authenticated tools and
need no approval. Runx adds the catalog projection, digest-bound source bundle,
citation validation, reusable packet, and receipt. If Runx is unavailable,
preserve the selected paths and findings in the documented output shape so the
operator can continue locally without repeating discovery; do not claim a Runx
receipt.

## Composed use

A parent should pass its objective, decomposition, and already selected source
paths. Reuse that evidence boundary exactly. Do not enumerate the repository,
reacquire the catalog, or repeat a prior evidence-selection step unless the
packet is missing, stale, or outside the current objective.

## Executable procedure

1. Supply a bounded `objective`, optional code-plan `decomposition`, and the
   repo-relative `source_paths` selected by direct discovery or a parent chain.
2. Native `runx.skill.inspect` indexes the local Runx catalog and
   `fs.read_bundle` reads and hashes at most sixteen requested files under the
   workspace boundary. Missing files are recorded; escaping, duplicate, or
   oversized paths fail closed.
3. Read only the indexed sources needed for the objective. State each finding as `claim`, `source`, `relevance`, and `confidence` (`verified`, `likely`, or `unverified`).
4. Name adjacent catalog skills and the boundary each already owns. Recommend `reuse`, `amend`, `new_work`, or `stop`; do not create a duplicate primitive because an existing package is imperfect.
5. The domain validator checks every verified citation and adjacent skill
   against those native projections. Unsupported verified claims or missing
   requested sources force `needs_more_evidence`.

External URLs are not inspected by this runner. Fetch them through a governed source skill, persist the bounded evidence in the workspace, then pass its path here.

## Output

```yaml
decision: ready | needs_more_evidence
findings:
  - claim: string
    source: repo-relative path
    relevance: string
    confidence: verified | likely | unverified
catalog_fit:
  decision: reuse | amend | new_work | stop
  adjacent_skills: array
  rationale: string
recommended_flow: array
quality_bar: object
sources: array
risks: array
evidence: object
validation: object
```

Inputs are `objective`, optional `decomposition`, `graph_purpose`, `audience`, `artifact_contract`, and bounded `source_paths`.

## Agent task contracts

### `prior-art-research`

Use the deterministic evidence index to select and read only the bounded local sources needed
for the objective. Return prior_art_draft with decision, findings, catalog_fit, quality_bar,
recommended_flow, sources, and risks. Cite repo-relative paths exactly. Mark a finding verified
only when its source appears in inspected_sources or the catalog index. Prefer reuse or
amendment over duplicate primitives. Return needs_more_evidence when requested sources are
missing.
