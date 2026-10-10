# Sentry

This connector reads issues and individual events from Sentry Cloud through
native HTTP. The instance pins the organization and credential. It offers no
issue resolution, comment, assignment or project mutation.

Use a token with `event:read`. Bind it through Runx's credential surface; never
pass it in skill inputs or put it in this package. The runtime injects bearer
authentication and limits credential delivery to `sentry.io`.

`issues.list` returns a single page, bounded to 100 issues. This implementation
does not expose Sentry's response-header pagination cursor: a full page is
potentially incomplete, never an exhaustive issue inventory. A known supplied
cursor can select another page. Prefer `issue.read` and `event.read` for exact
issue investigation. `event_id: latest` resolves at call time; preserve the
returned actual event identity and observation time when admitting evidence.

An alert or issue snapshot does not prove root cause or recovery. Event payloads
may contain paths, headers, users and other private information. Admit only the
needed fields through the existing evidence/privacy boundary before model use
or outward disclosure. This connector does not claim to scrub event content.

Provider references: [issue read](https://docs.sentry.io/api/events/retrieve-an-issue/),
[project issues](https://docs.sentry.io/api/events/list-a-projects-issues/),
[issue event](https://docs.sentry.io/api/events/retrieve-an-issue-event/).
