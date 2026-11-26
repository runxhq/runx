import { createHash, randomUUID } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import { observedForms } from "./form.mjs";
import { httpUrl, Refused, requireThat } from "./protocol.mjs";
import { ENGINES, engineQuery, readResults, within } from "./search.mjs";

export { Refused } from "./protocol.mjs";

// How long a control may take to become ready for a click, fill or select.
const READY = 7_000;
const MAX_TREE = 24_000;
// Cloudflare's managed challenge stands in for a page with a 403. It runs
// from Cloudflare's challenge hosts and the site's challenge path, nowhere else.
const CHALLENGE_HOST = "challenges.cloudflare.com";
const CHALLENGE_PATH = "/cdn-cgi/challenge-platform/";
const SAMPLE = 250;
const READS = new Set(["GET", "HEAD", "OPTIONS"]);
// The keys an agent presses as a person does: to submit, move and dismiss.
const KEYS = /^(Enter|Tab|Escape|Backspace|Delete|Space|Home|End|PageUp|PageDown|Arrow(Up|Down|Left|Right))$/;

function digest(value) {
  return createHash("sha256").update(JSON.stringify(value)).digest("hex");
}

/**
 * Where a binding's browser goes. Without `readOrigins` (or `origin`), any
 * public site, as a person's browser does; with them, those alone.
 * `submitOrigins` bounds what a held page may release.
 */
export function sitePolicy(raw = {}) {
  const listed = raw.readOrigins ?? (raw.origin === undefined ? undefined : [raw.origin]);
  if (listed !== undefined) {
    requireThat(Array.isArray(listed) && listed.length > 0 && listed.length <= 32,
      "Choose up to 32 exact browser origins.");
    for (const value of listed) requireThat(httpUrl(value).origin === value, "Use exact browser origins.");
  }
  const submitOrigins = raw.submitOrigins;
  if (submitOrigins !== undefined) {
    requireThat(Array.isArray(submitOrigins) && submitOrigins.every(origin =>
      httpUrl(origin).origin === origin && (!listed || listed.includes(origin))),
      "Submit origins must be exact origins, among the bound ones when the binding lists them.");
  }
  const account = raw.account;
  if (account) {
    requireThat(listed?.length === 1 && typeof account.path === "string" &&
      account.path.startsWith("/") && typeof account.id === "string" &&
      typeof account.login === "string", "The account probe needs one bound origin, a route, an id and a login.");
  }
  return {
    origins: listed && new Set(listed),
    submitOrigins: submitOrigins && new Set(submitOrigins),
    account,
  };
}

