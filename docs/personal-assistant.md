# Local personal assistant

`runx assistant` runs one finite, bounded turn in Runx OSS. The CLI hosts the
schedule and pins configuration; source skills own provider reads and translation,
the operator inbox, attention judgment, and notification delivery. The assistant
stores its own control, obligation, occurrence, and confirmed-memory records in
the protected workspace kernel store. The operator pins one exact model and OpenAI-compatible endpoint in
the private profile, either a loopback server or an HTTPS API. The assistant
never selects a model or falls back to another one. Runx does not start or
download a model. No resident Runx process is needed: an optional macOS launchd job invokes
`tick`, and the persisted next-due time prevents unnecessary provider or model
calls.

The profile is a private JSON file (`chmod 600`). Keep account identifiers and
the exact audience there, outside the OSS skill package. Its revision is the
mandate reference on delegated runs; an unsettled call cannot resume under a
different revision:

```json
{
  "schema": "runx.assistant.profile.v1",
  "instance_id": "personal",
  "skills_root": "/absolute/path/to/runx/oss/skills",
  "inbox_data_source_ref": "local://runx-data-store/operator-inbox",
  "authority": {
    "actor_ref": {"type": "principal", "uri": "runx:principal:assistant:personal"},
    "delegations": [{
      "principal_ref": {"type": "principal", "uri": "runx:principal:assistant:personal"},
      "resource_ref": {"type": "connector_instance", "uri": "runx:connector_instance:slack-personal"},
      "resource_family": "connector",
      "verbs": ["read"],
      "capabilities": [],
      "bounds": {
        "operations": ["slack/messages.search", "slack/thread.read"],
        "scopes": ["messages.search", "thread.read"],
        "targets": ["slack://messages", "slack://WORKSPACE/*"]
      },
      "expires_at": null
    }]
  },
  "sources": [
    {
      "source_id": "mentions",
      "skill": "slack",
      "provider": "slack",
      "triage_kind": "direct_mention",
      "locator_prefix": "slack://WORKSPACE/",
      "config": { "query": { "mentions_connected_subject": true, "limit": 5 } },
      "page_limit": 5,
      "local_provider": null
    }
  ],
  "allowed_action_ids": [],
  "goals": [
    {
      "goal_id": "correspondence",
      "purpose": "Review direct requests and preserve actionable follow-ups.",
      "completion_condition": "Each admitted request has an evidenced disposition.",
      "enabled": true
    }
  ],
  "work_routes": [
    {
      "route_id": "conversation",
      "goal_id": "correspondence",
      "skill": "conversation-review",
      "runner": "operate",
      "description": "Review one complete conversation and propose its next work.",
      "source_kinds": ["chat"],
      "model": true,
      "inputs": { "handoff_routes": {} },
      "source_context_input": "source_context",
      "turn_id_input": null,
      "local_provider": null
    }
  ],
  "charter": "Prioritize direct requests requiring a decision; keep routine status low priority.",
  "confidential_terms": [],
  "context_budget_bytes": 49152,
  "model": {
    "model": "your-served-qwen2.5-id",
    "endpoint_url": "http://127.0.0.1:1234/v1/chat/completions",
    "auth_mode": "local_none",
    "context_sharing": "local_only",
    "max_rounds": 8
  },
  "notification": null,
  "decision": null,
  "heartbeat_seconds": 300,
  "min_check_minutes": 15,
  "max_check_minutes": 60,
  "quiet_hours": { "start_hour": 22, "end_hour": 8 }
}
```

Replace `slack-personal` with the selected connector instance. The assistant
acts as its own principal; each provider operation requires an operator-authored
delegation for that exact instance. Runx mints and proves the delegation from
the instance's live capability at admission. An absent or insufficient
delegation refuses the call, including reads. A notification or another source
needs its own bounded entry; approval of a post does not supply delegation.

A standing goal may declare a `schedule` with `starts_at_unix_seconds`,
`interval_seconds` (300 seconds to 30 days), an eligible `route_id`, and a
fixed `target_ref`. The route must accept `scheduled_goal` and cannot require
conversation source context. On a due tick, the operator admits the latest
aligned occurrence as a normal obligation and advances its schedule head in
the same conditional SQLite event append. A repeated wake cannot admit the
same occurrence twice. If the machine sleeps through several intervals, this
routine check coalesces them into one current occurrence; an unresolved active
occurrence runs first. The selected skill receives its configured inputs and
scoped goal context through the existing native run. Scheduling does not grant
the route any extra provider authority or bypass approval for outward actions.

