---
name: google-calendar
description: Read a selected Google Calendar's bounded events or free/busy window through a governed connection, preserving coverage and time semantics for downstream planning.
---
# Google Calendar

Use this skill when a decision needs current events or busy intervals from a Google calendar the operator has explicitly selected. The default `events` runner reads expanded event instances; `freebusy` reads only busy intervals. Both are read-only `google-calendar` connector operations with an exact calendar target and a window no longer than 30 days. They cannot create, edit, delete, RSVP to, or invite anyone to events.

The portable skill owns Calendar evidence semantics, not credentials or connector selection. A local, self-hosted, third-party, or Runx-hosted connector is compatible when it implements the declared operations and scopes. Keep connection identifiers, credentials, tenant details and private calendar choices in the operator's binding and inputs, never in this package.

## Choose a bounded read

Authorize the intended Google account for `events.read` and/or `freebusy.read`. The hosted Google binding requests `calendar.events.readonly` for event details and `calendar.events.freebusy` for busy-only reads; Analytics or Search Console access does not grant Calendar access. Supply the exact calendar ID plus start and end instants with explicit UTC offsets. `primary` selects the connected account's primary calendar. For multiple calendars, run the selected IDs separately and preserve each result's coverage.

The `events` runner expands recurrence within the selected window and has a finite page budget. Use `freebusy` when only availability is relevant, so event titles are not collected. A live runner returns provider readback; `normalize-supplied` only normalizes already collected bytes and says `supplied_result`/`not_called`. Keep the prior receipt with that packet if it is to be treated as prior provider evidence.

## Interpret the evidence

The returned `calendar_context` binds calendar ID, window, observation time, recurrence and cancellation identity, all-day dates with exclusive end, transparency, self-response and busy intervals. Private event titles are removed. An inaccessible calendar, provider error, stale result or incomplete event page means unknown coverage, never a free slot. Event absence establishes nothing unless every selected calendar window is complete and fresh. An all-day event covers local dates through the day before its exclusive end; do not turn it into a guessed UTC meeting time. A cancelled recurring instance is not a live commitment.

The assistant may use complete current evidence for meeting preparation and attention timing. The `assistant-brief` runner takes an exact scheduled goal occurrence and selected calendar, reads an upcoming bounded window, and returns the common operator work result. Partial coverage holds that occurrence; it never means the calendar is free. A separate owning scheduling or conversation skill may propose times only from complete fresh coverage and private working-hour/buffer policy. This skill neither offers slots nor sends messages. Event mutations belong to a separately governed future capability.

## Recover and compose

If a page cap makes an event window partial, narrow the requested window into disjoint smaller windows, read them under the same selected calendar authority, and merge only complete slices by event instance identity. Repeating the same capped query cannot prove completeness. If a provider error or expired grant caused the gap, repair the binding and refresh the affected window. Do not add a mirrored event database, sync-token store, webhook loop or assistant-owned case for routine polling.

For example, read `primary` from `2026-10-04T00:00:00+11:00` through `2026-10-11T00:00:00+11:00`, then pass the context and receipt to the assistant. If that read is partial, split the week into smaller bounded reads before concluding there are no other commitments.
