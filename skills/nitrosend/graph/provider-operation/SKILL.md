---
name: nitrosend-provider-operation
description: Run one bounded Nitrosend connector operation and project its result as provider evidence.
runx:
  category: communications
---

# Nitrosend provider operation

Internal graph stage for the public `nitrosend` skill. It owns only the
skill's operation allowlist, argument validation, the choice of Nitrosend
connector operation, and provider-result projection. The connector manifest
owns the MCP tool, its pinned digest and the arguments each operation sends;
the kernel owns admission, approval, credential delivery and custody.

Do not call this stage as an operator entrypoint. Use `nitrosend`.
