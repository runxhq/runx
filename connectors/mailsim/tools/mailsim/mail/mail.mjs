// The rehearsal mail provider. Every operation of the mailsim connector,
// over append-only JSON-lines files in the workspace's world/ folder:
//   sent.jsonl          what was sent, one message per send key
//   inbound.jsonl       what arrived, written by the world simulator, each as
//                       the mail slot's event, the Standard Webhooks envelope
//                       {id, type, timestamp, data}, which inbox.read gives
//                       back as it is
//   suppressions.jsonl  addresses nothing is sent to again
// It answers as a provider does: a send under a key it holds replays the
// original, and one under that key with another message is refused; a
// suppressed recipient is refused before anything is sent, and so is one who
// has written since the time a send carries as unless_inbound_since; a write
// is read back by its key; who may be written to is the provider's verdict.
// It sends at once, so every message it holds is sent.
//
// A key's sends come one at a time: runx's custody of an act attempts it once
// at a time, in any run, refusing a second attempt before it's sent, so
// checking a key and appending under it need no lock of their own. Sends
// under other keys each append one whole line.

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

/** What arrived, each envelope as a message of its thread. */
const arrived = () =>
  lines("inbound.jsonl").map((event) => ({
    id: event.id,
    type: event.type,
    thread_id: event.data?.thread_id ?? null,
    rfc_message_id: event.data?.rfc_message_id ?? null,
    from: event.data?.from ?? null,
    to: event.data?.to ?? null,
    subject: event.data?.subject ?? null,
    body: event.data?.text ?? null,
    received_at: event.timestamp,
  }));

function sentByKey(key) {
  return lines("sent.jsonl").find((message) => message.idempotency_key === key) ?? null;
}

/** The message `input` asks to send, as it's held. */
const asked = (input) => ({
  to: lower(input.to),
  from_email: lower(input.from_email),
  from_name: input.from_name,
  reply_to: input.reply_to ? lower(input.reply_to) : null,
  subject: input.subject,
  body: input.body,
  in_reply_to: input.in_reply_to ?? null,
  outreach: input.outreach === true,
  unless_inbound_since: input.unless_inbound_since ?? null,
});

/** Whether `address` has written since `since`, to the millisecond. */
const wroteSince = (address, since) =>
  lines("inbound.jsonl").some((event) => event.type === "email.received" && lower(event.data?.from) === lower(address) && Date.parse(event.timestamp) > Date.parse(since));

/** Whether `held` is the message `input` asks to send, every field of it. */
const same = (held, input) => Object.entries(asked(input)).every(([field, value]) => held[field] === value);

function send(input) {
  const key = input.idempotency_key;
  if (input.phase === "readback") {
    const message = sentByKey(key);
    if (!message) throw new Error(`no message is held under key ${key}`);
    return publicMessage(message);
  }
  // What was sent under the key, for a send whose answer was lost: the
  // message as its send answered, or null when none was.
  if (input.phase === "lookup") {
    const message = sentByKey(key);
    return message ? publicMessage(message) : null;
  }
  const held = sentByKey(key);
  // One message per key: the same one again is the original; another is refused.
  if (held) return same(held, input) ? publicMessage(held) : { refused: "key_reused" };
  if (suppression(input.to).suppressed) return { refused: "suppressed" };
  if (input.unless_inbound_since && wroteSince(input.to, input.unless_inbound_since)) return { refused: "inbound_since" };
  const id = `msg_${createHash("sha256").update(key).digest("hex").slice(0, 16)}`;
  const parent = input.in_reply_to
    ? [...lines("sent.jsonl"), ...arrived()].find((message) => message.rfc_message_id === input.in_reply_to)
    : null;
  const message = {
    id,
    idempotency_key: key,
    thread_id: parent?.thread_id ?? `thr_${id.slice(4)}`,
    rfc_message_id: `<${id}@mailsim.local>`,
    ...asked(input),
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
  return { item: item ? { ...publicMessage(item), status: "sent" } : null };
}

function thread(input) {
  const messages = [
    ...lines("sent.jsonl").map((message) => ({ direction: "out", at: message.sent_at, ...publicMessage(message) })),
    ...arrived().map((message) => ({ direction: "in", at: message.received_at, ...message })),
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

/**
 * Whether the sender may write to `address` now, by this provider's policy:
 * never to a suppressed address; to one the sender met, serves, was given
 * permission by or was referred to; to a published one only with the page it
 * was published on; to anything else, not yet.
 */
function eligibility(input) {
  const address = lower(input.address);
  const policy = "mailsim-basis-v1";
  const kind = input.basis?.kind;
  const evidence = String(input.basis?.evidence ?? "").trim();
  const verdict = (status, reason) => ({
    address,
    status,
    policy_id: policy,
    evidence_ids: [`basis:${kind ?? "none"}`, ...(evidence ? [`evidence:${createHash("sha256").update(evidence).digest("hex").slice(0, 16)}`] : [])],
    reason,
  });
  if (suppression(address).suppressed) return verdict("ineligible", "The address is suppressed.");
  if (["met", "customer", "opted_in", "referred"].includes(kind)) return verdict("eligible", `The sender's basis: ${kind}.`);
  if (kind === "published") {
    return /^https?:\/\/\S+$/.test(evidence.split(/\s/)[0] ?? "")
      ? verdict("eligible", "Published, with the page it was published on.")
      : verdict("unknown", "Published, but no page shows where.");
  }
  return verdict("unknown", "No basis this policy accepts.");
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
  "eligibility.read": eligibility,
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
