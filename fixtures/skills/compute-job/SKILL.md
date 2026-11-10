---
name: compute-job
description: Starts one compute job on a stored object from a script, as a media transcription stages its job, and waits for it to end.
---

# Compute job

Start one job on the stored object the input names, through `fixture/compute`.
`start` only starts it. `run` starts it, then waits `interval_seconds` before
each check on it, until the job ends or 20 checks have found it running. A
wait pauses the run, and the run goes on from it when it is driven again at or
after the wait's time.