export async function browserSession(context, policy, existingTab, guestId, engines = ENGINES) {
  const id = randomUUID();
  const first = existingTab ?? await context.newPage();
  // The tabs the agent works, the newest last: a popup or a link that opens a
  // tab becomes the page, and closing it returns to the tab that opened it.
  const tabs = [];
  const challenges = new Map();
  const touched = new Set();
  const sockets = new Set();
  const pending = new Map();
  // Writes the browser has issued and not yet settled, sent or held.
  const flying = new Set();
  // Each tab's last read, and the reads waiting on a held form post.
  const seen = new WeakMap();
  const holding = new Set();
  // runx's browser.network setting, sent with each operation.
  let network = "open";
  let attended = false;
  let acting;
  let written = 0;
  let revision = 0;
  let tail = Promise.resolve();
  const page = () => tabs.at(-1);

  function admitted(url) {
    if (policy.origins) return policy.origins.has(url.origin);
    return !privateHost(url.hostname);
  }

  function track(tab) {
    tabs.push(tab);
    tab.setDefaultTimeout(READY);
    tab.on("popup", popup => { revision++; track(popup); });
    tab.on("close", () => {
      const at = tabs.indexOf(tab);
      if (at > 0) { tabs.splice(at, 1); revision++; }
    });
    // A new document is a page the agent hasn't touched.
    tab.on("domcontentloaded", () => touched.delete(tab));
    void guardRedirects(tab);
  }

  // A redirect leaves for an admitted site alone. Playwright's routes see a
  // navigation's first hop; this sees each hop's response.
  async function guardRedirects(tab) {
    const client = await context.newCDPSession(tab).catch(() => null);
    if (!client) return;
    client.on("Fetch.requestPaused", event => {
      const status = event.responseStatusCode;
      const location = event.responseHeaders?.find(header => header.name.toLowerCase() === "location");
      let leaves = false;
      if (status >= 300 && status < 400 && location) {
        try { leaves = !admitted(httpUrl(location.value, event.request.url)); }
        catch { leaves = true; }
      }
      void client.send(leaves ? "Fetch.failRequest" : "Fetch.continueRequest",
        { requestId: event.requestId, ...(leaves && { errorReason: "BlockedByClient" }) }).catch(() => {});
    });
    await client.send("Fetch.enable", {
      patterns: [{ urlPattern: "*", resourceType: "Document", requestStage: "Response" }],
    }).catch(() => {});
  }
  track(first);
  const land = request => flying.delete(request);
  context.on("request", request => { if (!READS.has(request.method())) flying.add(request); });
  context.on("requestfinished", land);
  context.on("requestfailed", land);

  function touch(tab) {
    touched.add(tab);
    if (network !== "held") return;
    for (const socket of sockets) void socket.close().catch(() => {});
    sockets.clear();
  }

  await context.routeWebSocket("**/*", socket => {
    let url;
    try { url = httpUrl(socket.url().replace(/^ws/, "http")); }
    catch { return socket.close(); }
    if (network === "held" && !attended && touched.has(page()) && !checkTraffic(url)) return socket.close();
    socket.connectToServer();
    sockets.add(socket);
  });
  await context.route("**/*", async route => {
    const request = route.request();
    let url;
    try { url = httpUrl(request.url()); }
    catch { return route.abort("blockedbyclient"); }
    let frame;
    try { frame = request.frame(); }
    catch {
      // A popup's first navigation comes before its frame does.
      return request.isNavigationRequest() && admitted(url) ? route.continue() : route.abort("blockedbyclient");
    }
    const tab = frame.page();
    const top = frame === tab.mainFrame();
    // The tab goes where a person's would, within the binding's sites.
    if (top && request.isNavigationRequest() && !admitted(url)) return route.abort("blockedbyclient");
    const ours = tabs.includes(tab);
    // The person working the live view acts for themselves.
    if (attended && ours) return route.continue();
    const challenge = challenges.get(tab);
    if (challenge && challengeTraffic(challenge, url, request, top)) return route.continue();
    const writes = !READS.has(request.method());
    // Held: once the agent has clicked, filled or selected on a page, what
    // it would change waits for a released, approved write. Everything else
    // goes as it would for a person.
    if (network === "held" && ours && writes && touched.has(tab) && !checkTraffic(url)) {
      const write = { id: `w${++written}`, route, request, tab, url,
        body: request.postDataBuffer() ?? Buffer.alloc(0) };
      pending.set(write.id, write);
      if (request.isNavigationRequest()) for (const wake of holding) wake();
      return;
    }
    // What the agent's action sends goes once, as the page made it: a lost
    // answer never sends it again.
    if (ours && writes && acting) {
      const entry = sent(request, url);
      acting.sent.push(entry);
      const answer = await forward(route).catch(() => undefined);
      // The site's answer, as the record of what the action sent.
      entry.status = answer?.status ?? null;
      entry.confirmation_url = answer?.location && URL.canParse(answer.location, url)
        ? new URL(answer.location, url).href : null;
      return;
    }
    return route.continue();
  });

  async function forward(route) {
    let response;
    try { response = await route.fetch({ maxRetries: 0, maxRedirects: 0, timeout: 0 }); }
    catch {
      await route.abort("failed");
      return undefined;
    }
    try {
      const answer = { status: response.status(), location: response.headers().location };
      await route.fulfill({ response });
      return answer;
    } finally {
      await response.dispose();
    }
  }

  // An action's writes have each been sent or held: what it set off has
  // reached the site, or waits for a release.
  async function landed(tab) {
    // A turn of the page's own task queue, so what its click set off is under
    // way. Over CDP, since Playwright's evaluate waits out a navigation, a
    // held one never commits, and a page with a navigation pending paints no
    // frames.
    const client = await context.newCDPSession(tab).catch(() => null);
    await client?.send("Runtime.evaluate", {
      expression: "new Promise(resolve => setTimeout(() => setTimeout(resolve)))",
      awaitPromise: true,
    }).catch(() => {});
    // Detaching waits out a pending navigation too, so it goes on its own.
    void client?.detach().catch(() => {});
    const held = () => new Set([...pending.values()].map(write => write.request));
    while ([...flying].some(request => !held().has(request) && tabOf(request) && tabs.includes(tabOf(request)))) {
      await delay(SAMPLE / 5);
    }
  }

  // The page settled enough to read: its writes landed and its document
  // loaded, or the caller hung up. Reading again while it still moves.
  async function steady(tab, signal) {
    if (!tab.isClosed()) {
      await landed(tab);
      if (network !== "held" || !pending.size) await until(tab.waitForLoadState("domcontentloaded", { timeout: 0 }), signal).catch(() => {});
    }
    for (;;) {
      try { return await observe(); }
      catch (error) {
        if (!(error instanceof Refused) || !/changed during observation/.test(error.message) || signal?.aborted) throw error;
        await delay(SAMPLE);
      }
    }
  }

  // The page an opened address leads to, as read: never a check standing in
  // for it. A page may move on as it loads, as one that writes its own
  // address does, and a check may draw itself late.
  async function arrived(signal) {
    const view = await steady(page(), signal);
    if (!view.check?.stands_in || view.check.solved) return view;
    const check = await checkOn(page()).catch(() => null);
    if (check?.standsIn && !check.solved) await passCheck(page(), check, signal);
    return steady(page(), signal);
  }

  async function navigate(tab, address, signal) {
    const url = httpUrl(address);
    requireThat(admitted(url), "The address is outside the binding's sites.");
    endChallenge(tab);
    let response = await until(tab.goto(url.href, { waitUntil: "domcontentloaded", timeout: 0 }), signal);
    if (response?.headers()["cf-mitigated"] === "challenge") {
      response = await passChallenge(tab, url, signal);
      await until(tab.waitForLoadState("domcontentloaded", { timeout: 0 }), signal);
    }
    // A check still being drawn is judged once the page has loaded. A page
    // that moves on as it opens is read once it has.
    if (await tab.mainFrame().evaluate(marked, MARKS).catch(() => false))
      await until(tab.waitForLoadState("load", { timeout: 0 }), signal).catch(() => {});
    const check = await checkOn(tab).catch(() => null);
    if (check?.standsIn && !check.solved) {
      const released = await passCheck(tab, check, signal);
      // The check cleared where it stood: the page in front of the agent is the page.
      if (!released) {
        requireThat(admitted(httpUrl(tab.url())), "The site did not return the page.");
        return;
      }
      response = released;
    }
    const refused = [403, 429].includes(response?.status());
    requireThat(response?.ok() && admitted(httpUrl(tab.url())), refused
      ? "The site refused this browser. If its page asks for a person, hand off to the person to clear it in the live view, then read the page again."
      : "The site did not return the page.");
  }

  // Let the challenge's own traffic through and tick its checkbox once, when
  // it shows one. The page Cloudflare then releases is the page. A fresh
  // challenge after that click means Cloudflare refused this browser: the
  // challenge stays open for the person to clear in the live view.
  async function passChallenge(tab, url, signal) {
    const challenge = { origin: url.origin, pathname: url.pathname };
    challenges.set(tab, challenge);
    let clicked = false;
    let released;
    let refused = false;
    const watch = response => {
      if (response.frame() !== tab.mainFrame() || !response.request().isNavigationRequest() ||
          !challengeDocument(challenge, response.url())) return;
      if (response.headers()["cf-mitigated"] !== "challenge") released = response;
      else if (clicked) refused = true;
    };
    tab.on("response", watch);
    try {
      for (;;) {
        if (released) {
          challenges.delete(tab);
          return released;
        }
        requireThat(!refused,
          "Cloudflare did not accept this browser. Hand off to the person to clear the check in the live view, then open the page again.");
        requireThat(!signal?.aborted && !tab.isClosed(),
          "The operation ended before the site's check cleared. Inspect the page again.");
        if (!clicked) {
          const point = await challengeCheckbox(tab).catch(() => null);
          if (point) {
            await tab.mouse.click(point.x, point.y);
            clicked = true;
          }
        }
        await delay(SAMPLE);
      }
    } finally {
      tab.off("response", watch);
    }
  }

  async function challengeCheckbox(tab) {
    const frame = tab.frames().find(child => child.parentFrame() === tab.mainFrame() &&
      URL.canParse(child.url()) && challengeHost(new URL(child.url())));
    return frame ? turnstileBox(frame) : null;
  }

  function endChallenge(tab) {
    challenges.delete(tab);
  }

  // The person check a tab shows, if any: which one, how a person ticks it,
  // whether it has cleared, and whether it stands in for the page, as a page
  // where the check is all there is to do. Throws while the page moves on.
  async function checkOn(tab) {
    if (tab.isClosed()) return null;
    let found;
    let puzzle = false;
    for (const frame of tab.frames()) {
      if (frame === tab.mainFrame() || !URL.canParse(frame.url())) continue;
      const url = new URL(frame.url());
      const kind = CHECKS.find(check => check.frame?.(url));
      const puzzles = PUZZLES(url);
      if (!kind && !puzzles) continue;
      const owner = await frame.frameElement().catch(() => null);
      if (!await owner?.isVisible().catch(() => false)) continue;
      if (puzzles) puzzle = true;
      else found ??= { ...kind, frame };
    }
    const page = await tab.mainFrame().evaluate(inspectChecks, {
      elements: CHECKS.flatMap(check => check.element ? [check.element] : []),
      answers: CHECKS.flatMap(check => check.answer ? [check.answer] : []),
    });
    found ??= page.element && CHECKS.find(check => check.element === page.element);
    if (!found) return null;
    const state = found.element && page.state;
    return {
      by: found.by,
      tick: found.tick && (() => found.tick(found.frame ?? tab)),
      decides: Boolean(found.decides),
      solved: found.answer ? page.answers.includes(found.answer) : state === "verified",
      busy: state === "verifying",
      puzzle: puzzle || ["error", "expired"].includes(state),
      standsIn: page.fields === 0,
      presses: page.presses,
    };
  }

  // A check standing in for the page asked for: tick it once, as a person
  // does, and return the page it releases, or nothing when it clears where it
  // stands. A puzzle, or a check again after it, is for the person.
  async function passCheck(tab, check, signal) {
    let navigating = false;
    let released;
    const main = request => { try { return request.isNavigationRequest() && request.frame() === tab.mainFrame(); } catch { return false; } };
    const asked = request => { if (main(request)) navigating = true; };
    const failed = request => { if (main(request)) navigating = false; };
    const answered = response => { if (main(response.request())) released = response; };
    tab.on("request", asked);
    tab.on("requestfailed", failed);
    tab.on("response", answered);
    let ticked = false;
    try {
      // A page that moves on mid-read is read again, once it has.
      for (let now = check; ; now = await checkOn(tab).catch(() => now)) {
        if (released) {
          await until(tab.waitForLoadState("domcontentloaded", { timeout: 0 }), signal);
          const again = await checkOn(tab).catch(() => null);
          requireThat(!again?.standsIn, handOff(again?.by ?? check.by));
          return released;
        }
        requireThat(!signal?.aborted && !tab.isClosed(),
          "The operation ended before the site's check cleared. Inspect the page again.");
        // The check left the page without it moving: what's there is the page.
        if (!now) return undefined;
        requireThat(!now.puzzle && (now.tick || now.decides), handOff(now.by));
        if (now.solved) {
          // What its clearing set off is under way before the page is judged.
          // Cleared beside a button, the page waits for its press; with none
          // to press, it goes on by itself.
          await landed(tab);
          if (!navigating && now.presses) return undefined;
        } else if (!ticked && !now.busy && now.tick) {
          const point = await now.tick().catch(() => null);
          if (point) {
            await tab.mouse.click(point.x, point.y);
            ticked = true;
          }
        }
        await delay(SAMPLE);
      }
    } finally {
      tab.off("request", asked);
      tab.off("requestfailed", failed);
      tab.off("response", answered);
    }
  }

  async function principal() {
    if (!policy.account) return { id: guestId ?? id, login: "guest" };
    const probe = await context.newPage();
    try {
      const url = httpUrl(policy.account.path, [...policy.origins][0]);
      await navigate(probe, url.href);
      const fields = await Promise.all(
        [policy.account.id, policy.account.login].map(selector => probe.locator(selector).innerText()),
      );
      requireThat(fields.every(value => value.length > 0 && value.length <= 256),
        "The site did not identify the signed-in account.");
      return { id: fields[0], login: fields[1] };
    } finally { await probe.close(); }
  }

  const heldPost = tab => [...pending.values()].some(write => write.tab === tab && write.request.isNavigationRequest());

  // A held form post leaves its page as it was, and Playwright reads a page
  // only once its navigation commits, which a held one never does: the last
  // read stands for it, from the moment the post is held.
  async function observe(offset = 0) {
    const tab = page();
    if (!heldPost(tab)) {
      let stop;
      const posted = new Promise(resolve => { stop = resolve; holding.add(stop); });
      try {
        const read = await Promise.race([readTab(tab, offset), posted.then(() => undefined)]);
        if (read) return read;
      } finally {
        holding.delete(stop);
      }
    }
    const last = seen.get(tab) ?? { url: tab.url(), tree: "", forms: [] };
    return {
      ...last,
      snapshot: digest([id, revision, tabs.length, last.url, last.tree, last.forms, [...pending.keys()]]),
      page: last.tree.slice(offset, offset + MAX_TREE),
      next_offset: offset + MAX_TREE < last.tree.length ? offset + MAX_TREE : null,
      tabs: tabs.length,
      pending: [...pending.values()].map(held),
    };
  }

  async function readTab(tab, offset) {
    const url = tab.url();
    requireThat(url !== "about:blank" && admitted(httpUrl(url)), "Open a page first.");
    // A tab that closes or moves on mid-read, as a popup does once it's
    // done, is a page that changed.
    const moving = error => {
      if (tab.isClosed() || page() !== tab || tab.url() !== url) throw new Refused("The page changed during observation.");
      throw error;
    };
    const tree = await tab.ariaSnapshot({ mode: "ai", timeout: 0 }).catch(moving);
    requireThat(Number.isSafeInteger(offset) && offset >= 0 && offset <= tree.length, "Invalid offset.");
    const end = offset + MAX_TREE;
    const forms = await observedForms(tab, tree.slice(0, MAX_TREE)).catch(moving);
    // A person check on the page, so no caller takes it for the page's content.
    const found = await checkOn(tab).catch(moving);
    const check = found ? { by: found.by, solved: found.solved, stands_in: found.standsIn } : undefined;
    const owner = await principal();
    requireThat(tab.url() === url && page() === tab, "The page changed during observation.");
    seen.set(tab, { url, tree, forms, check });
    return {
      url,
      snapshot: digest([id, revision, tabs.length, url, tree, forms, owner]),
      page: tree.slice(offset, end),
      forms,
      check,
      next_offset: end < tree.length ? end : null,
      tabs: tabs.length,
      pending: [...pending.values()].map(held),
      tree,
    };
  }

  async function current(input) {
    requireThat(!challenges.has(page()),
      "The site's check is still open. Once the person clears it, open the page again.");
    const view = await observe();
    requireThat(view.url === input.url && view.snapshot === input.snapshot,
      "The page or account changed. Observe it again.");
    return view;
  }

  async function control(input, view) {
    requireThat(/^(?:f\d{1,8})?e\d{1,8}$/.test(input.ref) && view.tree.includes(`[ref=${input.ref}]`),
      "Use a control reference from the current observation.");
    const target = page().getByRef(input.ref);
    requireThat(await target.count() === 1 && await target.isVisible(),
      "The referenced control is missing or ambiguous.");
    return target;
  }

  async function act(input, view, signal) {
    const target = await control(input, view);
    let perform;
    if (input.action === "follow") {
      const href = await target.evaluate(el => el instanceof HTMLAnchorElement ? el.href : null);
      requireThat(href, "Follow needs a rendered link.");
      perform = () => navigate(page(), href, signal);
    } else if (input.action === "click") {
      // A person's click: the mouse at the control's middle, whatever it then
      // sets off. What follows settles before the page is read again.
      perform = async () => {
        await target.scrollIntoViewIfNeeded();
        const box = await target.boundingBox();
        requireThat(box, "The referenced control has no place on the page.");
        await page().mouse.click(box.x + box.width / 2, box.y + box.height / 2);
      };
    } else if (input.action === "fill") {
      requireThat(typeof input.value === "string", "Fill needs a value.");
      const secret = await target.evaluate(el => ["password", "file"].includes(el.type));
      requireThat(!secret, "This field needs the person's input.");
      perform = () => target.fill(input.value);
    } else if (input.action === "select") {
      requireThat(typeof input.value === "string", "Select needs an option label.");
      perform = () => target.selectOption({ label: input.value });
    } else if (input.action === "press") {
      requireThat(typeof input.value === "string" && KEYS.test(input.value), "Press needs a key such as Enter, Tab or ArrowDown.");
      // The keyboard, as a person's: no wait on a navigation the key starts.
      perform = async () => {
        await target.focus();
        await page().keyboard.press(input.value === "Space" ? " " : input.value);
      };
    } else if (input.action === "hover") {
      perform = () => target.hover();
    } else if (input.action === "scroll") {
      perform = () => target.scrollIntoViewIfNeeded();
    } else {
      requireThat(false, "Unknown browser action.");
    }
    // A rejected request leaves the observation intact. Once dispatched, an
    // action may change the page even if its acknowledgement is lost.
    revision++;
    const tab = page();
    if (["click", "fill", "select", "press"].includes(input.action)) touch(tab);
    acting = { sent: [] };
    try {
      await perform();
      // What the action set off settles before the page is read again.
      const view = publicView(input.action === "follow" ? await arrived(signal) : await steady(tab, signal));
      return { ...view, sent: acting.sent };
    } finally {
      acting = undefined;
    }
  }

  // One search, as a person makes it: an engine's results page in a tab of
  // its own, read into results. An engine that turns the browser away, or
  // whose page holds no results to read, hands on to the next; with none
  // left, the search refuses and says why, never answering empty.
  async function search(input, signal) {
    const { query, limit, include_domains: domains } = input;
    requireThat(typeof query === "string" && /\S/.test(query) && query.length <= 2000, "Search needs a query.");
    requireThat(Number.isSafeInteger(limit) && limit >= 1 && limit <= 20, "Search needs a limit from 1 to 20.");
    requireThat(domains === undefined || (Array.isArray(domains) && domains.length <= 20 &&
      domains.every(domain => typeof domain === "string" && /^[a-zA-Z0-9.-]{1,253}$/.test(domain))),
      "Search takes up to 20 exact domains.");
    requireThat(input.start_published_at === undefined && input.end_published_at === undefined,
      "A results page can't bound its results by publication date, so this search can't honour one.");
    const turned = [];
    for (const engine of engines) {
      const tab = await context.newPage();
      void guardRedirects(tab);
      try {
        await navigate(tab, engine.url(engineQuery(query, domains)), signal);
        const results = [];
        for (const { href, title, excerpt } of await tab.mainFrame().evaluate(readResults, engine.parts)) {
          let target;
          try { target = engine.target(href); } catch { continue; }
          if (!target || !URL.canParse(target) || !title) continue;
          const url = new URL(target);
          if (!["http:", "https:"].includes(url.protocol) || !within(url, domains) ||
              results.some(result => result.url === url.href)) continue;
          results.push({ url: url.href, title: title.slice(0, 500), ...(excerpt && { excerpt: excerpt.slice(0, 1000) }) });
          if (results.length === limit) break;
        }
        if (results.length) return { search: { results } };
        turned.push(`${engine.name} showed no results to read`);
      } catch (error) {
        if (signal?.aborted) throw error;
        turned.push(`${engine.name}: ${error instanceof Refused ? error.message : "its page didn't load"}`);
      } finally {
        await tab.close().catch(() => {});
      }
    }
    throw new Refused(`No search engine answered this search. ${turned.join(" ")}`);
  }

  // A held page's write, released once, on the person's approval.
  async function release(input, signal) {
    requireThat(input.phase === "perform", "runx must perform this write.");
    const identity = await principal();
    requireThat(input.session_account === identity.login, "The browser account changed.");
    const write = pending.get(input.write);
    requireThat(write && write.url.href === input.destination,
      "That write isn't held now. Read the page, and act again if it's still needed.");
    requireThat(!policy.submitOrigins || policy.submitOrigins.has(write.url.origin),
      "This site is outside the binding's submit sites.");
    pending.delete(write.id);
    revision++;
    // Chromium may resend a write after a lost connection. Forward only the
    // held request, once, and hand the page its answer as it came.
    const answer = await forward(write.route).catch(() => undefined);
    if (!answer) throw new Error("Submission outcome unknown. Inspect the site before any retry.");
    return {
      destination: write.url.href,
      method: write.request.method(),
      request_digest: createHash("sha256").update(write.body).digest("hex"),
      response_status: answer.status,
      confirmation_url: answer.location && URL.canParse(answer.location, write.url)
        ? new URL(answer.location, write.url).href : null,
      ...publicView(await steady(write.tab, signal)),
    };
  }

  function drop() {
    for (const write of pending.values()) void write.route.abort("blockedbyclient").catch(() => {});
    pending.clear();
  }

  async function handle(input, signal) {
    if (input.session_network !== undefined) {
      requireThat(["open", "held"].includes(input.session_network),
        "runx sent an unknown browser.network setting.");
      network = input.session_network;
    }
    if (attended) {
      // The agent takes the tab back. The page may hold what the person
      // typed, and its observations are stale.
      attended = false;
      endChallenge(page());
      touch(page());
      revision++;
    }
    // Writes a held page set off wait for the agent's next say: a release,
    // or anything else, which drops them.
    // A search goes in a tab of its own and leaves the page as it was.
    if (!["page.read", "page.release", "session", "search.query"].includes(input.connector_operation)) drop();
    switch (input.connector_operation) {
      case "session": return principal();
      case "page.open":
        revision++;
        await navigate(page(), input.url, signal);
        return publicView(await arrived(signal));
      case "page.read": {
        const view = await observe(input.offset);
        if (input.offset) requireThat(input.snapshot === view.snapshot, "Read the changed page from the beginning.");
        return publicView(view);
      }
      case "page.act": return act(input, await current(input), signal);
      case "page.release": return release(input, signal);
      case "search.query": return search(input, signal);
      default: requireThat(false, "Unknown browser operation.");
    }
  }

  return {
    get page() { return page(); },
    call(input, signal) {
      const next = tail.then(() => handle(input, signal));
      tail = next.catch(() => {});
      return next;
    },
    // The person is working the tab through the live view until the
    // agent's next operation.
    attend() { attended = true; },
  };
}

