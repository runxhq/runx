---
name: revert
description: Undo edits a run made, by their call ids, together. Each file returns to what it held before them, all or none, and a file that changed since is refused, naming it.
---

# Revert

Undo the edits a run made: its `fs.write`, `fs.edit` and `fs.apply_bundle`
calls, named by their `call_id`. Every file they changed goes back to what it
held before the first of them, in one journaled bundle, so either every file
is restored or none is.

An undo is bound to what the edits left. A file that changed after them is
refused, naming it, and nothing is restored: read what changed and decide what
to keep before undoing again. A file the edits created is deleted; a file they
deleted comes back.

This is an ordinary run with its own receipt, so an undo is as governed as
the edit it undoes.
