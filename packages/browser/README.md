# runx browser connector

Add `browser/page.open`, `browser/page.read` and `browser/page.act` to the
agent step that works a site. The browser works as a person's does: pages run
their scripts, follow redirects, open popups and tabs, and sign in through a
provider when the profile is signed in there. The agent sees the URL,
accessibility tree and fresh control references on each call, and can follow
a link, click, fill, choose an option, press a key, hover or scroll, a submit
button, Enter in a search box and a "Sign in with ..." button included. Each action's result lists what it sent
that changes something on a site, with the site's status and any
confirmation address it gave (`sent`). An observation's `forms` give each
form's destination and method and each field's current value, so a caller
can check a form holds exactly what it means to send before clicking it. If the page exceeds the response
limit, pass its `next_offset` and `snapshot` to `page.read`. If a page or
account changes, inspect it again before issuing an action.

Use `web/page.read` or the existing search connector for discovery and static
pages. A browser binding supplies a browser, and the sites it may reach when
it names them, for one task. A host selects the binding on the machine it
runs runx on and calls the same operations as a local agent. The host keeps
the task, and runx keeps authority, decisions and receipts.

## Start a browser

There is nothing to start. Load `connectors/browser` into the runx project
and bind it with
`runx connector use browser --session --parameter profile=sources --scope browser.read`.
A binding's first call starts its executor, `runx-browser` from this package
(on `PATH` once the package is installed, else through `npx`), in a session
of its own so it outlives the call. It stops once nothing it holds is open:
no run's context and no call. A run's context lasts until the run seals,
through a wait on a person included. The next call starts it again. It
uses Playwright's pinned Chromium, else this machine's Chrome, else it
installs the pinned Chromium on its first start.

The executor needs Node.js 22 or newer and uses Unix-domain sockets, so it
runs on macOS and Linux. Native Windows support is pending.

A binding started this way has no `site`: it goes wherever a person's
browser goes, apart from loopback, private, link-local and metadata
addresses, which it reaches only by listing them. A binding that lists
sites, brings a signed-in profile, opens a window or attaches over CDP runs
from a config. Save it as `browser.json`

```json
{
  "binding": "sources",
  "backend": "local",
  "headless": true
}
```

and run `runx-browser browser.json` (`node packages/browser/src/host.mjs
browser.json` from the runx workspace) as a supervised process. The connector
uses the executor it finds running and starts none.

```yaml
calls:
  operations: [browser/page.open, browser/page.read, browser/page.act]
```

Headless, the executor runs full Chromium without a screen and shows sites
the browser it is: the user agent the same Chromium sends from a window, no
automation flag, and a desktop window and screen of the machine's kind.
Set `headless` to `false` for a window. Set `channel` to `chrome` to use
installed Chrome, or set `executablePath` to an installed Chromium-compatible
browser such as Brave. One executor serves each binding. runx gives
each root run an isolated browser context, shared by its child skills.
Separate runs have separate pages, cookies, and web storage, and can operate
concurrently. The executor closes a context when its root run seals, after
any browser operation in progress returns. An open run, including one
waiting on a person, keeps its context while the executor stays alive.
Each binding admits 16 resident contexts by default. Set `maxContexts` to a
value from 1 to 64 for its host. At capacity an existing context stays
usable and new browser roots are refused until a context is freed.
Use the matching runx executable on the executor's PATH or set
`RUNX_BROWSER_RUNTIME` to its absolute path. The executor uses it to read
committed run heads from the local workspace store through one reusable
native reader while there are contexts to reconcile.
Supply a distinct runx project and process identity per hosted tenant. A
guest binding made before workflow isolation needs to be bound again to
record its stable guest identity.

The same executor can run alongside runx in a dedicated Linux worker or
attach to an exclusive remote Chromium using `"backend": "cdp"` and a
`RUNX_BROWSER_CDP` endpoint supplied to the executor by its host. Each
workflow on CDP receives a fresh anonymous context. For authenticated tasks,
a dedicated local profile supplies cookies and web storage for the permitted
sites. Each workflow gets its own copy. To refresh the seed after signing in
on the dedicated profile, restart the executor. The CDP endpoint is privileged.
Keep it private and arrange for the host to stop
both the executor and the browser when the task ends. Local and pre-provisioned CDP calls charge no per-operation fee through
this connector. The host or browser provider bills any infrastructure separately. Remote provisioning,
provider lease expiry, and attachment to runx Cloud's hosted connector
gateway are separate deployment work.

