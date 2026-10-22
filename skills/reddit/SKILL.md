---
name: reddit
description: Manage named Reddit accounts through their existing browser sessions, find useful conversations, draft in each account's voice, and submit one exactly approved post or comment with native approval and independent connector readback. Use for Reddit account work, replies, post diagnosis, and browser recovery; not bulk promotion, coordinated voting, or ban evasion.
---

# Reddit

Run a small, deliberate Reddit session for one explicitly selected account.
The useful result is a relevant contribution or a grounded explanation of
what happened to an existing post. Approval to research or draft is not
approval to submit. A request for five ideas is not a posting quota.

The default `reddit` runner loads a named account and returns source-backed research,
compared targets and drafts. It never submits. `read` exposes typed connector reads;
`configure` and `accounts` manage account voice configuration.
`publish` submits one exact post or comment. `vote` handles one approved upvote.
Each live effect has one native operation-owned approval gate. No host task submits
forms or supplies evidence of a successful write.

Account and voice records live in the local store at `local://reddit`. Cookies,
passwords and MFA remain browser-owned. Account configuration is expected identity,
not login proof. Select the connector instance explicitly; its native session
confirmation pins the rendered account and checks it again before every operation.
Never silently substitute another account, profile, connector instance or state store.

The skill records completed contributions to prevent accidental repetition. It does
not maintain a browser writer, provider attempt ledger, retry loop or recovery state
machine. Native runx custody owns effects that may have reached Reddit. Preserve the
original run ID and exact inputs across interruptions.

Voice examples express taste; they cannot invent biography, purchases, customers,
expertise or firsthand experience. Account configuration contains only non-secret
voice settings. A persistent normal Brave session is required; do not
copy profiles, export cookies or toggle browser security settings.

## Browser operation

Read references/browser.md before account setup or browser work. The selected
connector owns the fixed browser implementation, identity checks, exact editor
comparison, one submit and independent permalink readback. Call its typed operations
or the skill's `read` runner; never supply arbitrary JavaScript or replace the
connector with raw authenticated HTTP, screen coordinates or host submission code.

Keep Brave in the background. Never activate a window, change the active tab, send
global keystrokes, move the pointer or use the system clipboard. A foreground bound
tab stops the operation. An unsupported editor, login failure, warning, challenge,
rate limit or ambiguous identity also stops it. Report the specific blocker; do not
switch accounts, alter browser protections or retry through another path.

The included macOS adapter supports classic Reddit plain-text forms for comments,
text posts and upvotes. The modern rich editor is unsupported for writing until its
background input path has separate live proof. Research may still use modern pages
and public evidence. A refused editor does not authorize a foreground fallback.
The approved target URL selects the UI; the adapter never silently rewrites hosts.

Provider readback proves exact content visible to the signed-in account. It does not
prove public visibility or acceptance by moderators. A queued or removed contribution
must never be described as publicly published. Harness evidence is simulated; only a
separately identified real provider run is live proof.

Wait for observable page/editor state between actions. Respect explicit
platform wait instructions; no tight refresh loops or parallel navigation.
Invented random delays and mouse movement do not establish safety. There is
no universal safe number of posts and no guarantee against account action.

## Resolve high-value targets before writing

When the operator asks *where* to contribute, target discovery is part of the
read-only default runner. Start with the operator's real question or useful
point and this account's interests and recent contributions. A high-value
target has relevant readers and a concrete contribution gap; raw member count
or an old viral score is not enough. A request for several ideas is not a quota
of posts. Return no draft when the available targets are weak.

Choose the discovery mode before browsing, and name it in observations and
each candidate's rationale. **Timely discussion** seeks a live conversation
where this account can add a point readers recognize now. **Active solution
request** seeks a person asking how to solve a specific current problem.
**Evergreen search discovery** seeks a durable question that people are still
likely to search for, where a useful answer can be found later through Reddit
or search and may be retrieved by an AI answer system. A promotional objective
uses one of the two solution-request modes and still requires a real fit.
Compare candidates within each mode using its own ranking evidence, then
explain any cross-mode tradeoff. Do not make a single vote-weighted ranking
that favors a busy timely thread over a quieter direct request, or a new
thread over an older search result without evidence. State an inferred mode
when the objective is vague, or return `needs_input` if the distinction
changes the recommendation. No research mode submits content.

