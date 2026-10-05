# Local personal assistant

`runx assistant` runs one finite, bounded turn in Runx OSS. The CLI hosts the
schedule and pins configuration; existing skills own Slack and Nitrosend reads,
the operator inbox, data-store state, attention judgment, and notification
delivery. The operator pins one exact model and OpenAI-compatible endpoint in
the private profile, either a loopback server or an HTTPS API. The assistant
never selects a model or falls back to another one. Runx does not start or
download a model. No resident Runx process is needed: an optional macOS launchd job invokes
`tick`, and the persisted next-due time prevents unnecessary provider or model
calls.

The profile is a private JSON file (`chmod 600`). Keep account identifiers and
the exact audience there, outside the OSS skill package:

```json
{
  "schema": "runx.assistant.profile.v1",
  "instance_id": "personal",
  "skills_root": "/absolute/path/to/runx/oss/skills",
  "data_source_ref": "local://runx-data-store/personal-assistant",
  "inbox_data_source_ref": "local://runx-data-store/operator-inbox",
  "sources": [
    {
      "kind": "slack_mentions",
      "source_id": "mentions",
      "query": { "mentions_connected_subject": true, "limit": 10 }
    },
    {
      "kind": "nitrosend_inbox",
      "source_id": "mailbox",
      "brand_sid": "brnd_example",
      "arguments": { "view": "full", "page": 1, "per": 10 },
      "credential_profile": null
    }
  ],
  "allowed_action_ids": [],
  "work_routes": [
    {
      "route_id": "pr_status",
      "kind": "github_pr_status",
      "repositories": ["example/project"],
      "credential_profile": null
    },
    {
      "route_id": "intake",
      "kind": "conversation_review",
      "repositories": [],
      "credential_profile": null
    },
    {
      "route_id": "planning",
      "kind": "work_plan",
      "repositories": [],
      "credential_profile": null
    }
  ],
  "charter": "Prioritize direct requests requiring a decision; keep routine status low priority.",
  "confidential_terms": [],
  "model": {
    "model": "your-served-qwen2.5-id",
    "endpoint_url": "http://127.0.0.1:1234/v1/chat/completions",
    "auth_mode": "local_none",
    "max_rounds": 8
  },
  "notification": null,
  "heartbeat_seconds": 300,
  "min_check_minutes": 15,
  "max_check_minutes": 60,
  "quiet_hours": { "start_hour": 22, "end_hour": 8 }
}
```

`auth_mode` defaults to `local_none` for existing profiles. That mode requires
an exact loopback chat-completions URL and sends no model API key. To use a
remote OpenAI-compatible model, set the exact model ID, its HTTPS
chat-completions URL, and `"auth_mode": "api_key"`; supply the key explicitly
through `RUNX_AGENT_API_KEY`, never in the profile. The assistant does not
infer a remote model key from the general Runx agent credential store. The
same assistant queue, skills, grants, and receipts run with either
mode. Changing the configured model or endpoint changes the profile revision
and requires an explicit resume before another turn.

The source-page limits must total at most 20. Nitrosend sources require
`view: "full"`: each mailbox row is followed by a bounded `get_thread` read,
and the row's conversation identity and timestamps must match that read before
the assistant records the provider's actual message ID. Each turn reads one
bounded page per source. It pins the normalized pages and review in private,
digest-bound artifacts before advancing operator-inbox scan cursors. After a
crash or a caught error, the same pinned pages replay; an unpinned legacy turn
restarts at page one. Operator-inbox stores a continuation when another page
exists, and the next due turn advances that scan. While `coverage_incomplete`
is true, a brief does not represent a complete mailbox or Slack audit. The
assistant interleaves a bounded page-one refresh when twice the minimum check
interval has elapsed since the last first-page read. Refresh and backlog use
separate scan heads in `operator-inbox`, so fresh messages can be noticed
during catch-up without resetting the older cursor. One turn still reads only
one page per source, and a refresh remains partial when more pages exist.
`status.last_fresh_scan_at_unix_seconds` reports the last committed first-page
read by configured source ID. The
20-observation limit is an admission ceiling, not a promise that every pinned
model can make a useful tool call at that size. In live Qwen 2.5 testing, a
10-plus-10 source page exhausted the empty-turn retry budget; 5-plus-5
completed the attention pass. Set per-source limits for the configured model
and confirm them with a real trial; the assistant never switches models itself.
The provider's cursor or page can become stale as the source changes, and a failed
continuation holds the turn for inspection. Independent cross-source checks
are still needed before autonomous follow-up actions.

