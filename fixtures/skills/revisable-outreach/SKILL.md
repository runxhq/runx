---
name: revisable-outreach
description: Prepare a fictional cold-outreach batch and approve each exact local fixture message before sending it.
---

# Revisable outreach

This executable fixture composes the actual cold-outreach batch with
fixture/mail. It keeps each campaign and recipient's key across revisions.
Only ready reviews are sent, after approval of the exact recipient and message.
Each call settles independently, so an unknown send holds its own recipient.
The CLI integration test interrupts the mailbox between acknowledgement and
readback, then revises the offer, recovers and checks the actual mailbox.
The mailbox is a local process that records every send and reads it back by key.

Install this fixture beside the actual cold-outreach, lead-enrichment,
lead-qualify and ghostwrite packages and their dependencies. Its fixture/mail
connector must declare `approval: required` for message.send. The integration
test installs that complete dependency set into a fresh workspace per variant.

Run the deterministic proof from the repository root:

```sh
cargo test --manifest-path crates/Cargo.toml -p runx-cli --test integration revisable_outreach
```

The two optional real-model tests require a keyless local chat-completions
server serving `qwen3-8b-32k`. Set `RUNX_TEST_OUTREACH_MODEL_URL` to that
server's endpoint and `RUNX_TEST_OUTREACH_PROVIDER` to its provider name, then
run the `crash_points::revisable_outreach::a_local_model_` filter with
`-- --ignored --test-threads=1`. The defaults use Ollama on port 11434.
Each task has an explicit eight-round bound. The test records the actual
requests and per-stage counts in its disposable workspace.
