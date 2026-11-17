// The rehearsal mail provider. Every operation of the mailsim connector,
// over append-only JSON-lines files in the workspace's world/ folder:
//   sent.jsonl          what was sent, one message per send key
//   inbound.jsonl       what arrived, written by the world simulator
//   suppressions.jsonl  addresses nothing is sent to again
// It answers as a provider does: a send under a key it holds replays the
// original; a suppressed recipient, or one who wrote since a given time,
// is refused before anything is sent; a write is read back by its key.

import { appendFileSync, existsSync, mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";

const world = join(process.env.RUNX_CWD ?? ".", "world");
mkdirSync(world, { recursive: true });
const file = (name) => join(world, name);

function lines(name) {
  if (!existsSync(file(name))) return [];
  return readFileSync(file(name), "utf8")
    .split("\n")
    .filter((line) => line.trim() !== "")
    .map((line) => JSON.parse(line));
}

function append(name, record) {
  appendFileSync(file(name), `${JSON.stringify(record)}\n`);
}

const lower = (address) => String(address ?? "").trim().toLowerCase();

function suppression(address) {
  const held = lines("suppressions.jsonl").filter((entry) => lower(entry.address) === lower(address));
  const last = held.at(-1);
  return { address: lower(address), suppressed: Boolean(last), reason: last?.reason ?? null };
}

function sentByKey(key) {
  return lines("sent.jsonl").find((message) => message.idempotency_key === key) ?? null;
}

function send(input) {
  const key = input.idempotency_key;
  if (input.phase === "readback") {
    const message = sentByKey(key);
    if (!message) throw new Error(`no message is held under key ${key}`);
    return publicMessage(message);
  }
  const held = sentByKey(key);
  if (held) return publicMessage(held);
  if (suppression(input.to).suppressed) return { refused: "suppressed" };
  if (input.unless_inbound_since) {
    const since = Date.parse(input.unless_inbound_since);
    const wrote = lines("inbound.jsonl").some(
      (event) => lower(event.from) === lower(input.to) && Date.parse(event.received_at) > since,
    );
    if (wrote) return { refused: "inbound_since" };
  }
  const id = `msg_${createHash("sha256").update(key).digest("hex").slice(0, 16)}`;
  const parent = input.in_reply_to
    ? [...lines("sent.jsonl"), ...lines("inbound.jsonl")].find((message) => message.rfc_message_id === input.in_reply_to)
    : null;
  const message = {
    id,
    idempotency_key: key,
    thread_id: parent?.thread_id ?? `thr_${id.slice(4)}`,
    rfc_message_id: `<${id}@mailsim.local>`,
    to: lower(input.to),
    from_email: lower(input.from_email),
    from_name: input.from_name,
    reply_to: input.reply_to ? lower(input.reply_to) : null,
    subject: input.subject,
    body: input.body,
    in_reply_to: input.in_reply_to ?? null,
    sent_at: new Date().toISOString(),
  };
  append("sent.jsonl", message);
  return publicMessage(message);
}

function publicMessage(message) {
  const { idempotency_key: _key, ...shown } = message;
  return shown;
}

function read(input) {
  const sent = lines("sent.jsonl");
  const item = input.idempotency_key
    ? sent.find((message) => message.idempotency_key === input.idempotency_key)
    : sent.find((message) => message.id === input.id);
  return { item: item ? publicMessage(item) : null };
}

function thread(input) {
  const messages = [
    ...lines("sent.jsonl").map((message) => ({ direction: "out", at: message.sent_at, ...publicMessage(message) })),
    ...lines("inbound.jsonl").map((event) => ({ direction: "in", at: event.received_at, ...event })),
  ]
    .filter((message) => message.thread_id === input.thread_id)
    .sort((left, right) => Date.parse(left.at) - Date.parse(right.at));
  return { thread_id: input.thread_id, messages };
}

function inbox(input) {
  const all = lines("inbound.jsonl");
  const after = Number(input.after ?? "0");
  const limit = input.limit ?? 100;
  const events = all.slice(after, after + limit);
  return { events, cursor: String(after + events.length), more: after + events.length < all.length };
}

function suppress(input) {
  if (input.phase === "readback") {
    const held = suppression(input.address);
    if (!held.suppressed) throw new Error(`${input.address} isn't suppressed`);
    return held;
  }
  if (!suppression(input.address).suppressed) {
    append("suppressions.jsonl", { address: lower(input.address), reason: input.reason, at: new Date().toISOString() });
  }
  return suppression(input.address);
}

const operations = {
  "message.send": send,
  "message.read": read,
  "thread.read": thread,
  "inbox.read": inbox,
  "suppression.read": (input) => suppression(input.address),
  "suppression.add": suppress,
};

// The world issues one key; a call under any other is refused, as a real
// provider refuses a wrong key.
const key = existsSync(file("key")) ? readFileSync(file("key"), "utf8").trim() : null;
if (!key || process.env.MAILSIM_API_KEY !== key) {
  process.stderr.write("mailsim refused the key: it isn't world/key's\n");
  process.exit(2);
}

const input = JSON.parse(process.env.RUNX_INPUTS_JSON ?? readFileSync(process.env.RUNX_INPUTS_PATH, "utf8"));
const operation = operations[input.connector_operation];
if (!operation) {
  process.stderr.write(`mailsim has no operation ${input.connector_operation}\n`);
  process.exit(2);
}
process.stdout.write(`${JSON.stringify(operation(input))}\n`);
