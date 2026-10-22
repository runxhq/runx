---
name: managed-agent-intake
description: Managed-agent front example; a governed agent step that drafts a triage summary.
---
# Triage intake

Read the issue inputs and produce a short triage summary. When the task is
complete, return the structured result.

By default the runtime yields this act to the host (`needs_agent`). If a managed
agent provider is configured (`runx config set agent.provider anthropic`, plus
`agent.model` and `agent.secret`), the runtime drives the bounded tool-use loop
in-process instead, governed and sealed the same way.
