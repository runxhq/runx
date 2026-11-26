#!/usr/bin/env node
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { lstat, mkdir, readFile, realpath, stat as fileStat, unlink } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { dirname, isAbsolute, join } from "node:path";
import { connect } from "node:net";
import { createInterface } from "node:readline";
import { chromium } from "playwright-core";
import { browserSession, Refused, sitePolicy } from "./session.mjs";
import { liveView } from "./view.mjs";

// A config names the binding's sites, profile and browser. Started by the
// connector for a binding with no executor running (`--binding <name>`), it
// is that binding's own headless browser for any public site, and it stops
// once its last run is done.
const started = process.argv[2] === "--binding";
const config = started
  ? { binding: process.argv[3], backend: "local" }
  : JSON.parse(await readFile(process.argv[2], "utf8"));
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

const sessions = new Map();
const inFlight = new Set();
// Pages take the window's size, as they do in a person's browser.
const contextOptions = { serviceWorkers: "block", acceptDownloads: false, viewport: null };
const runx = process.env.RUNX_BROWSER_RUNTIME || "runx";
const maximum = config.maxContexts ?? 16;
if (!Number.isSafeInteger(maximum) || maximum < 1 || maximum > 64)
  throw Error("Choose between 1 and 64 resident browser contexts per binding.");
let resident = 0;
let reader;
let browser;
let launch;
let seed;
let agent;
const headless = config.headless !== false;
const options = {
  headless,
  ...(config.backend === "local" ? await installed() : {}),
  // A person's browser, which pages don't see as automated. Headless, it has
  // no screen of its own: it sits on a desktop of this machine's kind.
  args: ["--disable-blink-features=AutomationControlled", ...(headless ? desktop() : [])],
};

// The browser the binding names, else the pinned Chromium, else this
// machine's Chrome, else the pinned Chromium installed now, once.
async function installed() {
  if (config.executablePath) return { executablePath: config.executablePath };
  if (config.channel) return { channel: config.channel };
  if (existsSync(chromium.executablePath())) return { channel: "chromium" };
  const chrome = await chromium.launch({ channel: "chrome", headless: true }).catch(() => null);
  if (chrome) {
    await chrome.close();
    return { channel: "chrome" };
  }
  const cli = join(dirname(createRequire(import.meta.url).resolve("playwright-core/package.json")), "cli.js");
  const install = spawn(process.execPath, [cli, "install", "chromium"], { stdio: "ignore" });
  const [code] = await new Promise((resolve, reject) => {
    install.once("error", reject);
    install.once("exit", (...ending) => resolve(ending));
  });
  if (code !== 0) throw Error("Chromium could not be installed for the browser.");
  return { channel: "chromium" };
}

function desktop() {
  return process.platform === "darwin"
    ? ["--window-size=1512,945", "--screen-info={3024x1964 devicePixelRatio=2 workAreaTop=76}"]
    : ["--window-size=1920,1048", "--screen-info={1920x1080 workAreaTop=32}"];
}

// Headless Chrome names itself in its user agent, and sites turn that away.
// It's the same browser as Chrome in a window, so it says what that one does.
async function launchLocal() {
  if (!headless) return chromium.launch(options);
  agent ??= await windowedAgent();
  return chromium.launch({ ...options, args: [...options.args, `--user-agent=${agent}`] });
}

async function windowedAgent() {
  const probe = await chromium.launch(options);
  try {
    const { userAgent } = await (await probe.newBrowserCDPSession()).send("Browser.getVersion");
    return userAgent.replace("HeadlessChrome/", "Chrome/");
  } finally { await probe.close(); }
}
// A dedicated profile brings its sign-ins. A site account probe, when the
// binding names one, pins which account it is.
if (config.profile && !/^[a-z0-9_-]{1,64}$/.test(config.profile)) {
  throw Error("A dedicated profile needs a valid name.");
}

async function chromiumForBinding() {
  if (browser?.isConnected()) return browser;
  launch ??= config.backend === "cdp"
    ? chromium.connectOverCDP(process.env.RUNX_BROWSER_CDP, { timeout: 10_000 })
    : launchLocal();
  try { browser = await launch; }
  finally { launch = undefined; }
  return browser;
}

async function storageState() {
  if (!config.profile) return undefined;
  seed ??= readProfile();
  try { return await seed; }
  catch (error) { seed = undefined; throw error; }
}

