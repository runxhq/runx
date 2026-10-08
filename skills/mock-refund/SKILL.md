---
name: mock-refund
description: Simulate a refund locally for deterministic Runx harness tests.
---

# Mock Refund

This internal harness fixture validates basic mock inputs and returns a stable
`simulated` result with `money_moved: false`. It does not resolve a real receipt,
account for prior refunds, call a provider, or prove a reversal. Use public
`refund` for real hosted execution.