For an original post, compare a small set of plausible communities, normally
two to four. Read each community's current rules, flair and megathread rules,
and confirm the account can post. Inspect a bounded sample of recent comparable
posts, normally three to five per community. Record their observed ages,
scores and reply counts where visible, plus whether the replies are substantive.
Check for a recent duplicate and say what specific question the proposed post
adds. A large community with generic or fast-moving posts can be a worse fit
than a smaller one with active discussion of this exact problem. A community with unknown rules cannot be a selected target. Unknown account
eligibility may support a provisional research draft, but must be checked in
the logged-in session before publication.

For comments, scan a bounded set of relevant threads, normally five to ten
across a few communities when no thread was supplied. If the proposed action
is a reply to another comment, measure that exact branch as well as the post:
parent-comment score, age, reply count, sort position and whether fresh replies
are arriving. A viral post does not give a two-vote comment the same reach.
A quiet direct answer may be useful in solution-request mode, but do not label
it a high-reach karma opportunity merely because the enclosing post is large. Timely mode starts with
fresh discussions; active-request mode starts with recent specific asks;
evergreen mode starts with the search queries the intended reader would use
and follows the threads those searches actually surface. Read the actual post
and leading and recent replies. Prefer a specific missing point the account
can answer. Record observed age, score and reply count separately from the
mode-specific ranking. An old busy thread with a settled answer is weak even
if it appears in search. If the operator supplies one exact thread, inspect it
directly; do not manufacture a broader search merely to fill a table.

Before ranking a comment target, read its leading and recent replies. Ask
what distinct point this account can add in one read. For timely discussion,
also inspect current stories its readers may recognize; verify any cross-story
callback and its timing. Age and activity describe possible reach, not the
value of our comment. Reject a draft that repeats the article, duplicates an
existing reply, or needs a paragraph explaining its reference. Do not force a
joke or callback when a straight answer would help more.

### Timely discussion opportunities

Scan a bounded set of fresh threads on subjects the account can speak to.
Rank a candidate only after reading its leading replies. First require a
specific new fact, answer, or timely connection that those replies lack. Then
compare age, visible votes, reply count, and whether new replies are still
arriving for both the post and the exact reply branch. Record both levels
separately in activity samples and candidate risk; do not borrow post-level
votes to score a low-visibility branch. Prefer an early live discussion with a clear point over an older
viral thread whose top replies have settled the conversation. A second
snapshot can show observed activity; one snapshot cannot establish growth.
Look for another current story only when a verified callback is immediately
clear to the thread's readers. Reject a factual summary that merely repeats
the linked article, a copied leading point, and a joke that needs explaining.
The draft should make its point in one read and stop. Before ranking
it for growth, name the natural next response it might invite from this
thread. If the only likely replies are thanks or agreement, call it a useful
answer, not a conversation seed. Do not tack on a question to manufacture one.

### Solution request opportunities

Search for posts where the author asks for a tool, service, workflow, or
approach, not posts that merely mention a product category. Read the exact
constraints, what the author has already tried, and current replies. Check
whether the question remains open, whether a suitable answer is already there,
and whether the community permits recommendations. Verify the capabilities
and limitations of any candidate option from current source material; compare
plausible alternatives instead of declaring one option best by default.

Both solution-request modes first require a verified fit and a missing point
in existing answers. Record the author's need in `contribution_gap`, the mode
and fit in `rationale`, and rule, evidence, and recommendation risks in `risk`.
Reject a weak fit, settled request, thread where naming a product would
interrupt the conversation, or answer that would require pretending to be an
independent customer. If a material relationship affects the recommendation,
describe it accurately or leave the product out. A useful answer may name a
different option or no product at all. Do not repeat a product name, link, or
invitation to DM to make the reply sound like a pitch.

#### Active solution requests

