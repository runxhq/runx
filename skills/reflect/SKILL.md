---
name: reflect
description: Read the record since this reader's mark, lay out the checks it will be judged against, and keep what holds up as a change to the workspace the person approves.
---
# reflect

Reflection reads what happened since it last read, judges it against checks it
lays out before judging, and keeps what holds up. Code decides what is checked,
what passes and what is written; a model is used only to say why something went
wrong, to write a fix with its test case, and to ask whether a successful run
did what its skill is for.

## survey

`survey` reads, with no model:

- **Its mark**, a position in reflect's own `marks` stream, one per reader: how
  far the record has been read, and the runs that were still open there.
- **The record since it**: every sealed run that started from the mark to
  `until`, and every run open at the mark that has sealed since. Each run is
  read to its last event.
- **The workspace's memory**: its standing `said:workspace` rules, through
  `runx/memory#index`.
- **The workspace's revision**: its `HEAD` commit, and `AGENTS.md` as `HEAD`
  holds it.

It returns:

- **`resistance`**: every refused, declined or failed call, failed step,
  declined ask with the person's reason, unmet verdict, and run that sealed other
  than completed, each with its evidence: the sealed receipt, or the record's
  address of the event.
- **`sampled`**: the latest success of each skill in the window, for the
  purpose-only reader.
- **`unheld`**: each standing `said:workspace` entry whose handle `AGENTS.md`
  doesn't hold at `HEAD`.
- **`criteria`**: a `runx.criteria.v1` set with one check per item above and
  one of its own, that every run was read to its end, laid out before anything
  is judged. The same record gives the same set, by digest.
- **`subject`**: the window judged, its runs and the workspace's revision, by
  digest.
- **`mark`**: where the mark would move once the verdict covers the window, and
  the runs still open there. `survey` never moves it.

A survey that runs out of calls before reading every run says so: its own check
then can't be met, and the mark it returns moves only past what it read.