Use `runx assistant check --profile <path>` to validate the profile and installed
skill packages. `profile_valid` checks model configuration and remote key
presence, but cannot certify credential validity, notification authority, or
endpoint health. `runx assistant resume --profile <path>` enables
turns; `tick` runs one due turn, and `status` reports control and timer state.
`pause` prevents future turns. A profile configuration change stops active
ticks until an explicit resume. A skill-package patch does not change that
revision: new work uses the newly inspected package, and an already started
native run retains a content-addressed local package copy for exact resume.
The binding includes local sibling packages reached by any runner in the
inspected closure. A sibling patch gives new work a new binding while the
existing run keeps its exact earlier closure; a changed pinned copy is refused.
Each assignment pins its exact invocation inputs and run identity before
calling a skill; composed conversations retain separate bindings for review
and coding intake. The original copy must remain available while its run is
resumable. A pending
exact notification or pinned source/review turn cannot be rebound across a
profile configuration change.
After updating the Runx executable itself, reinstall an existing assistant
timer so its owned binary snapshot runs the new host logic.
`runx assistant report --profile <path>` returns the last persisted, validated
attention packet with its review receipt and the five most recent completed
non-mutating assignments. It reports `not_available` until a turn has committed a
review. `status.report_available` indicates whether that artifact exists. A
partial source scan remains marked `coverage_incomplete`; the report is a
snapshot of that review, not a fresh source read.

The private `charter` is bounded to 2000 bytes and binds to the profile revision.
Confirmed memory is separate from that file: `runx assistant remember --profile
<path> --memory-id <id> --text-file <private-file>` appends an operator-confirmed
entry to the existing local `data-store` control stream. The text file must be
private (`chmod 600`) and contain 1–300 bytes. `memories` reads the confirmed
entries; `forget --memory-id <id>` removes one. Status reports only the count.
Memory changes are rejected during pending work and cause the next scan to
revisit current observations. Source messages and model guesses never become
confirmed memory automatically. The review skill receives the charter and
confirmed entries as typed context alongside its own package manual and the
current source evidence; it may use context to rank observations, but its
brief remains bound to actual source references.
`forget` removes an entry from current context; the append-only data-store
audit history retains prior events until the local source is separately erased.

`runx assistant work --profile <path>` reads one bounded page of the canonical
`operator-inbox` action queue with a receipt. Each observed thread has stable
identity and explicit open/waiting/followed-up/resolved/dismissed state there.
Pass its `next_cursor` back with `--cursor` to read the next page.
The assistant does not create a parallel source-action database or infer
completion from email text. The model proposes a bounded next check and the
local control stream persists its due time; the heartbeat invokes finite `tick`
and `execute` roles. A turn with changed evidence may also select up to
three exact non-mutating work candidates. `github_pr_status` makes an exact
provider read: the host extracts canonical PR links from observed summaries, intersects them with the
private repository allowlist, and asks the model to select exact
source/route/target triples. The finalizer rejects invented targets and
non-open source actions. The host records a stable assignment in `data-store`
before `assistant execute` runs one due `github-sync#pull`. It verifies the exact PR
identity in provider readback, records state and receipt, and makes that
bounded result available in `assistant work`. A completed assignment wakes the
intake loop immediately. Before reading another provider page, the loop turns up
to three sealed work results into one private progress update. This is a
deterministic projection of the result, not another model judgment or work
queue. Read retries reuse the same native run identity
after a crash; they cannot post or mutate the PR. Intake continues while the
worker is busy. When attention selects an open mail or Slack item and the
profile enables `conversation_review`, the host queues that review from the
exact observed thread; the model does not have to propose the same review a
second time. The worker rereads one exact recorded Slack or Nitrosend
thread through its owning skill, checks its identity against the current open
operator-inbox action, and calls `conversation-review` with the private charter
and confirmed context. Ordinary correspondence produces an unsent draft or
an unscheduled follow-up proposal; the current worker does not execute it.
Only a complete thread classified as a real coding request,
with a verbatim request quote, enters `issue-intake`; its change set may then
enter `work-plan`. The source and decision receipts and bounded result appear
in `assistant work`; recent work appears in `report`.
Slack context retains message order, authors, and timestamps across up to three
bounded thread pages. Slack search and thread previews can render markup differently, so the live
read is matched by tenant, message locator, occurrence time, author, and the
canonical inbox action rather than by comparing the preview strings. An
incomplete bounded thread, truncated mail body, older omitted mail messages, or
unread attachments yield `needs_context` from `conversation-review`; they cannot
enter coding intake or an outward reply.
The available mail window is passed as bounded conversation context, with its
omission count retained. An optional `work_plan` route queues one child of a
completed coding intake when the source is complete and the exact change set says
`recommended_lane: work-plan`, `commence_decision: approve`, and
`action_decision: proceed_to_plan`. The next finite worker turn invokes the
existing `work-plan` skill with that unchanged parent change set and the pinned
model. It checks preservation evidence and retains the bounded plan and receipts
in `assistant work`. This is planning only; repository edits, publication, and
replies remain separate governed host work. Completed assignments are deduplicated by
source occurrence, route, and target. `assistant work` shows
both canonical inbox actions and these bounded execution assignments.

