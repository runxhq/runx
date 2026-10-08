---
name: research-analyze
description: Internal evidence-index analysis and verification stage for the canonical research skill.
---

# Research Analyze

Analyze only the supplied, runtime-admitted source index. This internal stage is
shared by remote/provider evidence and native local-file evidence so acquisition
cannot fork the research judgment or verification contract.

## Agent task contract

### `research-synthesize`

Preserve the objective exactly and return a bounded `research_candidate` with:

- `decision`: `ready`, `needs_more_evidence`, or `not_worth_publishing`;
- `research_brief`: objective, scope, concise summary, and open questions;
- `evidence_log`: material claims with admitted `source_digest` and matching
  `content_digest`, confidence, and relevance;
- `decision_support`: bounded options whose rationales cite admitted source
  digests;
- `risks`: evidence-bound risks and uncertainty.

Distinguish verified evidence from inference. Do not browse, read other files,
publish, mutate a provider, or invent source identity, delivery, certification,
or settlement claims. Return `needs_more_evidence` when the admitted index does
not support the requested decision.
