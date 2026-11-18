// The rehearsal mail provider. Every operation of the mailsim connector, both
// its sides, over append-only JSON-lines files in its own folder (the
// instance's `dir`, outside every workspace that reaches it) or else the
// workspace's world/ folder:
//   sent.jsonl          what was sent, one message per send key, which the
//                       outside side reads as email.sent envelopes
//   inbound.jsonl       what arrived, each as the mail slot's event, the
//                       Standard Webhooks envelope {id, type, timestamp,
//                       data}, which inbox.read gives back as it is; the
//                       outside side adds it, or the rehearsal's world writes it
//   suppressions.jsonl  addresses nothing is sent to again
// Each message sent is posted to the world loop's webhook, and each one added
// to the company loop's, as a provider posts webhooks. A post is a doorbell:
// each side also reads after its own cursor, so a lost post costs only time.
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

const input = JSON.parse(process.env.RUNX_INPUTS_JSON ?? readFileSync(process.env.RUNX_INPUTS_PATH, "utf8"));
const parameters = input.parameters ?? {};
const world = parameters.dir ?? join(process.env.RUNX_CWD ?? ".", "world");
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

/** Whether `address` has written since `since`, to the millisecond, as the world delivered it. */
const wroteSince = (address, since) =>
  lines("inbound.jsonl").some((event) => event.type === "email.received" && lower(event.data?.from) === lower(address) && Date.parse(event.timestamp) > Date.parse(since));

/** Whether `held` is the message `input` asks to send, every field of it. */
const same = (held, input) => Object.entries(asked(input)).every(([field, value]) => held[field] === value);

/** A message as the outside side reads it: the email.sent envelope. */
const sentEnvelope = (message) => ({
  id: message.id,
  type: "email.sent",
  timestamp: message.sent_at,
  data: {
    message_id: message.id,
    thread_id: message.thread_id,
    rfc_message_id: message.rfc_message_id,
    in_reply_to: message.in_reply_to,
    from: message.from_email,
    from_name: message.from_name,
    to: message.to,
    subject: message.subject,
    text: message.body,
    outreach: message.outreach === true,
  },
});

/** Ring `url`'s doorbell with `envelope`, as a provider posts a webhook. A lost post is read later; it never fails the call. */
async function post(url, envelope) {
  if (!url) return;
  try {
    await fetch(url, { method: "POST", headers: { "content-type": "application/json", "webhook-id": envelope.id }, body: JSON.stringify(envelope), signal: AbortSignal.timeout(5000) });
  } catch {
    // Read after the reader's cursor on its next wake.
  }
}

async function send(input) {
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
  await post(parameters.world_webhook, sentEnvelope(message));
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

/** One page of `all` after a cursor, which is how many came before it. */
function page(all, input) {
  const after = Number(input.after ?? "0");
  const limit = input.limit ?? 100;
  const events = all.slice(after, after + limit);
  return { events, cursor: String(after + events.length), more: after + events.length < all.length };
}

/** What arrived, each envelope as delivered, without the key it was added under. */
const inbox = (input) => page(lines("inbound.jsonl").map(({ added_under: _key, ...envelope }) => envelope), input);

/** The outside side: what was sent, each as its email.sent envelope. */
const outbox = (input) => page(lines("sent.jsonl").map(sentEnvelope), input);

/**
 * The outside side: one thing a person written to sends back, once per key.
 * Mail received is threaded on the message it answers; a bounce or a
 * complaint is for the address written to, and a complaint suppresses it, as
 * a provider does; an unsubscribe is a click on the link, which suppresses the
 * address and is never mail.
 */
async function add(input) {
  const key = input.idempotency_key;
  const held = lines("inbound.jsonl").find((event) => event.added_under === key) ?? lines("suppressions.jsonl").find((entry) => entry.added_under === key);
  const answer = (entry) => (entry.type ? { id: entry.id, type: entry.type } : { id: entry.id, type: "unsubscribe" });
  if (input.phase === "readback") {
    if (!held) throw new Error(`nothing was added under key ${key}`);
    return answer(held);
  }
  if (input.phase === "lookup") return held ? answer(held) : null;
  if (held) return answer(held);
  const id = `in_${createHash("sha256").update(key).digest("hex").slice(0, 16)}`;
  if (input.type === "unsubscribe") {
    const address = lower(input.address ?? input.from);
    append("suppressions.jsonl", { id, address, reason: "unsubscribe", at: new Date().toISOString(), added_under: key });
    return { id, type: "unsubscribe" };
  }
  const parent = input.in_reply_to ? lines("sent.jsonl").find((message) => message.rfc_message_id === input.in_reply_to) : null;
  const thread_id = parent?.thread_id ?? input.thread_id ?? null;
  const at = new Date().toISOString();
  const person = lower(input.type === "email.received" ? input.from : input.to);
  const data =
    input.type === "email.received"
      ? { from: person, to: lower(input.to), thread_id, rfc_message_id: `<${id}@${person.split("@")[1]}>`, in_reply_to: input.in_reply_to ?? null, subject: input.subject ?? "", text: input.text ?? "", ...(input.auto_submitted ? { auto_submitted: input.auto_submitted } : {}) }
      : { to: person, thread_id, subject: input.subject ?? "", ...(input.type === "email.bounced" ? { bounce: { type: "hard" } } : {}) };
  const envelope = { id, type: input.type, timestamp: at, data };
  appendFileSync(file("inbound.jsonl"), `${JSON.stringify({ ...envelope, added_under: key })}\n`);
  if (input.type === "email.complained" && !suppression(person).suppressed) append("suppressions.jsonl", { address: person, reason: "complaint", at });
  await post(parameters.company_webhook, envelope);
  return { id, type: input.type };
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
  "outbox.read": outbox,
  "inbound.add": add,
};

// The world issues one key; a call under any other is refused, as a real
// provider refuses a wrong key.
const key = existsSync(file("key")) ? readFileSync(file("key"), "utf8").trim() : null;
if (!key || process.env.MAILSIM_API_KEY !== key) {
  process.stderr.write("mailsim refused the key: it isn't world/key's\n");
  process.exit(2);
}

const operation = operations[input.connector_operation];
if (!operation) {
  process.stderr.write(`mailsim has no operation ${input.connector_operation}\n`);
  process.exit(2);
}
process.stdout.write(`${JSON.stringify(await operation(input))}\n`);
