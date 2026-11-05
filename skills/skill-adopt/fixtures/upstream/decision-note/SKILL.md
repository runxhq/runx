---
name: fixture-decision-note
description: Turn one bounded objective into a reviewable decision note.
---

# Fixture Decision Note

Use this skill when an operator needs one bounded recommendation
from a supplied objective. It preserves the objective, explains
one next action, and stops without mutating any external system.

Supply a concrete objective. If it is missing or too vague to
support a recommendation, stop and request the missing boundary.
A sealed result is a decision note, not evidence that an external
action happened.

## Agent task contracts

### `fixture-decision-note`

Return `decision_note` with the objective preserved exactly and
one concise recommendation. Do not claim external effects.
