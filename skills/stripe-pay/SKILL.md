---
name: stripe-pay
description: Execute a Stripe SPT payment through the canonical hosted spend contract.
---

# Stripe Pay

`stripe-pay` is a discoverable branded facade over `spend`. Its only runner
selects the hosted `stripe-spt` rail and forwards the exact payment signal,
parent authority, opaque rail profile, optional opaque admission material,
realm, and idempotency seed to the canonical contract.

There is no Stripe SDK, API client, SPT adapter, credential custody, ledger, or
finality implementation in OSS. Those responsibilities live in Runx Hosted.
The local graph cannot fall back to a simulator and cannot claim settlement
without hosted mutation plus provider readback.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `spend#spend`

## Operator guide

Use the `fixtures/skills/mock-pay` fixture only for deterministic local tests. For real Stripe execution,
use this skill with an explicitly configured hosted Stripe grant. Stop on
missing authority, profile, approval, hosted binding, or consistent readback,
and never pass Stripe secrets in skill input.