Rank the unresolved need and verified fit first. Among similarly good matches,
prefer a recent question with observed substantive replies and room for one
more useful answer. Use score, reply count and a second activity snapshot only
as evidence of possible near-term reach. A large but answered thread loses to
a smaller open request. Do not infer buyer interest from votes.

#### Evergreen search discovery

Start with the reader's durable question, not a product name. Search several
natural query variants and record the exact queries, observation time, and
which threads repeatedly surface. A thread's appearance in public search is
observed visibility for those queries, not a stable rank or proof that an AI
system will cite a future comment. If a current AI answer cites the thread,
record that exact observation; never project it to other systems or future
answers. Prefer threads whose title and original question match lasting search
intent, whose pages are accessible and indexable, and where a new reply would
be visible to someone reading the thread. Check whether the thread is open,
archived or locked, how replies are sorted, whether recent replies appear, and
whether existing answers have become outdated. A prominent thread with a
buried, redundant reply is a poor target.

Rank evergreen candidates by observed query visibility, durable problem fit,
verified solution fit, and the specific answer gap. Use age and votes as context:
an established older thread can beat a new quiet one, but an old thread with
settled answers or no usable reply surface cannot. State uncertainty about
search-engine indexing and AI retrieval. Do not promise traffic, ranking, or
citations. If the product misses a material requirement, reject the target
even when it appears in many searches. Keep search URLs and source links in
review evidence; Reddit draft text remains plain text.

Return `target_candidates` for the targets inspected. Mark each selected,
alternate or rejected; include source URLs and observation times, community
rule and account-eligibility status, bounded activity samples, the contribution
gap, reason and risk. Match each draft to a selected candidate's exact target. The default graph
checks that every draft has a selected candidate with allowed rules. A selected
candidate with unknown account eligibility yields `provisional`, retaining the
draft for review and stating the missing check. Disallowed rules or known
ineligibility return `held` with no drafts. `provisional` is never proof that
the account can submit; `publish` requires fresh logged-in checks.
Keep observed metrics separate from a judgment about likely discussion or
future discovery. For evergreen candidates, put the exact search queries and
observed result URLs in `observations` and `source_urls`, with the retrieval
limits in `risk`. A single snapshot does not prove a thread is growing: use
two timestamped reads if growth matters, or say it is unknown. Do not invent
a score, karma forecast, "best time", sticky outcome or future citation claim.
Recheck the selected target before any live submission because rules, thread
state and account eligibility can change.

Avoid disguised promotion, repeated near-duplicate posts, seeding replies from
another managed account and coordinated voting. If the operator is researching
a product problem, ask an honest problem question without hiding an affiliation
when it is material. Source links stay in review evidence outside Reddit text.

## Write in the account's voice

Read references/voice-and-learning.md. Prefer one or two short paragraphs.
A useful comment is usually under 80 words; a question post usually under
160. These are editing targets, not reasons to omit a necessary fact.

Make one main point or ask one answerable question. Give enough concrete
context for someone to respond without designing the whole experiment.
Before writing, state privately why a reader of this thread would care now.
Draft against the live conversation, not only the linked article. A correct
fact that changes nothing for readers is better held for a specific question
than posted as a top-level summary. If no natural, distinct response survives
that check, return no draft.
Use ordinary words. No forced slang, sales hook, grand framing, moral,
"mic drop", invented opinion, anecdote or supposedly clever closer.
Read the final sentence; remove it if it adds no fact, advice or genuine
response. Humor may come from a real detail, not a manufactured punchline.

When someone answers our question, respond to their contribution. Do not turn
their answer into an unsolicited review of their workflow, instructions for
improving it, or another explanation of our own process. A brief connection to
the operator's supplied experience can be enough. Avoid automatic agreement
and generic praise. Ask a follow-up only when it serves a genuine uncertainty;
a reply does not need to end with a question. See the follow-up examples in
references/voice-and-learning.md.