function publicView({ tree, ...value }) {
  return value;
}

/** What a held write sends, as the agent and the person see it. */
function held(write) {
  const type = (write.request.headers()["content-type"] ?? "").split(";")[0].trim();
  const text = write.body.toString("utf8");
  const carries = type === "application/x-www-form-urlencoded"
    ? Object.fromEntries([...new URLSearchParams(text)].slice(0, 48)
      .map(([name, value]) => [name.slice(0, 128), value.length <= 256 ? value : `${value.slice(0, 256)}…`]))
    : type.includes("json") ? text.slice(0, 2048)
    : `${type || "no content type"}, ${write.body.length} bytes`;
  return { write: write.id, method: write.request.method(), destination: write.url.href,
    bytes: write.body.length, carries };
}

function tabOf(request) {
  try { return request.frame().page(); }
  catch { return undefined; }
}

/** What an action sent that changes something, for its record. */
function sent(request, url) {
  return { method: request.method(), destination: url.href, bytes: request.postDataBuffer()?.length ?? 0 };
}

/** `work`, or a refusal when the caller hangs up first. */
function until(work, signal) {
  if (!signal) return work;
  return new Promise((resolve, reject) => {
    const stop = () => reject(new Refused("The operation ended before the page answered. Read the page again."));
    if (signal.aborted) return stop();
    signal.addEventListener("abort", stop, { once: true });
    work.then(resolve, reject).finally(() => signal.removeEventListener("abort", stop));
  });
}