`auth_mode: local_none` requires
an exact loopback chat-completions URL and sends no model API key. To use a
remote OpenAI-compatible model, set the exact model ID, its HTTPS
chat-completions URL, and `"auth_mode": "api_key"`; supply the key explicitly
through `RUNX_AGENT_API_KEY`, never in the profile. The assistant does not
infer a remote model key from the general Runx agent credential store. The
same assistant queue, skills, grants, and receipts run with either
mode. `context_sharing` declares whether scoped private context stays local or
may reach the configured model endpoint. Changing the configured model or endpoint changes the profile revision
and requires an explicit resume before another turn.
`context_budget_bytes` bounds assistant-supplied context (8 to 128 KiB). The
mandate and current obligation must fit; then matching confirmed memories and
verified history are selected newest first. Attention review receives the
declared route roster; each actual work run loads its own pinned skill
procedure. A scan does not load every work skill manual.

The source-page limits must total at most 20. Each configured source binds one
skill, provider name, non-overlapping locator prefix, private configuration,
and queue triage kind. The source skill implements three runners:
`assistant_scan(config, page_limit, cursor?)` returns the declared
`runx.conversation_page.v1` packet with bounded `items` and `next_cursor`;
`assistant_observe(config, item)` returns the declared
`runx.conversation_observation.v1` packet with `source_kind`, `summary`, and
`message`, where `message`
uses the existing `operator-inbox` message shape; and
`assistant_hydrate(config, source_ref, source_digest, thread_locator, action, cursor?)`
emits `assistant_context` as `runx.source_packet.v1`; its workflow inputs carry
`source_ref`, `source_digest`, `thread_locator`, `source_message`, `title`,
`body`, `context`, `complete`, and `next_cursor`. A hydration
page may omit `source_message` until the selected occurrence appears. The host
validates bounds, provider and locator binding, occurrence time, tenant,
connected subject, and author against the canonical open action. The source
skill owns provider-specific freshness and content checks. For example, the
Nitrosend adapter grounds each full mailbox row with a bounded thread read;
those mail details do not live in the assistant host. External packages may
implement the same three runners through an exact `skill_paths` binding.
Calendar and other periodic work enter through scheduled goal routes and the
shared `runx.work_outcome.v1`, not through a fabricated conversation.
Each turn reads one
bounded page per source. It pins the normalized pages and review in private,
digest-bound artifacts before advancing operator-inbox scan cursors. Pinning a
source page also records each signal's identity and page provenance in the
protected workspace store in the same transaction. A handled disposition
advances that signal's stream with the assistant state; repeat scans consult
the durable signal state for the configured profile revision. After a
crash or a caught error, the same pinned pages replay; an unpinned turn
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
10-plus-10 source page exhausted the empty-turn retry budget. A five-item
mailbox first page completed, but its second page produced malformed tool-call
JSON on every retry even after raising the local server's output cap. Two
consecutive three-item pages completed with six distinct queued actions and
no notification. Set per-source limits for the configured model and confirm
them with a real trial; the assistant never switches models itself.
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
resumable. A pending source/review turn keeps its profile binding. A pending
notification keeps its exact destination, adapter, inputs and run identity;
other source bindings may be revised after an explicit resume without
rewriting or retrying that notification.
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
entry to the protected assistant memory stream. The text file must be
private (`chmod 600`) and contain 1–300 bytes. `memories` reads the confirmed
entries; `forget --memory-id <id>` removes one. Status reports only the count.
Memory changes are rejected during pending work and cause the next scan to
revisit current observations. Source messages and model guesses never become
confirmed memory automatically. The review skill receives the charter and
confirmed entries as typed context alongside its own package manual and the
current source evidence; it may use context to rank observations, but its
brief remains bound to actual source references.
`forget` removes an entry from current context; the protected event history
keeps the earlier events.

