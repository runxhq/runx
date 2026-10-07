import { array, positiveInteger, record, text } from "./twitter-execution-values.mjs";

const MAX_ACTS = 50;
export const MAX_ENGAGEMENT_ACTS = 10;
export const MAX_THREAD_SEGMENTS = 25;

// Each act kind, the connector operation that performs it, and how. A
// published act (post, reply, quote, thread segment) is one operation call;
// the other kinds repeat one operation over a contiguous run of same-kind
// acts, keyed by the act's target.
const ACT_KINDS = {
  post: { consequence: "public_send", publish: true },
  reply: { consequence: "public_send", publish: true },
  quote: { consequence: "public_send", publish: true },
  thread: { consequence: "public_send", publish: true },
  repost: { consequence: "public_send", engagement: true, operation: "posts.repost", list: "post_ids", target: "post_id", state: ["retweeted", true] },
  delete_post: { consequence: "live_mutation", operation: "posts.delete", list: "post_ids", target: "post_id", state: ["deleted", true], self: false },
  unfollow: { consequence: "live_mutation", operation: "users.unfollow", list: "user_ids", target: "target_user_id", state: ["following", false] },
  mute: { consequence: "live_mutation", operation: "users.mute", list: "user_ids", target: "target_user_id", state: ["muting", true] },
  block: { consequence: "live_mutation", operation: "users.block", list: "user_ids", target: "target_user_id", state: ["blocking", true] },
  follow: { consequence: "live_mutation", engagement: true, operation: "users.follow", list: "user_ids", target: "target_user_id", state: ["following", true] },
  like: { consequence: "live_mutation", engagement: true, operation: "posts.like", list: "post_ids", target: "post_id", state: ["liked", true] },
};

const VERBS = {
  post: "Publish a post",
  reply: "Publish a reply",
  quote: "Publish a quote post",
  thread: "Publish a thread segment",
  repost: "Repost",
  delete_post: "Delete",
  unfollow: "Unfollow",
  mute: "Mute",
  block: "Block",
  follow: "Follow",
  like: "Like",
};

export function actKind(kind) {
  return ACT_KINDS[kind] ?? null;
}

export function executionActLimit(value) {
  return Math.min(positiveInteger(value, MAX_ACTS), MAX_ACTS);
}

// The next batch from the durable cursor: one published act or thread
// segment, or the longest contiguous run of one repeated kind, up to
// `maxActs`, with no target twice.
export function selectBatch(acts, progress, maxActs) {
  const first = acts[progress.nextActIndex];
  const kind = ACT_KINDS[first.kind];
  if (kind.publish) return publishBatch(first, progress);
  const selected = [];
  const targets = new Set();
  for (let index = progress.nextActIndex; index < acts.length; index += 1) {
    const act = acts[index];
    const target = text(record(act.params)[kind.target]);
    if (act.kind !== first.kind || selected.length >= maxActs || targets.has(target)) break;
    targets.add(target);
    selected.push(batchAct(act, index, target));
  }
  return {
    kind: first.kind,
    operation: kind.operation,
    input: { [kind.list]: selected.map((act) => act.target) },
    self: kind.self !== false,
    acts: selected,
    segment: null,
  };
}

function publishBatch(act, progress) {
  const params = record(act.params);
  const index = progress.nextActIndex;
  if (act.kind !== "thread") {
    const [operation, input] = {
      post: ["post.create", { text: params.text }],
      reply: ["post.reply", { text: params.text, in_reply_to: params.in_reply_to }],
      quote: ["post.quote", { text: params.text, quote_of: params.quote_of }],
    }[act.kind];
    return {
      kind: act.kind,
      operation,
      input,
      self: false,
      acts: [batchAct(act, index, null)],
      segment: null,
    };
  }
  const texts = array(params.texts);
  const active = progress.activeThread?.act_index === index ? progress.activeThread : null;
  const segment = active ? active.next_segment_index : 0;
  const replyTo = active ? active.in_reply_to : text(params.in_reply_to);
  return {
    kind: "thread",
    operation: replyTo ? "post.reply" : "post.create",
    input: replyTo
      ? { text: texts[segment], in_reply_to: replyTo }
      : { text: texts[segment] },
    self: false,
    acts: [batchAct(act, index, null)],
    segment: { index: segment, count: texts.length },
  };
}

function batchAct(act, index, target) {
  return {
    act_id: String(act.act_id),
    act_index: index,
    kind: act.kind,
    consequence: ACT_KINDS[act.kind].consequence,
    target,
  };
}

// The operation call for a batch: its input, with the account's own id for
// the kinds that act from it, and what the person approving it is shown. The
// account is the verified one the call runs as.
export function batchCall(batch, account, plan) {
  const input = batch.self
    ? { source_user_id: account.id, ...batch.input }
    : batch.input;
  const count = batch.acts.length;
  const first = batch.acts[0]?.act_index ?? 0;
  const span = count === 1 ? `act ${first}` : `acts ${first} to ${first + count - 1}`;
  const what = batch.segment
    ? `${VERBS.thread} (${batch.segment.index + 1} of ${batch.segment.count})`
    : ACT_KINDS[batch.kind].publish
      ? VERBS[batch.kind]
      : `${VERBS[batch.kind]} ${count} ${batch.kind === "delete_post" || batch.kind === "like" || batch.kind === "repost" ? "post" : "account"}${count === 1 ? "" : "s"}`;
  return {
    operation: batch.operation,
    input,
    approval: {
      reason: `${what} on X as @${account.username} (account ${account.id}): ${span} of ${plan.total_act_count} in plan ${text(plan.plan_digest)}.`,
    },
  };
}

// Whether a repeated act's acknowledgement reports the state it asked for:
// true, false, or null when the answer does not say.
export function reachedState(kind, acknowledgement) {
  const state = ACT_KINDS[kind]?.state;
  if (!state) return null;
  const data = record(record(acknowledgement).data);
  const [field, expected] = state;
  return typeof data[field] === "boolean" ? data[field] === expected : null;
}
