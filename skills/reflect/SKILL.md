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
  `until`, now when none is given, and every run open at the mark that has
  sealed since. Each run is read to its last event, and where its package lay.
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
  the runs still owed: those that have not sealed and those whose checks
  remain unresolved. `survey` never moves it.

A survey that runs out of calls before reading every run says so: its own check
then can't be met, and the mark it returns moves only past what it read.

## reflect

`reflect` runs `survey`, then judges the window and decides it:

- **`judge`** gives each resistance to `diagnose` and each sampled success to
  `challenge`, each its own fresh turn. Code reads each run's record first and
  keeps a finding only when it cites evidence and every ref it cites is a
  sealed receipt or an event of that run. A finding that rests on anything else
  is dropped, and the check says why.
- **`propose`** drafts a fix for each repair through `runx/skill-lab#draft`:
  each kept diagnosis, and each sampled success its purpose-only reader found
  didn't do what its purpose asks, since a run that succeeded at the wrong
  thing needs fixing as a failure does. Only a package the workspace owns is
  changed. An installed one is fixed in your own copy of it at
  `skills/<publisher>/<name>/`, which runx runs in its place, once you've made
  one: a copy pins its version, so reflection never makes one itself. Until
  then, and for one runx bundles or one outside the workspace, its repair is
  kept as an observation for whoever owns it, saying why. `skill-lab`'s own turns write the fix and its case. Code keeps the
  draft only when it adds a case, names each case it changes or removes, and
  two validations prove the new case where the package lies, so the skills it
  calls resolve as they do for it: the new case fails on the package as the
  workspace holds it with only that case added, and passes on the whole
  change. Nothing is in the workspace until then. The change is
  applied with `runx.skill.apply` under `effects: ask`, so the person approves
  the exact files. A decline writes nothing and its reason is kept in
  reflect's `declines` stream, keyed by the change's target and evidence, so
  a later run doesn't propose that change again unless its evidence changed.
- **`promote`** writes each standing `said:workspace` rule the repository
  doesn't hold into `AGENTS.md`, word for word under its `## Rules` heading,
  with its date and handle, under `effects: ask`, so the person approves the
  exact file. A write that would take the file past 128 KiB, what reaches a
  turn whole, is refused, and the rule goes elsewhere. Reflection never
  commits: the person does.
- **`observe`** keeps an observation for each repair on the
  workspace's `observed:` stream, its evidence as its sources and its run's
  date as its own. One memory already holds is that one.
- **`decide`** answers every check with a `runx.verdict.v1` whose subject is
  survey's window, and `verdict.decide` decides it by the one rule. A repair is
  met once it's kept, as a proven fix that was applied or as an observation,
  and a rule once it's promoted: until then each is `not_met`, saying what it
  lacks, a declined fix with the person's reason. A sampled run that did what
  its purpose asks is met, and one with no finding kept is unresolved.
  An unresolved run stays in the mark's open list for the next reflection;
  a rejected diagnosis never finishes the work. The loop notice names what
  still needs reflection.
- **`keep`** closes each rule `AGENTS.md` holds at `HEAD` through
  `runx/memory#disable`, its reason naming the commit, so the person confirms
  the close at memory's own gate. A rule is held only where the file holds its
  line as `promote` writes it: the person's words, date and handle. A rule
  whose handle the file holds under other words is edited, not held: its
  memory stays active, and the person is shown both what they said and what
  the file says now. A close the person declines is kept in the
  `declines` stream by the rule's handle, so a later run doesn't ask again
  until the rule changes. Then it moves the mark past the window the verdict
  covers, at the version survey read, so a concurrent reflection's move wins
  and this one says it lost. Its `loop` output tells a loop that reflects
  whether the window changed anything, and what it read, proved, applied,
  promoted and kept, in words code writes.

## As a loop

`runx loop new runx/reflect <name>` makes a loop that reflects once a day as
its own reader. Each fix it proves and each rule it promotes waits in its run
for you, and its notice says what the window held.

Reflection's term reads the workspace's memory, writes its `observed:`
stream, and appends to its `said:workspace` stream only for memory's disable,
which closes a rule the repository holds. It never reads or writes personal
memory, and never keeps or corrects a person's words: it calls no
`remember` or `correct`.

## Agent task: diagnose

You are given one thing that resisted in a recorded run: a refused, declined
or failed call, a failed step, a declined approval with the person's reason,
an unmet verdict, or a run that stopped. Say why it happened from the supplied
event. If it shows what happened but does not establish a cause, say so.

Return `finding`: `summary`, what went wrong in one sentence; `cause`, why,
as the record shows it; and `evidence`, each sealed receipt id or event
address (`runx:run:<stream>@<version>`) your cause rests on, exactly as the
record gives them. Cite only what you read. If the record doesn't show why,
say so in `cause` and cite what it does show.

## Agent task: challenge

You are given a skill's purpose, one sentence from its manual, and one of its
runs that succeeded, as its record holds it: each step's output and each
call's outcome. You are not given how the skill is built, and you don't need
it. Judge whether the run did what the purpose asks: what a person relying on
the skill for that purpose would have needed from it.

Return `finding`: `met`, whether it did; `summary`, what it did against what
the purpose asks; and `evidence`, each receipt id or event address from the
run that shows it, exactly as given.
