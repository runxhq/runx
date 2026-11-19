#!/usr/bin/env node
import { lstat, mkdir, readFile, realpath } from "node:fs/promises";
import { createServer } from "node:http";
import { join } from "node:path";
import { chromium } from "playwright-core";
import { browserSession, Refused, sitePolicy } from "./session.mjs";

const config = JSON.parse(await readFile(process.argv[2], "utf8"));
const policy = sitePolicy(config.site);
if (!["local", "cdp"].includes(config.backend))
  throw Error("Choose a local or pre-provisioned CDP browser.");
if (config.backend === "cdp" && !process.env.RUNX_BROWSER_CDP)
  throw Error("A dedicated CDP endpoint is required.");
if (config.backend === "cdp" && (config.profile || policy.account))
  throw Error("Use a dedicated local profile for authenticated actions.");
const profile = config.binding;
if (typeof profile !== "string" || !/^[a-z0-9][a-z0-9_-]{0,47}$/.test(profile))
  throw Error("A browser binding name is required.");
const root = join(await realpath("/tmp"), "runx-browser-" + process.getuid());
await mkdir(root, { mode: 0o700, recursive: true });
const stat = await lstat(root);
if (!stat.isDirectory() || stat.uid !== process.getuid() || stat.mode & 0o077)
  throw Error("The socket directory must be private.");

let context;
let browser;
let start;
let busy = false;
const anonymousContext = { serviceWorkers: "block", acceptDownloads: false };
async function open() {
  if (config.backend === "cdp") {
    // A pre-provisioned, exclusive browser. Its endpoint stays at the executor.
    browser = await chromium.connectOverCDP(process.env.RUNX_BROWSER_CDP, {
      timeout: 10_000,
    });
    context = await browser.newContext(anonymousContext);
  } else if (config.backend === "local") {
    const options = {
      headless: config.headless !== false,
      ...(config.executablePath
        ? { executablePath: config.executablePath }
        : { channel: config.channel ?? "chromium" }),
    };
    if (config.profile) {
      // The profile is provisioned only under this executor's private state.
      if (!/^[a-z0-9_-]{1,64}$/.test(config.profile))
        throw Error("Invalid dedicated profile name.");
      if (!policy.account)
        throw Error("A dedicated profile needs a site account probe.");
      context = await chromium.launchPersistentContext(
        join(root, "profiles", config.profile),
        {
          ...options,
          ...anonymousContext,
        },
      );
    } else {
      browser = await chromium.launch(options);
      context = await browser.newContext(anonymousContext);
    }
  } else throw Error("Choose a local or pre-provisioned CDP browser.");
  if (
    !context ||
    context.pages().length > 1 ||
    context.serviceWorkers().length ||
    context.pages().some((p) => p.url() !== "about:blank")
  )
    throw Error("A dedicated browser starting on a blank tab is required.");
  return browserSession(context, policy, context.pages()[0]);
}

const operations = new Set([
  "session",
  "page.open",
  "page.read",
  "page.act",
  "page.release",
  "page.prepare",
]);
const server = createServer(async (req, res) => {
  const response = (code, value) => {
    res.writeHead(code, {
      "content-type": "application/json",
      "cache-control": "no-store",
    });
    res.end(JSON.stringify(value));
  };
  const operation = req.url?.slice(1);
  if (req.method !== "POST" || !operations.has(operation))
    return response(404, { refused: "Unknown operation." });
  if (busy)
    return response(503, { error: "Another operation is still running." });
  busy = true;
  try {
    let length = 0;
    const chunks = [];
    for await (const chunk of req) {
      if ((length += chunk.length) > 65536)
        return response(413, { refused: "Browser input is too large." });
      chunks.push(chunk);
    }
    const input = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    if (!start) start = open();
    const driver = await start;
    const result = await driver.call({
      ...input,
      connector_operation: operation,
    });
    response(200, result);
  } catch (error) {
    if (error instanceof Refused)
      return response(409, { refused: error.message });
    // Playwright errors can contain page text, filled secrets, or a CDP URL.
    response(502, {
      error:
        "Browser operation did not complete. Inspect current site state before retrying.",
    });
  } finally {
    busy = false;
  }
});
server.requestTimeout = 30000;
const socket = join(root, profile + ".sock");
server.listen(socket, () => process.stdout.write("ready\n"));
async function close() {
  server.close();
  // Closing a Playwright CDP client disconnects it from the provider's browser.
  if (context) await context.close();
  if (browser) await browser.close();
  server.closeAllConnections();
}
for (const signal of ["SIGTERM", "SIGINT"]) {
  process.once(signal, () => {
    void close().catch(() => {
      process.exitCode = 1;
    });
  });
}
