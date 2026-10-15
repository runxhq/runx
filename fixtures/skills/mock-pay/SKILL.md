---
name: mock-pay
description: Simulate an outbound payment locally for deterministic Runx harness tests.
---

# Mock Pay

This internal harness fixture validates basic mock inputs and returns a stable
`simulated` result with `money_moved: false`. It is not a rail and does not
exercise hosted authorization, credentials, reservation, ledger, recovery, or
finality. Use the public `pay` skill for real execution.
