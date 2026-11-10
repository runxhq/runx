---
name: memory
description: Keep what a person said, in their exact words once they confirm them, and what an agent found, with its date and sources, by level, and give a turn a bounded index of it, ranked search across it, and any entry by handle.
---

# Memory

Memory keeps two things, by level: personal, a workspace, a project in a
workspace, or a brand in a workspace.

- **What the person said,** in their exact words, on a `said:` stream, with
  their purpose: a `mandate` or a `procedure`, which stand as rules, or
  `confirmed_memory`, something they told you. Nothing is kept until the
  person confirms the words are theirs, and a correction replaces them only
  with the person's approval of the exact change.
- **What an agent found,** on an `observed:` stream, with the fact's date and
  its sources. An observation is evidence, never an instruction, and never
  becomes a confirmed memory.

Every entry carries its own date, the fact's and not when it was stored, where
it came from, and `summary`, the line the index shows for it, which its writer
writes for recall: a cue to read the entry, on one line of at most 120
characters. An entry can say the last date it's true, `valid_until`: once that
has passed, the index leaves it out, and history, search and `read` keep it. A
correction or a superseding observation is the entry's next version, so what's
current is what a turn reads, and its history stays.

## Confirming the person's words

`remember` asks the person to confirm the quote, the line the index shows for
it, its purpose, its dates, its sources and the stream it goes to. A host that
holds the words may answer itself only when the line is the quote itself and
the purpose is `confirmed_memory`. A line the host wrote, or a purpose that
makes the words a rule, is shown to the person, because a quote that checks
out doesn't confirm a summary of it or the force it's given.

## Methods

- `remember`: keep the person's exact words, once they're confirmed.
- `correct`: replace words with corrected words, as the entry's next version.
- `disable`: stop an entry applying. The record keeps it.
- `observe`: keep what an agent found, or supersede an earlier observation.
- `index`: a bounded index for a turn's context, the default method.
- `search`: ranked matches among current entries, by their text and
  summary only, never a confirmation's wording, a purpose or a source.
- `read`: one entry by handle, at the version it names.

An entry's handle is `memory:<stream>#<key>@<version>`.

## A write is shown what it likely replaces

`remember` and `observe` take `supersedes`: the handle of the entry the new
one replaces, or `none`. Without it, they search the stream they'd write for
current entries whose text matches the new one. If any match, nothing is kept
and they're returned as `matches`, so the writer decides with them in view and
calls again. If none does, there's nothing to decide, so it's kept at once and
the result says so, with no `matches` and the new handle. For `observe`, a
handle keeps the new observation as that entry's next version. For
`remember`, it makes the call a correction of that entry, at the correction's
own gate. So a fact that contradicts an older one can't sit beside it as
current unless the writer said it doesn't replace it.

## The index

Each line is `<handle> <date> <summary>`, with its date and never a relative
age, so the same memory reads the same from one day to the next. In order:

1. **Standing rules:** the person's words kept as a `mandate` or a
   `procedure`, every one, whatever its age, oldest first.
2. **Facts:** the person's other words, then observations. Given a `focus`,
   what the turn is about to do, the ones that match it come first, best
   first and newest first among equals, and the rest follow newest first.
   Without one, newest first.

An entry whose `valid_until` has passed, by the run's clock, is left out. Past
its budget, `max_characters`, lines lose their summary from the bottom up: the
lowest-ranked fact's first and the standing rules' last, the newest rule's
before the oldest. A fact's line goes only once every summary has. A standing
rule's line never goes: when the rules alone pass the budget, every one shows
and `over_budget` says so. It says when it cut entries, and `search` and
`read` reach them.

## Reading it from another skill

Compose it with an ordinary step, `skill: runx/memory#index`, and hand its
`entries` to a turn through `context.entries`. A run reads only the streams its
authority names: a stream it doesn't reach is passed over, never an error, and
a level that needs a workspace in a run with none is passed over too. Entries
are labelled with the person's purpose, `mandate`, `procedure` or
`confirmed_memory`, with `person_confirmed`, or `observation` with
`untrusted_evidence`, and shared with the model the person configured.
