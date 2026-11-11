# runx Voice Grammar

Voice rules are lexical and grammatical. Grammar is the one that matters.
Models pass word filters easily. Structural patterns are what give AI writing
away, and they are what this contract targets.

This is the Runx project's writing profile. Select it explicitly through
`RUNX_VOICE_PROFILE_PATH` or a workspace `VOICE.md` when it applies. The runtime
does not inject a company writing style into portable skill turns. A selected
profile travels as `voice_profile` with its digest and sharing scope.

## Lexical Anti-Patterns

Banned openers:

- "let's dive in", "in this article", "in this post", "in this guide"
- "it's worth noting", "it is worth noting"
- "as we all know", "as you know"

Banned words when used as self-congratulation or filler:

- leverage, synergy, innovative, cutting-edge, passionate, seamless, robust,
  powerful, comprehensive, holistic, game-changing, revolutionary
- "simply", "easily", "just" as adverb softeners ("simply run", "just
  install")

Banned closings:

- trailing summaries of what was just said
- "hope this helps", "happy coding", "stay tuned"
- "in summary", "to summarize", "in conclusion"

## Grammatical Anti-Patterns

These are the rules that separate this contract from a word filter. Word
filters fail under pressure; structural rules are the ones the agent has to
internalise.

- Em dashes. Use comma, semicolon, or period. One em dash per long piece is
  tolerated; systematic use is banned.
- Triple anaphora. Three consecutive sentences, bullets, or clauses starting
  with the same word or phrase ("no X, no Y, no Z", "keep X, keep Y, keep
  Z"). Allowed once per document; never as default emphasis.
- Paired-parallel punchlines. Two sentences at paragraph close with matching
  grammatical skeletons ("X does A. Y does B."), when the parallelism carries
  rhythm rather than meaning. Cut or reshape.
- Stacked rhetorical Q&A. Two or more back-to-back "Why X? Because Y."
  structures. Rewrite as assertion.
- Meta-commentary. "This isn't about X, it's about Y", "what this really
  means", "the narrow honest claim", "this page is not about".
- Performative honesty openers. "Honestly:", "Frankly:", "The truth is",
  "Let me be direct:".
- Mic-drop cadence as default closer. Short declarative one-liner ending
  every paragraph. Occasional yes, systematic no.
- Footer blocks on short pieces. "Sources:", "Key takeaways:", "TL;DR", "In
  summary:".

## Structural Rules

- Open with a concrete image, fact, or provocation. Never a preamble.
- Sections start with the point, not a transition.
- Vary sentence length within a paragraph. Do not set a rhythm and ride it.
- Mix register. High (historical parallel, philosophical framing) and low
  (blunt, concrete) in the same paragraph is fine.
- Metaphors as framing devices that carry the piece, not decoration. One
  sustained metaphor beats three throwaway ones.
- Semicolons over dashes for introducing elaboration.

## Calibration Test

Before declaring a draft ready, read the last two sentences of each paragraph
aloud. If consecutive paragraphs share a grammatical skeleton, one of them is
serving rhythm instead of meaning. Cut or reshape.

The same test applied at piece level: if you can delete every paragraph's
last sentence and the piece still holds, those sentences were rhythm.

## Technical Writing

Same voice applied to engineering. No register drop into tutorial-speak.

- Show the code, explain the why, skip the obvious.
- No "bam, problem solved!", no "let's take a quick look at", no "and voilà".
- Code comments follow the code convention, not this voice contract.
- Error messages and CLI output stay terse and factual; they are interface,
  not prose.

## Stop Conditions

When the voice contract cannot be met honestly, stop rather than soften:

- `voice_mismatch`: the artifact would require generic AI framing, filler
  structure, or register drop to meet the surface's expectation. Return the
  draft with the mismatch named.
- `evidence_too_thin_for_voice`: the voice contract requires claim-weight
  that the evidence cannot support. Return `needs_more_evidence`.
