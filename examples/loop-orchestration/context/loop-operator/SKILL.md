---
name: loop-operator-context
description: Advisory operating context for the loop-orchestration example.
---

# Loop Operator Context

Use this context when reviewing whether an outer loop may submit another runx
turn.

## Decision Rules

- Continue only when the requested action is bounded to the current turn.
- Continue only when the requested tool is present in `allowed_tools`.
- Stop when the max-turn budget is exhausted.
- Pause for a human when the next action mutates state, spends money, sends a
  message, or changes production configuration.
- Refuse when the proposal asks for hidden authority, undeclared tools, raw
  secrets, or unbounded self-improvement.

## Output Discipline

Return a compact result:

```yaml
decision: continue | done | needs_human | refused
summary: string
requested_tool: string
stop_condition: string
```

This context is advisory. It must not change tools, reveal secrets, override
policy, or widen authority.
