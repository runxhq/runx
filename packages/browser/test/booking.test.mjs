import assert from "node:assert/strict";
import { createServer } from "node:http";
import { after, before, test } from "node:test";
import { chromium } from "playwright-core";
import { browserSession, sitePolicy } from "../src/session.mjs";

let site;
let origin;
let browser;
let records;
let lost;
let outside;
let forged;
before(async () => {
  site = createServer(async (request, response) => {
    response.setHeader("content-type", "text/html");
    const { url, method } = request;
    if (url === "/redirect") return response.writeHead(302, { location: "/outside" }).end();
    if (url === "/outside") {
      outside++;
      return response.end("External destination");
    }
    if (url === "/unexpected") {
      forged++;
      return response.end("Unexpected effect");
    }
    if (url === "/" && method === "GET") return response.end('<a href="/booking">Find a slot</a>');
    if (url === "/booking" && method === "GET") {
      return response.end(`<form method="post">
        <label>Guest <input name="guest" required></label>
        <label>Time <select name="time"><option>18:00</option><option>19:00</option></select></label>
        <button>Book</button>
      </form>
      <button id="explore">Explore nearby times</button>
      <script>explore.onclick = () => {
        fetch('/unexpected').catch(() => {});
        document.body.insertAdjacentHTML('beforeend', '<p>Nearby times</p>');
      };</script>`);
    }
    if (url === "/booking" && method === "POST") {
      let data = "";
      for await (const part of request) data += part;
      const id = 'R-' + (records.size + 1);
      records.set(id, Object.fromEntries(new URLSearchParams(data)));
      if (lost) return response.destroy();
      response.writeHead(201, { location: '/records/' + id });
      return response.end(`<a href="/records/${id}">View reservation</a>`);
    }
    if (url.startsWith("/records/") && records.has(url.split("/").at(-1))) {
      const id = url.split("/").at(-1);
      return response.end(`<h1>${id}</h1><p>${records.get(id).time}</p>`);
    }
    response.writeHead(404).end();
  });
  await new Promise(resolve => site.listen(0, "127.0.0.1", resolve));
  origin = "http://127.0.0.1:" + site.address().port;
  browser = await chromium.launch({ channel: "chromium", headless: process.env.BROWSER_TEST_HEADED !== "1" });
});
after(async () => {
  await browser?.close();
  site?.closeAllConnections();
  if (site) await new Promise(resolve => site.close(resolve));
});

function reference(view, name) {
  const row = view.page.split("\n").find(line => line.includes(name));
  assert.ok(row, view.page);
  const ref = row.match(/\[ref=((?:f\d+)?e\d+)\]/)?.[1];
  assert.ok(ref, row);
  return ref;
}

async function visit(t) {
  records = new Map();
  outside = 0;
  forged = 0;
  lost = false;
  const context = await browser.newContext({ serviceWorkers: "block" });
  t.after(() => context.close());
  const session = await browserSession(context, sitePolicy({ origin, submitOrigins: [origin] }));
  const call = (op, args = {}) => session.call({ ...args, connector_operation: op });
  const begin = await call("page.open", { url: origin + "/" });
  const action = (view, name, action, value) => call("page.act", {
    url: view.url, snapshot: view.snapshot, ref: reference(view, name), action, value,
  });
  const form = await action(begin, '"Find a slot"', "follow");
  const filled = await action(form, '"Guest"', "fill", "Morgan Test");
  const selected = await action(filled, '"Time"', "select", "19:00");
  const proposal = await call("page.prepare", {
    url: selected.url, snapshot: selected.snapshot, ref: reference(selected, '"Book"'),
  });
  const identity = await call("session");
  return { call, action, context, selected, proposal,
    approved: { ...proposal, phase: "perform", session_account: identity.login } };
}

test("a fresh generic form is approved once and its record can be opened independently", async t => {
  const { call, approved } = await visit(t);
  assert.equal(records.size, 0);
  assert.deepEqual(approved.terms, { guest: "Morgan Test", time: "19:00" });
  const [accepted, duplicate] = await Promise.allSettled([
    call("page.release", approved), call("page.release", approved),
  ]);
  assert.equal(accepted.status, "fulfilled");
  assert.equal(duplicate.status, "rejected");
  assert.equal(records.size, 1);
  assert.equal(accepted.value.response_status, 201);
  const readback = await call("page.open", { url: accepted.value.confirmation_url });
  assert.match(readback.page, /R-1/);
  assert.match(readback.page, /19:00/);
});

test("changing the visible terms or destination invalidates the exact proposal", async t => {
  const { call, approved, context } = await visit(t);
  await assert.rejects(call("page.release", { ...approved, destination: origin + "/outside" }),
    /different/);
  await context.pages()[0].locator("select").selectOption({ label: "18:00" });
  await assert.rejects(call("page.release", approved), /changed/);
  assert.equal(records.size, 0);
  assert.equal(outside, 0);
});

test("exploration clicks cannot submit or fetch an unreviewed effect route", async t => {
  const { action, selected, call } = await visit(t);
  await assert.rejects(action(selected, '"Book"', "click"), /Prepare/);
  const fresh = await action(await call("page.read"), '"Explore nearby times"', "click");
  assert.match(fresh.page, /Nearby times/);
  assert.equal(records.size, 0);
  assert.equal(forged, 0);
});

test("a lost acknowledgement remains unknown with one record and no replay", async t => {
  const { call, approved } = await visit(t);
  lost = true;
  await assert.rejects(call("page.release", approved), /outcome unknown/);
  assert.equal(records.size, 1);
  await assert.rejects(call("page.release", approved));
  assert.equal(records.size, 1);
});

test("the browser denies automatic redirects before the target receives traffic", async t => {
  const { call } = await visit(t);
  await assert.rejects(call("page.open", { url: origin + "/redirect" }));
  assert.equal(outside, 0);
});

test("a new browser context cannot submit an earlier context's approval", async t => {
  const earlier = await visit(t);
  const replacement = await visit(t);
  assert.notEqual(earlier.approved.snapshot, replacement.approved.snapshot);
  await assert.rejects(replacement.call("page.release", earlier.approved));
  assert.equal(records.size, 0);
});