async function readProfile() {
  const source = await chromium.launchPersistentContext(join(root, "profiles", config.profile), {
    ...options,
    ...contextOptions,
  });
  try {
    const state = await source.storageState({ indexedDB: true });
    // A binding that names no sites goes where a person's browser does, so
    // it brings the whole signed-in profile: a provider's sign-in included.
    if (!policy.origins) return state;
    const hosts = [...policy.origins].map(origin => new URL(origin).hostname);
    return {
      cookies: state.cookies.filter(({ domain }) => {
        const host = domain.replace(/^\./, "");
        return hosts.some(name => name === host || name.endsWith("." + host));
      }),
      origins: state.origins.filter(({ origin }) => policy.origins.has(origin)),
    };
  } finally { await source.close(); }
}

async function open() {
  if (resident >= maximum)
    throw new Refused("This browser binding is at its context limit. Finish an open run before starting another.");
  resident++;
  let context;
  try {
    const storage = await storageState();
    const chromium = await chromiumForBinding();
    context = await chromium.newContext({ ...contextOptions, ...(storage && { storageState: storage }) });
    context.once("close", () => { resident--; });
    return { context, driver: await browserSession(context, policy, undefined, profile) };
  } catch (error) {
    if (context) await context.close();
    else resident--;
    throw error;
  }
}

async function sessionFor(scope, operation, run) {
  let session = sessions.get(scope);
  if (typeof run?.id !== "string" || !/^[a-zA-Z0-9_.:-]{1,200}$/.test(run.id) ||
      typeof run.store !== "string" || !isAbsolute(run.store))
    throw new Refused("runx must identify the root run and its store.");
  if (session && (run.id !== session.run.id || run.store !== session.run.store))
    throw new Refused("The browser session belongs to another run.");
  if (!session) {
    if (!["page.open", "search.query", "session"].includes(operation))
      throw new Refused("The workflow's browser session is unavailable. Reopen the site to inspect it. Do not retry an unconfirmed submission.");
    session = { ...await open(), run };
    sessions.set(scope, session);
    session.context.once("close", () => {
      if (sessions.get(scope) === session) sessions.delete(scope);
      if (!sessions.size) stopReader();
      settle();
    });
  }
  return session.driver;
}

function stopReader() {
  reader?.child.stdin.end();
  reader = undefined;
}

async function sealedRoots(store, roots) {
  if (!reader) {
    const child = spawn(runx, ["--browser-sealed-roots"], { stdio: ["pipe", "pipe", "ignore"] });
    const lines = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
    child.on("error", () => child.stdout.destroy());
    child.stdin.on("error", () => child.stdout.destroy());
    reader = { child, lines };
  }
  const { child, lines } = reader;
  let timer;
  try {
    child.stdin.write(JSON.stringify([store, roots]) + "\n");
    const line = await Promise.race([
      lines.next(),
      new Promise(resolve => { timer = setTimeout(() => resolve({ done: true }), 2_000); }),
    ]);
    if (line.done || line.value.length > 32_768) throw Error("Browser state reader unavailable.");
    return JSON.parse(line.value);
  } catch {
    if (reader?.child === child) reader = undefined;
    child.kill();
    return null;
  } finally {
    clearTimeout(timer);
  }
}

async function reclaimSealed() {
  const byStore = new Map();
  for (const [scope, session] of sessions) {
    if (inFlight.has(scope)) continue;
    const group = byStore.get(session.run.store) ?? [];
    group.push([scope, session]);
    byStore.set(session.run.store, group);
  }
  for (const [store, candidates] of byStore) {
    // SQLite commits land in the WAL. Check the database too for resets
    // and checkpoints. The native reader stays idle while those files
    // stand, with a periodic check as a fallback.
    const stamp = (await Promise.all([store, store + "-wal"].map(async file => {
      const info = await fileStat(file, { bigint: true }).catch(() => null);
      return info && `${info.ino}:${info.size}:${info.mtimeNs}:${info.ctimeNs}`;
    }))).join("/");
    const now = Date.now();
    const group = candidates.filter(([, session]) => session.checked?.stamp !== stamp ||
      now - session.checked.at >= 60_000);
    for (let offset = 0; offset < group.length; offset += 64) {
      const batch = group.slice(offset, offset + 64);
      const keys = await sealedRoots(store, batch.map(([, session]) => session.run.id));
      // An unavailable reader cannot prove that a run is finished.
      if (!Array.isArray(keys)) continue;
      const sealed = new Set(keys);
      await Promise.all(batch.map(async ([scope, session]) => {
        if (inFlight.has(scope) || sessions.get(scope) !== session) return;
        if (!sealed.has(session.run.id)) {
          session.checked = { stamp, at: now };
          return;
        }
        inFlight.add(scope);
        try { await session.context.close(); }
        catch { /* Recheck if the context still exists. */ }
        finally { inFlight.delete(scope); }
      }));
    }
  }
}

