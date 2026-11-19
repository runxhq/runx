import { createHash, randomUUID } from "node:crypto";
import { isDeepStrictEqual } from "node:util";
import { formSubmission, matchesSubmission } from "./form.mjs";
import { httpUrl, requireThat } from "./protocol.mjs";

export { Refused } from "./protocol.mjs";

const TIMEOUT = 7_000;
const MAX_TREE = 24_000;
const ASSET_TYPES = new Set(["stylesheet", "script", "image", "font"]);

function digest(value) {
  return createHash("sha256").update(JSON.stringify(value)).digest("hex");
}

export function sitePolicy(raw) {
  const origins = raw.readOrigins ?? [raw.origin];
  requireThat(Array.isArray(origins) && origins.length > 0 && origins.length <= 32,
    "Choose up to 32 exact browser origins.");
  for (const value of origins) {
    requireThat(httpUrl(value).origin === value, "Use exact browser origins.");
  }
  const submitOrigins = raw.submitOrigins ?? [];
  requireThat(Array.isArray(submitOrigins) && submitOrigins.every(origin => origins.includes(origin)),
    "Submit origins must be among the bound origins.");
  const account = raw.account;
  if (account) {
    requireThat(origins.length === 1 && typeof account.path === "string" &&
      account.path.startsWith("/") && typeof account.id === "string" &&
      typeof account.login === "string", "The account probe needs a route, id and login.");
  }
  return { origins: new Set(origins), submitOrigins: new Set(submitOrigins), account };
}

