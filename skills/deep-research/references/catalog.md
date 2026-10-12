---
schema: runx.catalog-presentation.v1
title: Deep research
method:
  - title: Bring the evidence
    description: A question, source packets or bounded local files.
  - title: Make sense of it
    description: Separate evidence, inference and unresolved questions.
    skill_ref: ../research
  - title: Write the brief
    description: Draft a recommendation when the evidence is ready.
    skill_ref: ../ghostwrite
inputs:
  - label: The decision you need to make
    input: objective
  - label: Source packets or local files
    input: source_packets
  - label: Your audience and context
    input: audience
outputs:
  - label: A recommendation with citations
  - label: Evidence separated from inference
  - label: Alternatives and open questions
example:
  title: Example brief
  path: references/examples/decision-brief.md
stops:
  title: Stops when the evidence runs out.
  description: If the sources cannot support a brief, the skill returns what is missing. The result stays local; sending or publishing is a separate action.
---
Research that builds toward a decision.