Additional unattended routes require an explicit typed target mapping and
configured authority. A model proposal alone never expands the roster or
creates a provider write. `work_routes: []` disables delegated work.
Two distinct source occurrences that link the same PR can currently produce
two reads. Both are bounded and read-only; deduplicating them across occurrences
requires a freshness rule so a later request can still trigger a new check.
Delivery and assignments have separate persisted due times. A blocked or quiet
private notification does not prevent a due non-mutating assignment from running
on the next finite worker wake. A transient assignment failure receives its
own bounded retry time, so a different due assignment can run first. A sealed
failed skill run is held with its receipt; the worker does not retry that
terminal native run ID indefinitely. Each new package digest gives an
assignment a distinct native run identity. A held read-only or planning
assignment is eligible for a new attempt when its failed skill package changes;
the prior receipt remains in the append-only control history.
If a bound skill asks the host for a decision, the assignment becomes
`awaiting_resolution` with the original native run ID and exact pending-request
artifact. `assistant execute` checks that native continuation on its due wake;
it does not call the model again while the request is unresolved. The operator
can use the native `runx resume <run-id> <answers.json|->` path for an exact
decision. Once Runx closes that run, the next worker wake consumes its result
under the original input and package binding. `assistant work` shows request
IDs and the run ID, but omits private artifact paths and bound input references.
`status.awaiting_resolution_count` separates these assignments from due work.
Native graph checkpoints retain terminal receipt identity for closed, blocked,
deferred, and failed runs; a repeated bound invocation replays that sealed
outcome. The terminal checkpoint includes the sealed receipts and is synced
before receipt files are written, so a retry can finish receipt persistence
without executing the graph again. Rejected or digest-mismatched continuation
answers leave the original request pending. Checkpoint replacement syncs the
file and parent directory.
This does not yet provide a Slack decision adapter or authorize an outward send.

Notification is off when `allowed_action_ids` is empty and `notification` is
`null`; in that mode a useful attention brief closes as `ready_undelivered`
and remains available in `report`. Its source occurrences are marked handled
so an unchanged scan does not call the model again. Completed work is recorded
as `work_available_undelivered` with its sealed receipts, and remains visible
in `assistant work`; it is not reported as sent. To enable delivery, configure
`private_update` plus an exact
private `slack://workspace/channel` target, provider grant, principal, expiry,
and post/text quotas; install its native standing authority with `grant`.
Delivery uses a persisted exact intent, a stable graph run identity and
idempotency key, the delivery skill package pinned when the intent is created,
native permission checks, and provider readback. A dispatched
intent remains pinned if delivery fails or its outcome is unknown. Repeating
the same explicit graph run ID recovers its completed checkpoint and signed
receipt without executing the delivery step again; missing or invalid completion
evidence holds the run for inspection. The host does not mint a replacement
identity or silently send again. `tick` pins the intent and `execute` delivers
it; source intake continues while delivery is pending. This private control
post uses exact standing authority. Outward messages to other people require
their owning approval. A notification from a partial scan carries an explicit
coverage caveat in the exact posted text. `revoke` disables the standing
authority. Quiet hours defer a pending notification; read-only checks can
continue during quiet hours even while notification is pending.
The local host projects each selected item into a short, plain-text notification
with its full source digest, or the sealed receipt for completed work, within
the channel's byte quota. The complete
evidence-bound review, including exact source locators, remains available through
`report`; sealed assignment results remain in `assistant work`. A work update
is marked delivered only after provider readback matches its pinned intent.
If even the digests cannot fit, delivery holds without posting. Source
markup and active Slack mentions are removed from the notification so a quoted
message cannot change the posted text or ping others. Source locators are kept
out of the Slack post because Slack auto-links even code-formatted locators and
changes the exact text required by provider readback. The final rendered text is
checked for protected local paths, credential material, system variables and
private profile terms before intent creation and again before delivery.
Every composed assistant skill run also carries an assistant-only native
`provider.mutate` rule. If a skill omits its own exact approval request, the
provider effect requests one human decision before dispatch; an existing skill
approval remains the single gate. A paid-job authority cannot satisfy that
assistant human gate. The same confidentiality check covers provider-bound
metadata, scopes, target, and outgoing payload before effect admission. A missing or
malformed private-term policy refuses the mutation. This applies to native
provider mutations; direct MCP writes are not an admitted assistant work route
in this version and require an owning delivery check before being configured.

If a pinned notification is blocked before native reservation, pause the profile
and run `discard-notification`; Runx checks the exact idempotency key in its
authority ledger before allowing the discard. Resuming then rescans from page
one and constructs a new intent. A reserved intent cannot be discarded this way
because its provider outcome may need reconciliation.

On macOS, `install-timer` installs two owned launchd jobs for intake and work,
and snapshots the exact
Runx and JavaScript-worker binaries into the instance directory. `remove-timer`
removes those jobs and only their owned files. Each timer wakes at the configured
heartbeat; actual work can begin one heartbeat plus OS scheduling delay after
the persisted due time. Start with manual read-only ticks and inspect receipts
before installing a timer or enabling delivery.