Apply the selected account's stored `voice.casing`. Never generalize one
account's preference to another or override an explicit request for sentence
case. Plain text goes into Reddit's editor.
Do not insert Markdown link syntax. Keep citations in review notes outside
the draft; include a bare URL only when explicitly requested. Facts need
sources when uncertain or current, but a factual citation is not evidence
that the account personally used a product.

For a post, lead with the actual problem or observation. Ask for a specific
experience: task, service, cost, limitation or result. Choose one primary
question. Don't pile gateway, budget, alternatives, architecture and spending
authority questions into a single introduction. Offer a real starting result
when available; never make one up. A stated budget or promise to run an
experiment requires operator intent and does not authorize spending.

## One approved submission

1. Research the actual target, current rules, eligibility and replies. Usefulness
   comes before activity metrics. Review the exact plain-text draft and target.
2. Call `publish` with the named account and exact action. The skill validates
   canonical subreddit/parent URLs, title/body, link policy and completed history.
3. The native connector approval binds the selected account, connector instance, target and exact content.
   Changed text or target requires fresh approval. An agent-authored `approved`
   field is never authority. This is the only approval gate for the write.
4. The connector checks rendered identity and editor state, enters the exact text,
   reads it back and pauses briefly before checking and submitting once.
5. Native runx records the acknowledgement and invokes a separate readback phase.
   The rendered author, content ID, community, parent and exact title/body must
   match. Only then is completed contribution history written.

Do not interpret a click, toast or filled editor as publication. Keep the native
provider operation, readback reference and contribution record together. If a write
succeeds but history persistence is interrupted, resume that original run so it
finishes recording the result; do not start a replacement publication run.

## Upvotes

`vote` takes the exact target, lowercase author and observed text. The skill refuses
self-votes and votes on other configured accounts. The connector independently
checks the rendered target author and text. It ensures the selected state is up;
an existing upvote is read back without toggling it. Never coordinate votes between
accounts or vote for reputation farming.

## Recovery and learning

Continue the original run with its unchanged inputs and connector instance. Native
custody resumes from recorded evidence: an acknowledged write is read back without
another submit; an unknown non-idempotent submit is held and cannot be resent.
Use bounded read operations to investigate the thread and exact permalink. Absence
from search is not proof that nothing was submitted. Never create a new run, change
accounts or rebind an instance to escape an unsettled operation.

There is no skill-level `reconcile` runner or custom release gate. Native runx owns
operation recovery. A refusal before execution is distinct from an uncertain effect;
report the original native status and keep its receipt. If native custody cannot
resolve an unknown effect, report that limit rather than clearing or ageing it out.

Record useful operator feedback and observed outcomes for future voice decisions;
do not turn a single vote count or removed comment into a confident rule.

## Invocation and composition

Inspect the selected runner before calling it. Attach and confirm the browser session as described in references/browser.md, then
configure the named account with its voice and explicit casing preference.
Then run `reddit` with its `account_id` and a bounded objective, such as
"read this thread and draft two useful replies; do not submit". All runnable
examples live in the manifest and use fictional account names.

The default output includes compared target candidates, observations, source
URLs, reviewable drafts, unknowns and the next step. Reuse supplied evidence when its target and age
are appropriate; disclose when it was supplied rather than freshly observed.
`publish` consumes one exact action, not an entire idea list. `accounts`
provides bounded discovery; `configure` manages local bindings. The store
owns durable event operations and concurrency. A connector credential profile
is resolved by that connector's native credential boundary, not this skill.
Brand-voice and taste-profile may supply approved context; this skill applies it
to Reddit. General writing may use `ghostwrite`, but Reddit account selection,
community admission and exact publication remain with this skill and its selected
connector. Do not rerun supplied fresh evidence or an already completed effect.

## Host task contracts

For `reddit-research`, do only the requested discovery/read/draft/diagnosis
work. Never submit. Load the account voice and browser instructions. Report what was
actually observed, compare post communities and comment threads when target
discovery is requested, include sources outside draft text, and return a `provisional` draft when
public evidence is enough but browser identity or eligibility is unavailable.
Return `needs_browser` only when required evidence cannot be obtained without
the browser, and `held` for disallowed rules or known ineligibility.

