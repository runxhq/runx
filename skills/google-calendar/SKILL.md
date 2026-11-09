---
name: google-calendar
description: Read a selected Google Calendar's bounded events or free/busy window through a governed connection, preserving coverage and time semantics for downstream planning.
---
# Google Calendar

Use this skill when a decision needs current events or busy intervals from a Google calendar the operator has explicitly selected. The default `events` method reads expanded event instances; `freebusy` reads only busy intervals. Both are read-only `google-calendar` connector operations with an exact calendar target and a positive, explicitly bounded time window. They cannot create, edit, delete, RSVP to, or invite anyone to events.

The portable skill owns Calendar evidence semantics, not credentials or connector selection. A local, self-hosted, third-party, or Runx-hosted connector is compatible when it implements the declared operations and scopes. Keep connection identifiers, credentials, tenant details and private calendar choices in the operator's binding and inputs, never in this package.

## Choose a bounded read

Authorize the intended Google account for `events.read` and/or `freebusy.read`. The hosted Google binding requests `calendar.events.readonly` for event details and `calendar.events.freebusy` for busy-only reads; Analytics or Search Console access does not grant Calendar access. Supply the exact calendar ID plus start and end instants with explicit UTC offsets. `primary` selects the connected account's primary calendar. For multiple calendars, run the selected IDs separately and preserve each result's coverage.

The `events` method expands recurrence and reads one result page. `calendar_id` defaults to `primary`; select another exact ID when needed. Choose `page_size` for the bounded operation (100 by default, up to Google’s 2,500-event page maximum), and use its returned cursor to read any remaining events in the same window. This is a per-call work budget, not a limit on the number of events or the duration you can inspect. Use `freebusy` when only availability is relevant, so event titles are not collected. A live method returns provider readback; `normalize-supplied` only normalizes already collected bytes and says `supplied_result`/`not_called`. Keep the prior receipt with that packet if it is to be treated as prior provider evidence.

## Interpret the evidence

The returned `calendar_context` binds calendar ID, window, observation time, recurrence and cancellation identity, all-day dates with exclusive end, transparency, self-response and busy intervals. Private event titles are removed. An inaccessible calendar, provider error, stale result or incomplete event page means unknown coverage, never a free slot. Event absence establishes nothing unless every selected calendar window is complete and fresh. An all-day event covers local dates through the day before its exclusive end; do not turn it into a guessed UTC meeting time. A cancelled recurring instance is not a live commitment.

The assistant may use complete current evidence for meeting preparation and attention timing. The `assistant-brief` method takes an exact scheduled goal occurrence, selected calendar and the calling host's actual `observed_at` instant with an explicit UTC offset, reads an upcoming bounded window, and returns the common operator work result. The window starts at the later of the occurrence due time and `observed_at`, so a wake after sleep reads the upcoming period. The host supplies this execution time; the deterministic worker never infers it from its own clock. Partial coverage holds that occurrence; it never means the calendar is free. A separate owning scheduling or conversation skill may propose times only from complete fresh coverage and private working-hour/buffer policy. This skill neither offers slots nor sends messages. Event mutations belong to a separately governed future capability.

## Recover and compose

If an event page is partial, pass its exact `next_page_token` as `page_token`, with the same calendar, time bounds and page size. Do not repeat the first page. The final resumed page has `page_complete: true` but still has partial window coverage: it does not contain earlier pages. Feed the ordered `calendar_context` values to `combine-pages` to recover whole-window coverage. That phase makes no provider call; it checks the cursor chain and window identity, deduplicates identical instances, and holds gaps, cycles or conflicting revisions. Keep the prior read receipts with these contexts. Google pagination does not provide an atomic snapshot; if instances change while paging, refresh rather than choosing a revision silently. The combined observation starts at the oldest page, so combining cannot make old evidence fresh. If a provider error or expired grant caused the gap, repair the binding and refresh the affected window. Do not add a mirrored event database, sync-token store, webhook loop or assistant-owned case for routine polling.

For example, read `primary` from `2026-10-04T00:00:00+11:00` through `2026-10-11T00:00:00+11:00`, then pass the context and receipt to the assistant. If `next_page_token` is present, continue the same query until the tail is complete, then combine the pages before concluding there are no other commitments. You may also choose separate narrower windows when that better fits the task, accounting for event overlap at their boundaries.

`normalize-supplied` accepts a prior provider result and the exact requested `page_token` for that result. It reuses the evidence without calling Google, labels it `supplied_result`/`not_called`, preserves every admitted event and busy interval, and validates event timing. Partial or malformed free/busy evidence cannot establish availability. Supplied-result reuse and page combination do not independently verify the source provider; carry the original receipts when the next workflow needs that assurance.

Event reads cover overlapping events: an event must end after `time_min` and start before `time_max`. Events already underway are included. These bounds are not a filter on event start alone.
