---
name: send-batch
description: Sends one message to each recipient of a batch from a script, each under its own key, and reports how each went.
---

# Send batch

Send `body` to each recipient in `recipients` through `fixture/mail`, each
under the key `send-batch:<recipient>`. A recipient whose send fails doesn't
stop the others. The result says, for each recipient, whether its message was
sent, was already done under its key, or failed, with the code it failed with.