// Loopback, private, link-local and metadata addresses, which a binding
// reaches only by listing them.
function privateHost(hostname) {
  const host = hostname.replace(/^\[|\]$/g, "").toLowerCase();
  if (host === "localhost" || host.endsWith(".localhost") || host.endsWith(".internal") ||
      host.endsWith(".local") || host === "metadata.google.internal") return true;
  const v4 = host.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/);
  if (v4) {
    const [a, b] = [Number(v4[1]), Number(v4[2])];
    return a === 0 || a === 10 || a === 127 || (a === 169 && b === 254) ||
      (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 100 && b >= 64 && b <= 127);
  }
  return host.includes(":") &&
    (host === "::1" || host === "::" || /^f[cd]/.test(host) || /^fe[89ab]/.test(host) || host.startsWith("::ffff:"));
}

// The page the challenge stands in for, with whatever query the challenge adds.
function challengeDocument(challenge, address, base) {
  try {
    const url = new URL(address, base);
    return url.origin === challenge.origin && url.pathname === challenge.pathname;
  } catch {
    return false;
  }
}

function challengeHost(url) {
  return url.protocol === "https:" &&
    (url.hostname === CHALLENGE_HOST || url.hostname.endsWith("." + CHALLENGE_HOST));
}

// Cloudflare's, Google's and hCaptcha's checks verify from these alone.
function checkTraffic(url) {
  if (url.protocol !== "https:") return false;
  const under = domain => url.hostname === domain || url.hostname.endsWith("." + domain);
  return challengeHost(url) || under("hcaptcha.com") ||
    (["www.google.com", "www.recaptcha.net", "www.gstatic.com"].includes(url.hostname) &&
      url.pathname.startsWith("/recaptcha/"));
}

