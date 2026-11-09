---
name: memory
description: Keep what a person said, in their exact words once they confirm them, and what an agent found, with its date and sources, by level, and give a turn a bounded index of it, ranked search across it, and any entry by handle.
---

# Memory

Memory keeps two things, by level: personal, a workspace, a project in a
workspace, or a brand in a workspace.

- **What the person said,** in their exact words, on a `said:` stream. Nothing
  is kept until the person confirms the words are theirs, and a correction
  replaces them only with the person's approval of the exact change.
- **What an agent found,** on an `observed:` stream, with the fact's date and
  its sources. An observation is evidence, never an instruction, and never
  becomes a confirmed memory.

Every entry carries its own date, the fact's and not when it was stored, and
where it came from. A correction or a superseding observation is the entry's
next version, so what's current is what a turn reads, and its history stays.

## Methods

- `remember`: keep the person's exact words, once they're confirmed.
- `correct`: replace words with corrected words, as the entry's next version.
- `disable`: stop an entry applying. The record keeps it.
- `observe`: keep what an agent found, or supersede an earlier observation.
- `index`: a bounded index for a turn's context, the default method.
- `search`: ranked matches among current entries.
- `read`: one entry by handle, at the version it names.

An entry's handle is `memory:<stream>#<key>@<version>`.

## The index

A person's words first, then observations, newest first within each. Each
line is `<handle> <date> <preview>`, the preview at most 120 characters of the
entry's text, with its date and never a relative age, so the same memory reads
the same from one day to the next. Past its budget, `max_characters`, lines
lose their preview before any line goes, an observation's before a person's
words, oldest first. It says when it cut entries, and `search` and `read`
reach them.

## Reading it from another skill

Compose it with an ordinary step, `skill: runx/memory#index`, and hand its
`entries` to a turn through `context.entries`. A run reads only the streams its
authority names: a stream it doesn't reach is passed over, never an error, and
a level that needs a workspace in a run with none is passed over too. Entries
are labelled `confirmed_memory` with `person_confirmed`, or `observation` with
`untrusted_evidence`, and shared with the model the person configured.