`runx assistant work --profile <path>` reads one bounded page of the canonical
`operator-inbox` action queue with a receipt. Each observed thread has stable
identity and explicit open/waiting/followed-up/resolved/dismissed state there.
Pass its `next_cursor` back with `--cursor` to read the next page.
The assistant does not create a parallel source-action database or infer
completion from email text. The model proposes a bounded next check and the
protected control stream persists its due time; the heartbeat invokes finite `tick`
and `execute` roles. A turn with changed evidence may also select up to
three exact non-mutating work candidates from the profile's declared work
routes: the model selects exact source/route/target triples, and the finalizer
rejects invented targets and non-open source actions. The host records a stable
assignment in the protected assistant obligation stream before `assistant
execute` runs the route's declared skill runner, records its state and receipt,
and makes that bounded result available in `assistant work`. A completed assignment wakes the
intake loop immediately. Before reading another provider page, the loop turns up
to three sealed work results into one private progress update. This is a
deterministic projection of the result, not another model judgment or work
queue. Read retries reuse the same native run identity
after a crash; they cannot write to the provider. Intake continues while the
worker is busy. An admitted route binds an exact skill, runner, goal and source
kind. The worker hydrates selected source context through the configured source
skill, pins the inputs and skill closure, and passes the private charter,
confirmed memory and bounded related history through the native context
envelope. A typed `work_result` can satisfy, hold or admit a successor in the
same conditional state transition that acknowledges its parent. A reply route
may prepare an exact outward draft and pause at native approval. After an
authenticated decision, the same native run executes the bound effect and
checks provider readback. A queued mail reply schedules a later status read;
provider acceptance alone does not satisfy delivery. Coding work enters
`issue-intake` and `work-plan` only through configured routes for genuine
repository requests. The source and decision receipts and bounded result appear
in `assistant work`; recent work appears in `report`.
Slack context retains message order, authors, and timestamps across up to three
bounded thread pages. Slack search and thread previews can render markup differently, so the live
read is matched by tenant, message locator, occurrence time, author, and the
canonical inbox action rather than by comparing the preview strings. An
incomplete bounded thread, truncated mail body, older omitted mail messages, or
unread attachments yield `needs_context` from `conversation-review`; they cannot
enter coding intake or an outward reply.
The available mail window is passed as bounded conversation context, with its
omission count retained. Nitrosend reply context carries the conversation ID
and provider freshness digest; the product validates them again when the exact
approved reply is applied. Routes marked `successor_only` cannot be selected
as new work by the attention model; only a sealed parent result can admit them.
Completed assignments are deduplicated by source occurrence, route and target.
`assistant work` shows canonical inbox actions and execution assignments.

Additional unattended routes require an explicit typed target mapping and
configured authority. A model proposal alone never expands the roster or
creates a provider write. `work_routes: []` disables delegated work.
Two distinct source occurrences that name the same target produce two reads.
Both are bounded and read-only.
Delivery and assignments have separate persisted due times. A blocked or quiet
private notification does not prevent a due non-mutating assignment from running
on the next finite worker wake. A transient assignment failure receives its
own bounded retry time, so a different due assignment can run first. A sealed
failed skill run is held with its receipt; the worker does not retry that
terminal native run ID indefinitely. Each new package digest gives an
assignment a distinct native run identity. A held read-only or planning
assignment is eligible for a new attempt when its failed skill package changes;
the prior receipt remains in the obligation history.
If a bound skill asks the host for a decision, the assignment becomes
`awaiting_resolution` with the original native run ID and exact pending-request
artifact. `assistant execute` checks that native continuation on its due wake;
it does not call the model again while the request is unresolved. The
configured decision skill presents the exact native request and returns an
authenticated, request-bound observation. Slack is one implementation; the
native `runx resume <run-id> <answers.json|->` path remains available for an
exact local decision. Once Runx closes that run, the next worker wake consumes its result
under the original input and package binding. `assistant work` shows request
IDs and the run ID, but omits private artifact paths and bound input references.
`status.awaiting_resolution_count` separates these assignments from due work.
When an acknowledged provider write is waiting for independent readback, the
native graph retains its completed write step and emits a bounded `retry`
request. The operator answers that exact request on a later due wake and
restarts only the readback step. For local HTTP mutations, a complete provider
acknowledgement is persisted before readback so an immediate readback failure
can likewise resume without resending. A timeout before that acknowledgement
is saved remains an unknown effect and requires explicit reconciliation; it is
never treated as permission to issue a second write.
Native graph checkpoints retain terminal receipt identity for closed, blocked,
deferred, and failed runs; a repeated bound invocation replays that sealed
outcome. The terminal checkpoint includes the sealed receipts and is synced
before receipt files are written, so a retry can finish receipt persistence
without executing the graph again. Rejected or digest-mismatched continuation
answers leave the original request pending. Checkpoint replacement syncs the
file and parent directory. Every outward effect remains behind the exact
human approval request even when private progress posts have standing authority.

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
If the provider posted but did not return a matching acknowledgement, inspect
the exact message and use `reconcile-notification --message-locator <locator>`.
That command verifies the pinned failed run and read-only provider result before
acknowledging the original intent. An unreserved failed intent can instead be
discarded while paused; a dispatched or unknown intent cannot be discarded.
An explicit `grant` after a source binding change renews the same authority ID
without resetting its expiry or spent quota, provided no reservation remains
unresolved. Hosted credential authentication happens before Runx records a
possible dispatch, so a failed preflight cannot consume a notification slot.
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
Every composed assistant skill run also carries an assistant-only rule for
connector writes. If a skill omits its own exact approval request, the kernel
requests one human decision before dispatch; an existing skill approval
remains the single gate. A paid-job authority cannot satisfy that
assistant human gate. The same confidentiality check covers provider-bound
metadata, scopes, target, and outgoing payload before effect admission. A missing or
malformed private-term policy refuses the mutation. This applies to every
connector write; direct MCP writes are not an admitted assistant work route
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
