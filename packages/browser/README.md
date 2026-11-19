# Runx browser connector

Add `browser/page.open`, `browser/page.read` and `browser/page.act` to the existing
agent step that needs rendered pages. The agent sees the URL, accessibility
tree and fresh control references on each call. It can follow visible links,
click, fill, choose a visible option or scroll within the bound origins. If the page exceeds the response limit,
pass its `next_offset` and `snapshot` to `page.read`. If a page or account
changes, inspect it again before issuing an action.

Use `web/page.read` or the existing search connector for discovery and static
pages. A browser binding supplies a browser and its permitted sites for one
task. Fieldwork selects the binding on its Runx runner and calls the same
operations as a local agent. Fieldwork retains the task and native Runx
retains authority, decisions and receipts.

## Start a browser

The executor uses Unix-domain sockets and currently runs on POSIX hosts.
It uses the same browser operations with Chromium installed on macOS or Linux
or attached over CDP. Native Windows support is pending.

From the Runx workspace with Node.js 22 or newer and dependencies installed,
install the pinned Chromium using
`pnpm --filter @runxhq/browser exec playwright-core install chromium`.
Save this config on the runner as `browser.json`.

```json
{
  "binding": "sources",
  "backend": "local",
  "headless": true,
  "site": { "readOrigins": ["https://www.iana.org"] }
}
```

Start `node packages/browser/src/host.mjs browser.json` in a supervised
process and load `connectors/browser` into the Runx project. From that
project, bind it with
`runx connector use browser --session --parameter profile=sources --scope browser.read`.

```yaml
calls:
  operations: [browser/page.open, browser/page.read, browser/page.act]
```

Set `headless` to `false` for a window. Set `channel` to `chrome` to use
installed Chrome, or set `executablePath` to an installed Chromium-compatible
browser such as Brave. Each task uses its own executor and binding name.
Supply a distinct Runx project and process identity per hosted tenant.

The same executor can run alongside Runx in a dedicated Linux worker or
attach to an exclusive remote Chromium using `"backend": "cdp"` and a
`RUNX_BROWSER_CDP` endpoint supplied to the executor by the runner. Each CDP attachment creates a fresh anonymous context. Use a dedicated local
profile for authenticated tasks today. The CDP endpoint is privileged. Keep it private and arrange for the runner to stop
both the executor and the browser when the task ends. Local and pre-provisioned CDP calls charge no per-operation fee through
this connector. The runner or browser provider bills any infrastructure separately. Remote provisioning,
provider lease expiry, and attachment to Runx Cloud's hosted connector
gateway are separate deployment work.

## Scope a site action

For read-only work, set `site.origin` or enumerate `site.readOrigins`.
For form submissions, enumerate `site.submitOrigins` as a subset of those
origins. Where the task uses a signed-in local profile, `site.account` names
the account page and the visible selectors for its stable ID and login. The
executor checks that identity when it observes or prepares a page.

Grant `browser.release` to the task that needs to submit. Add
`browser/page.prepare` and `browser/page.release` to that agent step. The
agent prepares a currently rendered submit control and receives its
destination, terms and proposal digest. Runx binds approval to those fields.
Immediately before submission the executor rechecks the current session,
snapshot, control and exact terms. It forwards one matching POST without
retries. The first implementation accepts URL-encoded native HTML forms.

The response reports the HTTP acknowledgement and an eligible confirmation
URL when the site supplies one. Read the actual record separately and
corroborate it with other evidence when available. If acknowledgement is
lost, inspect current site state before deciding whether any further action
is appropriate. Keep the executor running until confirmation or
reconciliation is complete.

For a human challenge, suspend the agent step with the native human handoff.
The operator works in the open browser. Observe the current page afresh when
the run resumes. Headed operation requires access to the renderer's display.

Browser routing scopes ordinary page requests. Run public browsing inside a
disposable runner with network controls that deny access to other tenants,
private services, and metadata endpoints. An origin list is an application
policy. Enforce the network boundary outside Chromium.

## Check

Run `pnpm --filter @runxhq/browser test` after installing Chromium. On
headless Linux, the same suite can exercise a window by setting
`BROWSER_TEST_HEADED=1` and running under Xvfb. The tests use an in-process
site and do not need a vendor account.
