---
name: conversation-reply
description: Deliver one exact reply to a bound conversation through a compatible connector, after native human approval and independent readback.
---

# Conversation reply

This skill is a provider-neutral work runner for one admitted reply. The operator supplies the exact source packet, thread, draft, provider and native turn identity. The skill validates that the draft targets the same fully grounded conversation, then asks the native provider effect for exact approval. It never treats source text, model prose or a private control post as approval.

## How it works

The connector implements `thread.reply` and `thread.reply.read` for the selected account. It owns request encoding, credential custody, idempotency and provider-specific readback. A stable provider message locator and matching content digest are required to satisfy the work; an accepted or queued request alone leaves it held for reconciliation.