export async function browserSession(context, policy, existingTab) {
  const id = randomUUID();
  const page = existingTab ?? await context.newPage();
  page.setDefaultTimeout(TIMEOUT);
  const navigation = new Map();
  let staged;
  let sending;
  let revision = 0;
  let tail = Promise.resolve();

  function admitted(url) {
    return policy.origins.has(url.origin);
  }

  // The runner also enforces its network boundary outside the browser.
  // Page scripts may fetch the managed document's ordinary assets while it
  // loads. Exploration clicks never get a general network allowance.
  await context.routeWebSocket("**/*", socket => socket.close());
  await context.route("**/*", async route => {
    const request = route.request();
    let frame;
    try { frame = request.frame(); }
    catch { return route.abort("blockedbyclient"); }
    const tab = frame.page();
    if (frame !== tab.mainFrame()) return route.abort("blockedbyclient");
    const url = httpUrl(request.url());
    const scope = navigation.get(tab);
    if (scope && ["GET", "HEAD"].includes(request.method()) && admitted(url) &&
        (request.isNavigationRequest() ? url.href === scope.href :
          url.origin === scope.origin && ASSET_TYPES.has(request.resourceType()))) {
      return route.continue();
    }
    const action = sending;
    if (tab !== page || !action || action.dispatched || request.method() !== "POST" ||
        url.href !== action.destination || !request.isNavigationRequest() || !matchesSubmission(request, action)) {
      return route.abort("blockedbyclient");
    }
    action.dispatched = true;
    try {
      // Chromium may resend a POST after a lost connection. Forward only
      // the held request, with transport retries and redirects disabled.
      const response = await route.fetch({ maxRetries: 0, maxRedirects: 0, timeout: TIMEOUT });
      try {
        const status = response.status();
        const location = response.headers().location;
        if (status >= 200 && status < 400) {
          action.acknowledgement = { status, confirmation_url: null };
          if (location) {
            const receipt = httpUrl(location, url);
            if (receipt.origin === url.origin && admitted(receipt))
              action.acknowledgement.confirmation_url = receipt.href;
          }
        }
        if (status >= 300 && status < 400) return await route.abort("blockedbyclient");
        return await route.fulfill({ response });
      } finally {
        await response.dispose();
      }
    } catch {
      await route.abort("failed").catch(() => {});
    }
  });

  async function navigate(tab, address) {
    const url = httpUrl(address);
    requireThat(admitted(url), "The address is outside the bound browser scope.");
    // Playwright request routes apply only to the first hop. The browser's
    // response-stage hook prevents additional navigation from a redirect.
    const client = await context.newCDPSession(tab);
    client.on("Fetch.requestPaused", event => {
      const status = event.responseStatusCode;
      const isRedirect = status >= 300 && status < 400 &&
        event.responseHeaders?.some(header => header.name.toLowerCase() === "location");
      const method = isRedirect ? "Fetch.failRequest" : "Fetch.continueRequest";
      const arguments_ = { requestId: event.requestId };
      if (isRedirect) arguments_.errorReason = "BlockedByClient";
      void client.send(method, arguments_).catch(() => {});
    });
    await client.send("Network.setBypassServiceWorker", { bypass: true });
    await client.send("Fetch.enable", { patterns: [{ urlPattern: "*", requestStage: "Response" }] });
    navigation.set(tab, url);
    try {
      const response = await tab.goto(url.href, { waitUntil: "domcontentloaded", timeout: TIMEOUT });
      requireThat(response?.ok() && new URL(tab.url()).origin === url.origin,
        "The site did not return the admitted page.");
    } finally {
      navigation.delete(tab);
      await client.detach().catch(() => {});
    }
  }

  async function principal() {
    if (!policy.account) return { id, login: "guest" };
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

  async function observe(offset = 0) {
    const url = page.url();
    requireThat(url !== "about:blank" && admitted(httpUrl(url)), "Open a scoped page first.");
    const tree = await page.ariaSnapshot({ mode: "ai", timeout: TIMEOUT });
    requireThat(page.url() === url, "The page changed during observation.");
    requireThat(Number.isSafeInteger(offset) && offset >= 0 && offset <= tree.length, "Invalid offset.");
    const end = offset + MAX_TREE;
    return {
      url,
      snapshot: digest([id, revision, url, tree, await principal()]),
      page: tree.slice(offset, end),
      next_offset: end < tree.length ? end : null,
      tree,
    };
  }

  async function current(input) {
    const view = await observe();
    requireThat(view.url === input.url && view.snapshot === input.snapshot,
      "The page or account changed. Observe it again.");
    return view;
  }

  async function control(input, view) {
    requireThat(/^(?:f\d{1,8})?e\d{1,8}$/.test(input.ref) && view.tree.includes(`[ref=${input.ref}]`),
      "Use a control reference from the current observation.");
    const target = page.getByRef(input.ref);
    requireThat(await target.count() === 1 && await target.isVisible(),
      "The referenced control is missing or ambiguous.");
    return target;
  }

  async function act(input, view) {
    const target = await control(input, view);
    revision++;
    staged = undefined;
    if (input.action === "follow") {
      const href = await target.evaluate(el => el instanceof HTMLAnchorElement ? el.href : null);
      requireThat(href, "Follow needs a rendered link.");
      await navigate(page, href);
    } else if (input.action === "click") {
      const submits = await target.evaluate(el => Boolean(el.form && el.type === "submit"));
      requireThat(!submits, "Prepare the form before asking Runx to submit it.");
      await target.click();
    } else if (input.action === "fill") {
      requireThat(typeof input.value === "string", "Fill needs a value.");
      const secret = await target.evaluate(el => ["password", "file"].includes(el.type));
      requireThat(!secret, "This field needs the person's input.");
      await target.fill(input.value);
    } else if (input.action === "select") {
      requireThat(typeof input.value === "string", "Select needs an option label.");
      await target.selectOption({ label: input.value });
    } else if (input.action === "scroll") {
      await target.scrollIntoViewIfNeeded();
    } else {
      requireThat(false, "Unknown browser action.");
    }
    return publicView(await observe());
  }

  async function prepare(input, view) {
    const form = await formSubmission(await control(input, view));
    const destination = httpUrl(form.destination, view.url);
    requireThat(destination.origin === httpUrl(view.url).origin &&
      policy.submitOrigins.has(destination.origin),
      "This site is outside the binding's submission authority.");
    staged = { ...form, url: view.url, snapshot: view.snapshot, ref: input.ref,
      destination: destination.href };
    return { ...staged, proposal: digest(staged) };
  }

  async function release(input) {
    requireThat(input.phase === "perform", "Runx must perform this native write.");
    const view = await current(input);
    const identity = await principal();
    requireThat(input.session_account === identity.login, "The browser account changed.");
    requireThat(staged && input.proposal === digest(staged) && input.ref === staged.ref &&
      input.snapshot === staged.snapshot && input.destination === staged.destination &&
      isDeepStrictEqual(input.terms, staged.terms),
      "The approved proposal is unavailable or different.");
    const button = await control(input, view);
    const live = await formSubmission(button);
    requireThat(httpUrl(live.destination, view.url).href === staged.destination &&
      isDeepStrictEqual(live.terms, staged.terms), "The form terms or destination changed.");
    const action = { ...staged, dispatched: false, acknowledgement: undefined };
    staged = undefined;
    sending = action;
    revision++;
    try {
      // A navigation triggered by the submit may itself report an aborted
      // move if the transport response was lost or required a fresh read.
      await button.click().catch(() => {});
      if (!action.acknowledgement) {
        throw new Error("Submission outcome unknown. Inspect the site before any retry.");
      }
      return { destination: action.destination, terms: action.terms,
        response_status: action.acknowledgement.status,
        confirmation_url: action.acknowledgement.confirmation_url };
    } finally { sending = undefined; }
  }

  async function handle(input) {
    switch (input.connector_operation) {
      case "session": return principal();
      case "page.open":
        staged = undefined;
        revision++;
        await navigate(page, input.url);
        return publicView(await observe());
      case "page.read": {
        const view = await observe(input.offset);
        if (input.offset) requireThat(input.snapshot === view.snapshot, "Read the changed page from the beginning.");
        return publicView(view);
      }
      case "page.act": return act(input, await current(input));
      case "page.prepare": return prepare(input, await current(input));
      case "page.release": return release(input);
      default: requireThat(false, "Unknown browser operation.");
    }
  }

  return { call(input) {
    const next = tail.then(() => handle(input));
    tail = next.catch(() => {});
    return next;
  }};
}

function publicView({ tree, ...value }) {
  return value;
}
