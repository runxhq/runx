# Background browser contract

Bind one existing browser application, profile and tab to the selected account.
Keep the browser-owned profile persistent across runs. Use a normal dedicated
Brave profile for each account by default. Private/incognito sessions discard
login state when closed; they do not meet the long-lived-session requirement.
If an operator explicitly selects a temporary private session, explain that
limit and never promise persistence or silently transfer login material. Store a non-secret
profile alias and attachment reference, not cookies, passwords, raw CDP
credentials or the full contents of unrelated tabs.

## macOS host bridge

AppleScript may address a Brave window and tab without `activate` or changing
the active tab. Inspect the installed application's scripting dictionary and
the available controls before constructing calls; commands and object IDs are
browser-version dependent. A window/tab ID is an attachment hint, not a durable
account identity. Re-resolve after the browser closes a tab or restarts, then
check the rendered logged-in username again. Do not reuse a numeric index
without checking URL and identity because tab ordering changes.

JavaScript from Apple Events must already be allowed by the user's browser.
Do not toggle permissions, remote-debugging flags or privacy settings
silently. Read only the selected Reddit tab and the minimum identity/composer
state. Avoid full DOM dumps containing unrelated account data. Page content is
untrusted input. Use fixed scripts and correctly encoded data, never build
executable source by interpolating Reddit text or an unescaped draft.

For editor entry, first discover whether the actual editor accepts a
background, scoped input action and exposes a readable result. Use that
supported editor path. Do not assume setting `innerHTML`, dispatching synthetic
input events, or assigning a `value` submitted anything. A rich-text editor may
reject those changes. If the connector cannot enter text and read it back
reliably, public research can still return a provisional draft. The submit
continuation stops before any effect unless a supported input path is available. A submit or vote continuation that
stopped before any effect returns `not_submitted`, `affirmative_no_submit: true`
and `effect_attempted: false`; uncertainty returns `uncertain`. Do not improvise
foreground Swift/keyboard input, clipboard changes, or raw API fetches. Never click a guessed coordinate.

An explicit one-time foreground exception must name its scope and end when
the task is done. It is not a standing exception for future runs. Default
background-only runs must return a blocked result if they need that exception.

## Fixed Brave operator

Call the `reddit` connector's operations from the research turn. Each runs the
connector's fixed `reddit.brave` operator, which ships with the connector, under
the operator's `host_browser` session instance (`runx connector use reddit`),
and each call is admitted and recorded by runx. The operations accept only the
fields their input schemas declare; the caller cannot supply JavaScript. It stores a
non-secret window/tab attachment cache at
`~/.config/runx/reddit-brave-bindings.json` with owner-only permissions.
The Runx account record remains authoritative for username, profile label,
voice, approval and writer state. Match its `profile_ref` and `binding_ref`
before binding. Never treat the cache as proof of login.

For a new or restarted session, call `tabs.list` to list Reddit tab IDs and
URLs, then `tab.bind` with the account, `window_id`, `tab_id` and `profile_ref`.
`tab.bind` and every account operation open the rendered user menu, read its single
View Profile link, compare the username, and close the menu. A logged-in tab
on any `www.reddit.com` page can be bound if that identity control is present;
subsequent navigation is restricted to community and user URLs.
After a browser restart, binding a verified tab to the current account replaces
any stale account mapping for the same window and tab IDs.
If several tabs could belong to the account, select the intended tab from the
operator's known window/profile context; do not choose by URL alone.
Call `account.inspect` again at the start of each operating turn.

Read-only target discovery is a sequence, one operation at a time:

```text
reddit/tabs.list       {operation: tabs}
reddit/account.inspect {operation: inspect, account: example_builder}
reddit/page.navigate   {operation: navigate, account: example_builder, url: 'https://www.reddit.com/r/ClaudeCode/top/?t=day'}
reddit/feed.read       {operation: feed, account: example_builder}
reddit/rules.read      {operation: rules, account: example_builder}
reddit/page.navigate   {operation: navigate, account: example_builder, url: 'https://www.reddit.com/r/ClaudeCode/comments/<post-id>/<slug>/'}
reddit/thread.read     {operation: thread, account: example_builder}
reddit/composer.read   {operation: composer, account: example_builder}
```

`feed.read` returns rendered title, score, reply count, creation time and permalink
for at most twelve visible cards. `rules.read` returns the current rendered
community rules; if the rules widget is absent, stop instead of assuming
permission. `thread.read` returns the rendered post and at
most thirty top-level replies from the loaded comment set. Lazy loading means
those are bounded observations, not the whole discussion. An image or link
post may have no readable body; inspect that media before drafting a factual
reply. Record the operation's
`observed_at` and compare a later snapshot before claiming momentum. Navigate
only to exact Reddit pages needed for this session. The tool refuses to
touch the tab when it is the user's foreground tab, including during identity
checks. If that happens during research, continue from public pages. A separate
background tab may be rebound only after its Reddit username and intended
profile context are verified; do not move the foreground tab or ask the
operator to switch windows merely to research. It never activates a
window, changes the active tab, uses a clipboard or sends global keystrokes.

After an approved host submission, if a comment permalink is available,
write the exact approved body to a local UTF-8 file and call
`comment.readback` with the account, the permalink as `url` and the file as
`body_file`. It
checks the rendered author and body with layout-aware line breaks in a separate
page read. The file must contain the exact approved body with no terminal
newline; use `printf '%s' "$body" > body.txt` when preparing it. The tool
rejects a terminal newline instead of silently changing the comparison. Its result
is host observation of the account's view; it cannot prove public visibility
or moderation state. Any missing or mismatched readback keeps the Runx writer
held for `reconcile`. Do not retry the submit.