## Search

`browser/search.query` searches the web as a person does and answers in
web-search's format: `search.results`, at most `limit` of them, each with
its target `url`, `title` and `excerpt`. It opens an engine's results page in
a tab of its own, so the run's page stays as it was, and reads the results
the page carries. Redirect links are unwrapped to their targets. It reads
DuckDuckGo, then Bing, then Mojeek, one search each. It moves to the next
engine only when one turns the browser away, shows a check only a person can
clear, or shows no results to read. With none left it refuses and says why,
never answering empty. `include_domains` adds `site:` to the query and keeps
only results on those domains. A publication-date bound is refused, since a
results page can't honour one. It needs the `browser.search` scope alone, so
a binding can search without opening any site the agent names.

## Sites, sign-ins and submissions

`site.readOrigins` (or `site.origin`) makes the sites the tab may reach an
allow-list: navigation, redirects and popups go only there. Without it, the
tab goes where a person's would. A dedicated profile (`profile`) brings its
sign-ins: every site's cookies and storage, or only the listed sites' when
the binding lists them. A provider's "Sign in with ..." then completes on its
own session, by popup or redirect, the consent included, and the popup's tab
returns the agent to the one that opened it. `site.account` names an account
page and the visible selectors for its stable ID and login when a binding
pins which account it is; the executor checks that identity whenever it
observes a page.

What an action sends goes once, as the page made it: if its answer is lost,
the browser never sends it again, so inspect the site before acting again.
Read a submission's own record to confirm it. If the executor crashes,
restart the same binding and reopen the site; the new context starts fresh.

## What pages may send

runx's `browser.network` setting, which runx passes with each operation,
decides what a page sends. `open`, the default, lets it send what a person's
browser would. `held` does the same until the agent first clicks, fills or
selects on a page. From then, each request that page makes that changes
something, any method but GET, HEAD and OPTIONS, waits: the action's result
lists it in `pending`, with its destination and what it carries, and
`page.release` sends exactly that request, once, when the person approves it,
to `site.submitOrigins` alone when the binding lists them. Anything but a
release drops what waits. Set it with `runx config set browser.network held`.

## Bot checks and the person

When Cloudflare's managed challenge stands in for a page, `page.open` and
`follow` admit the challenge's own traffic (Cloudflare's challenge hosts and
the site's `/cdn-cgi/challenge-platform/` path), tick its checkbox once if it
shows one, and return the page Cloudflare releases. The site's clearance
cookie then serves later pages in the same run. Other checks that score the
page as it loads run as they would for a person.

A site may show its own person check in place of the page: a Turnstile,
reCAPTCHA, hCaptcha or ALTCHA checkbox on a page with nothing else to fill
in. `page.open` and `follow` tick it once, as a person does, and return the
page it releases; cleared beside a Continue button, the page is returned for
the agent to press it. Every observation names a check still on the page in
`check`, with whether it has cleared and whether it stands in for the page,
so a check is never read as the page's content.

A check the browser cannot pass, such as a puzzle, a slider, a press and hold,
a check asked again after the tick or a site that refuses the browser
outright, needs the person. The operation refuses and says so, and the agent
asks the person through the native human handoff. runx's app then
shows the run's tab live beside the question, wherever the person is: runx
reads it from this executor's socket and carries it as the view
`runx://runs/<workspace>/<run>/browser`, over the always-on runx's port or its
link's relay, and the person's clicks, drags, scrolls and keys come back as
`browser_input`, a person's command alone. The view streams only the run's
tab, headed, headless or attached, and nothing listens beyond the socket.
While the person works the tab, their own requests go out as theirs. The
agent's next operation takes the tab back and observes it afresh. Actions on
a page with an open Cloudflare challenge are refused until it clears.

On a server, run the executor headed (`headless: false`) under a virtual
display such as `xvfb-run`.

Browser routing scopes ordinary page requests. Run public browsing inside a
disposable machine with network controls that deny access to other tenants,
private services, and metadata endpoints. An origin list is an application
policy. Enforce the network boundary outside Chromium.

## Check

Run `pnpm --filter @runxhq/browser test` after installing Chromium. On
headless Linux, the same suite can exercise a window by setting
`BROWSER_TEST_HEADED=1` and running under Xvfb. The tests use an in-process
site and do not need a vendor account.
Set `RUNX_BROWSER_TEST_RUNTIME` to the locally built runx binary to exercise
durable root sealing and approval across separate native processes.