function challengeTraffic(challenge, url, request, top) {
  const navigates = top && request.isNavigationRequest();
  if (challengeHost(url)) return !navigates;
  if (url.origin !== challenge.origin) return false;
  if (url.pathname.startsWith(CHALLENGE_PATH)) return !navigates;
  return navigates && url.pathname === challenge.pathname;
}

async function turnstileBox(frame) {
  const owner = await (await frame.frameElement()).boundingBox();
  if (!owner) return null;
  // The checkbox sits in a closed shadow root that page scripts cannot
  // reach. The frame's DOM, read over CDP, includes it.
  const client = await frame.page().context().newCDPSession(frame);
  try {
    const { root } = await client.send("DOM.getDocument", { depth: -1, pierce: true });
    const box = findCheckbox(root);
    if (!box) return null;
    const { model } = await client.send("DOM.getBoxModel", { nodeId: box.nodeId });
    if (!model.width || !model.height) return null;
    const [left, top, , , right, bottom] = model.content;
    return { x: owner.x + (left + right) / 2, y: owner.y + (top + bottom) / 2 };
  } finally {
    await client.detach().catch(() => {});
  }
}

const under = (url, domain) => url.hostname === domain || url.hostname.endsWith("." + domain);
const recaptcha = (url, part) => url.protocol === "https:" &&
  ["www.google.com", "www.recaptcha.net"].includes(url.hostname) &&
  new RegExp(`^/recaptcha/(api2|enterprise)/${part}$`).test(url.pathname);
