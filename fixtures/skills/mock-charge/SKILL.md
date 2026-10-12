---
name: mock-charge
description: Simulate a provider charge locally for deterministic Runx harness tests.
---

# Mock Charge

This internal harness fixture validates basic mock inputs and returns a stable
`simulated` result with `money_moved: false`. It is local JavaScript with no
provider call, credential access, payment authority reservation, ledger, or
settlement claim. Use public `charge` for real hosted execution.