let sweeping = false;
const sweep = setInterval(async () => {
  if (sweeping || !sessions.size) return;
  sweeping = true;
  try { await reclaimSealed(); }
  finally { sweeping = false; }
}, 1_000).unref();

async function principal() {
  if (!policy.account) return { id: profile, login: "guest" };
  const probe = await open();
  try { return await probe.driver.call({ connector_operation: "session" }); }
  finally { await probe.context.close(); }
}

const operations = new Set([
  "session",
  "page.open",
  "page.read",
  "page.act",
  "page.release",
  "search.query",
]);
// Started for a binding, the executor stops once it holds nothing open: no
// run's context and no call. A call that reaches it as it stops is told so,
// before anything is done, and the connector starts another.
let calls = 0;
let stopping = false;
function settle() {
  if (!started || stopping || calls || sessions.size) return;
  stopping = true;
  void close().then(() => process.exit(0), () => process.exit(1));
}

const view = liveView(sessions);
const server = createServer(async (req, res) => {
  const response = (code, value) => {
    res.writeHead(code, {
      "content-type": "application/json",
      "cache-control": "no-store",
    });
    res.end(JSON.stringify(value));
  };
  if (stopping) return response(503, { refused: "The browser is stopping. Call again." });
  calls++;
  res.once("close", () => { calls--; settle(); });
  // The person's view of a run's tab, which runx's host reads (control.md §12).
  if (req.url?.startsWith("/view/")) return view(req, res);
  const operation = req.url?.slice(1);
  if (req.method !== "POST" || !operations.has(operation))
    return response(404, { refused: "Unknown operation." });
  // A caller that hangs up ends any wait on the site, such as its bot check.
  const ended = new AbortController();
  res.once("close", () => { if (!res.writableEnded) ended.abort(); });
  let held;
  try {
    let length = 0;
    const chunks = [];
    for await (const chunk of req) {
      if ((length += chunk.length) > 65536)
        return response(413, { refused: "Browser input is too large." });
      chunks.push(chunk);
    }
    const input = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const scope = input.session_workflow;
    if (operation === "session" && scope === undefined)
      return response(200, await principal());
    if (typeof scope !== "string" || !/^[a-f0-9]{64}$/.test(scope))
      return response(409, { refused: "runx must bind this browser operation to its workflow." });
    if (inFlight.has(scope))
      return response(409, { refused: "Another operation is still running in this workflow." });
    inFlight.add(scope);
    held = scope;
    if (operation === "session" && !policy.account)
      return response(200, await principal());
    const driver = await sessionFor(scope, operation, input.session_run);
    response(200, await driver.call({ ...input, connector_operation: operation }, ended.signal));
  } catch (error) {
    if (error instanceof Refused)
      return response(409, { refused: error.message });
    // Playwright errors can contain page text, filled secrets, or a CDP URL.
    response(502, {
      error:
        "Browser operation did not complete. Inspect current site state before retrying.",
    });
  } finally {
    if (held !== undefined) inFlight.delete(held);
  }
});
server.requestTimeout = 30000;
const socket = join(root, profile + ".sock");
function listen() {
  return new Promise((resolve, reject) => {
    const failed = error => { server.off("listening", ready); reject(error); };
    const ready = () => { server.off("error", failed); resolve(); };
    server.once("error", failed);
    server.once("listening", ready);
    server.listen(socket);
  });
}

function occupied() {
  return new Promise((resolve, reject) => {
    const probe = connect(socket);
    probe.once("connect", () => { probe.destroy(); resolve(true); });
    probe.once("error", error => {
      if (["ECONNREFUSED", "ENOENT"].includes(error.code)) resolve(false);
      else reject(error);
    });
    probe.setTimeout(1000, () => probe.destroy(new Error("The browser binding is still in use.")));
  });
}

try {
  await listen();
} catch (error) {
  if (error.code !== "EADDRINUSE") throw error;
  const held = await lstat(socket);
  if (!held.isSocket() || held.uid !== process.getuid() || await occupied()) throw error;
  const same = await lstat(socket);
  if (same.dev !== held.dev || same.ino !== held.ino) throw error;
  await unlink(socket);
  await listen();
}
process.stdout.write("ready\n");

async function close() {
  clearInterval(sweep);
  stopReader();
  server.close();
  for (const { context } of sessions.values()) await context.close().catch(() => {});
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