const hcaptcha = (url, part) => url.protocol === "https:" && under(url, "hcaptcha.com") &&
  new URLSearchParams(url.hash.slice(1)).get("frame") === part;

// The person checks sites show: each one by the frame it draws in or the
// element it is, the mark a page carries for it before it's drawn, the field
// its answer fills, the checkbox a person ticks, and whether it may clear by
// itself. One with neither a checkbox nor that is a puzzle for the person, as
// is the puzzle a ticked one may answer with.
const CHECKS = [
  { by: "Cloudflare Turnstile", frame: challengeHost, mark: ".cf-turnstile", answer: "cf-turnstile-response",
    decides: true, tick: turnstileBox },
  { by: "reCAPTCHA", frame: url => recaptcha(url, "anchor") && url.searchParams.get("size") !== "invisible",
    mark: ".g-recaptcha", answer: "g-recaptcha-response", tick: frame => middle(frame.locator("#recaptcha-anchor")) },
  { by: "hCaptcha", frame: url => hcaptcha(url, "checkbox") && !url.hash.includes("size=invisible"),
    mark: ".h-captcha", answer: "h-captcha-response", tick: frame => middle(frame.locator("#checkbox")) },
  { by: "ALTCHA", element: "altcha-widget", tick: tab => middle(tab.locator("altcha-widget input[type=checkbox]")) },
  { by: "DataDome", frame: url => under(url, "captcha-delivery.com") && url.pathname.startsWith("/interstitial/"), decides: true },
  { by: "DataDome", frame: url => under(url, "captcha-delivery.com") },
  { by: "HUMAN", element: "#px-captcha" },
  { by: "Arkose Labs", frame: url => under(url, "arkoselabs.com") || under(url, "funcaptcha.com") },
];
const PUZZLES = url => recaptcha(url, "bframe") || hcaptcha(url, "challenge");
const MARKS = CHECKS.flatMap(check => [check.mark ?? check.element]).filter(Boolean).join(", ");