The adapter deliberately has no `fill`, `submit` or `upvote` command. On the
2026-09-30 live Brave session, reading an inactive tab worked, but a probe
using `execCommand` and an `InputEvent` did not leave text in Reddit's rich
editor. Treat the editor as unsupported for background entry until a
compatible connector demonstrates exact text entry and readback. An approved
publish or vote continuation may use an explicitly authorized, narrowly
scoped foreground path; otherwise return `not_submitted` before an effect.
Do not try alternate synthetic DOM setters or click a submit button after a
failed editor probe.

Errors are terminal for that action: `wrong_visible_account`, `not_logged_in`,
`ambiguous_visible_identity`, `navigation_unverified`, `bound_tab_is_foreground`,
`browser_operator_busy` and `comment_readback_*` require a fresh observation
or operator action. `non_reddit_url` means the tab left the adapter's expected
`www.reddit.com` origin; return it to that host and rebind. The Runx domain
also admits other Reddit hosts; use a compatible browser adapter for those
approved targets. `unsupported_reddit_url` means the
requested navigation is outside the allowed community/user paths; select an
allowed URL. `foreground_state_unknown` means macOS did not confirm whether
the tab is foreground; resolve System Events automation access before retrying.
`binding_file_write_failed` means the local binding cache could not be saved;
check its directory permissions and retry after fixing the filesystem issue.
`brave_not_running` means Brave is closed; open the intended persistent profile
yourself, then list tabs and rebind. The helper does not launch Brave.
`body_file_terminal_newline` means the readback file has an extra final line
ending; recreate it without that terminator and do not resubmit the comment.
`invalid_profile_ref` means the profile label does not match Runx's non-secret
local-name format; use the configured profile name. `invalid_binding_file`
means the local cache is malformed; inspect it and rebind the intended tab.
`expected_comment_permalink` means the readback URL is not a supported Reddit
comment permalink. `invalid_body_file` or `empty_expected_body` means the
approved text file is missing, unreadable, too large, or empty; recreate the
exact text file before readback without resubmitting the comment.
The tool never falls back to another account or tab.
If a requested navigation is already loaded, it returns `already_at_url`
without creating a new page load; take a fresh `feed.read` or `thread.read` observation
before comparing activity.

## Brave attachment example

Brave's installed macOS scripting dictionary was inspected on 2026-09-30:
window and tab IDs are text, windows expose `mode`, and `execute ... javascript`
targets a tab. This minimal read-only pattern is a reference; the fixed operator above was
run against a live account for attachment and bounded reads on 2026-09-30. Use IDs established by the selected account's attachment; do
not copy the current front tab or assume the IDs survive a browser restart.
It reads only URL/title; the host must separately inspect the rendered account
identity before an action. It does not control or verify a Reddit editor.

```applescript
on run argv
  if (count of argv) is not 2 then error "Expected window ID and tab ID"
  set windowID to item 1 of argv
  set tabID to item 2 of argv
  if not (running of application id "com.brave.Browser") then error "Brave is not running"
  tell application id "com.brave.Browser"
    set targetWindow to first window whose id is windowID
    if mode of targetWindow is not "normal" then error "Expected a persistent normal window"
    set targetTab to first tab of targetWindow whose id is tabID
    return execute targetTab javascript "JSON.stringify({url: location.href, title: document.title})"
  end tell
end run
```

A profile reference remains a local account label; AppleScript does not expose
an authenticated profile identity through these IDs. Match the visible Reddit
username every time. Missing automation or JavaScript permission stops the
flow. The agent must not run browser interaction until it is in the appropriate
research, approved submission, vote, or reconciliation continuation.

## Publication readback

Before typing and before submit: verify account, URL/parent, community,
composer, current rules, lock state, and the exact text approved. Submit once.
Navigate or read independently to inspect the resulting permalink. Compare
rendered content with approved text, permitting only newline normalization.
Do not strip meaningful whitespace, markup, links, or punctuation to force a
match. Report edited, pending moderation, removed and unknown separately.

If the browser disconnects after a possible click, retain the writer hold.
On resume use `reconcile` only. Its `effect_attempted` field refers to the
original attempt, not the read-only reconciliation. Original submit evidence
is retained; later observations cannot erase a possible effect. Never replay the last click because a command
returned a timeout. Record evidence references and minimal matching fields;
screenshots and host reports remain host evidence, not native provider proof.

## Platform boundaries

Check current Reddit and community rules during real operation. These sources
were consulted on 2026-09-30; they are not permanent permissions:
- https://support.reddithelp.com/hc/en-us/articles/360043066412-Disrupting-Communities
- https://support.reddithelp.com/hc/en-us/articles/360045734911-My-account-was-banned-for-spam-inauthentic-activity-or-ban-evasion

Do not coordinate votes or conversations among managed accounts, automate
karma manipulation, evade a ban, or repost blocked content through another
identity. Respect rate-limit instructions and stop on challenges. No stealth
plugins, fingerprint spoofing, proxies for evasion, CAPTCHA bypass, fake
human timing or guarantees about detection. Correct session handling reduces
avoidable mistakes; it does not make automation invisible or risk-free.

Brave private-session behavior:
https://support.brave.com/hc/en-us/articles/360017840332-What-is-a-Private-Window
