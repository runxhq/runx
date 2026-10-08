# runx-sdk

CLI-backed Rust SDK for runx.

This crate calls an installed `runx` binary and consumes documented
`runx --json` output. It provides typed helpers for the initial client surface:
skill search, skill run, continue, host protocol decoding, and act-assignment
construction.

The SDK does not execute skills itself; it drives the installed `runx`
binary.

```rust
use runx_sdk::{RunSkillOptions, RunxClient};

let client = RunxClient::new();
let results = client.search_skills("sourcey", None)?;
let report = client.run_skill(
    "skills/sourcey",
    RunSkillOptions::default().with_input("project", "."),
)?;
```

The SDK depends on `runx-contracts`, not `runx-core`.