// Runs in the page: whether it carries a check's mark, drawn or not.
function marked(selector) {
  return document.querySelector(selector) !== null;
}

function handOff(by) {
  return `The site's check (${by}) needs a person. Hand off to the person to clear it in the live view, then read the page again.`;
}

async function middle(control) {
  if (await control.count() !== 1) return null;
  const box = await control.boundingBox();
  return box && { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

// Runs in the page: the first visible check element, an ALTCHA widget's
// state, which check answers are filled, how many fields a person could type
// in besides the check, and how many buttons the check's own form has to press.
function inspectChecks({ elements, answers }) {
  const visible = el => el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true });
  const element = elements.find(selector => [...document.querySelectorAll(selector)].some(visible));
  const widget = document.querySelector("altcha-widget");
  const state = (widget?.shadowRoot ?? widget)?.querySelector("[data-state]")?.getAttribute("data-state");
  const fields = [...document.querySelectorAll("input, textarea, select")].filter(el =>
    !["hidden", "checkbox", "radio", "submit", "button", "image", "reset"].includes(el.type) && visible(el)).length;
  const parts = [...elements.flatMap(selector => [...document.querySelectorAll(selector)]),
    ...answers.flatMap(name => [...document.getElementsByName(name)])];
  const forms = new Set(parts.map(el => el.closest("form")).filter(Boolean));
  const presses = [...forms].flatMap(form => [...form.elements]).filter(el =>
    (el.tagName === "BUTTON" || ["submit", "button", "image"].includes(el.type)) && visible(el)).length;
  return {
    element,
    state,
    answers: answers.filter(name => [...document.getElementsByName(name)].some(el => el.value)),
    fields,
    presses,
  };
}

function findCheckbox(node) {
  if (!node) return null;
  const attributes = node.nodeName === "INPUT" ? node.attributes ?? [] : [];
  for (let index = 0; index < attributes.length; index += 2) {
    if (attributes[index] === "type" && attributes[index + 1] === "checkbox") return node;
  }
  for (const child of [...node.children ?? [], ...node.shadowRoots ?? [], node.contentDocument]) {
    const found = findCheckbox(child);
    if (found) return found;
  }
  return null;
}
