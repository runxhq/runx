import {
  MAX_ENGAGEMENT_ACTS,
  MAX_THREAD_SEGMENTS,
  actKind,
  batchCall,
  executionActLimit,
  selectBatch,
} from "./twitter-execution-batch.mjs";
import {
  activeThread,
  array,
  nonNegativeInteger,
  record,
  text,
} from "./twitter-execution-values.mjs";

export function prepareExecution(inputs) {
  const plan = record(inputs.plan_json);
  const digest = text(record(inputs.digest_result).digest);
  const acts = array(plan.acts);
  const blockers = validatePlan(plan, acts, digest, text(inputs.plan_digest));
  const progress = readProgress(record(inputs.execution_ledger), digest, acts, blockers);
  const expectedVersion = nonNegativeInteger(inputs.expected_version);
  const idempotencyKey = expectedVersion === null
    ? ""
    : batchIdempotencyKey(digest, expectedVersion);
  validateBatchBinding(
    inputs,
    expectedVersion,
    progress.version,
    idempotencyKey,
    blockers,
  );
  const base = executionPlanBase(
    plan,
    acts.length,
    digest,
    progress,
    expectedVersion,
    idempotencyKey,
  );
  if (blockers.length > 0) {
    return { twitter_execution_plan: { ...base, decision: "refused", blockers } };
  }
  if (progress.nextActIndex === acts.length) {
    return { twitter_execution_plan: { ...base, decision: "noop" } };
  }
  return {
    twitter_execution_plan: {
      ...base,
      decision: "ready",
      batch: selectBatch(acts, progress, executionActLimit(inputs.max_acts)),
    },
  };
}

// The batch's operation call, from the plan and the account it runs as. The
// bound account must be the plan's principal and, once the ledger has pinned
// one, the account every earlier batch ran as; otherwise nothing is sent.
export function bindExecution(inputs) {
  const plan = record(inputs.execution_plan);
  const account = record(inputs.account);
  const id = text(account.id);
  const username = text(account.username);
  const handle = principalHandle(plan.principal);
  if (!X_ID.test(id) || !username) {
    throw new Error("the account snapshot did not name the bound account");
  }
  if (!handle || handle.toLowerCase() !== username.toLowerCase()) {
    throw new Error(
      `the bound X account @${username} is not the plan's principal ${text(plan.principal)}`,
    );
  }
  const pinned = text(plan.account_id);
  if (pinned && pinned !== id) {
    throw new Error(
      `the bound X account ${id} is not account ${pinned}, which earlier batches of this plan ran as`,
    );
  }
  return {
    twitter_call: {
      ...batchCall(record(plan.batch), { id, username }, plan),
      account_id: id,
    },
  };
}

// The handle a plan's principal names: `account:@handle`.
function principalHandle(principal) {
  const match = /^account:@([A-Za-z0-9_]{1,15})$/.exec(text(principal));
  return match ? match[1] : null;
}

function validateBatchBinding(
  inputs,
  expectedVersion,
  durableVersion,
  canonicalIdempotencyKey,
  blockers,
) {
  if (expectedVersion === null) {
    blockers.push("expected_version must be a non-negative safe integer");
  } else if (expectedVersion !== durableVersion) {
    blockers.push(
      `expected_version ${expectedVersion} does not match durable version ${durableVersion}`,
    );
  }
  if (text(inputs.idempotency_key) !== canonicalIdempotencyKey) {
    blockers.push(
      `idempotency_key must equal ${canonicalIdempotencyKey || "the canonical batch key"}`,
    );
  }
}

function executionPlanBase(
  plan,
  totalActCount,
  digest,
  progress,
  expectedVersion,
  idempotencyKey,
) {
  return {
    decision: "refused",
    plan_digest: digest,
    principal: plan.principal ?? null,
    account_id: progress.accountId,
    expected_version: expectedVersion,
    idempotency_key: idempotencyKey,
    start_act_index: progress.nextActIndex,
    next_act_index: progress.nextActIndex,
    total_act_count: totalActCount,
    remaining_count: Math.max(0, totalActCount - progress.nextActIndex),
    active_thread: progress.activeThread,
    batch: null,
    blockers: [],
  };
}

function readProgress(ledger, planDigest, acts, blockers) {
  const version = nonNegativeInteger(ledger.after_version);
  const events = array(ledger.events);
  const empty = { version: version ?? 0, nextActIndex: 0, activeThread: null, accountId: null };
  if (version === null) {
    blockers.push("execution ledger omitted a valid after_version");
    return empty;
  }
  if (version === 0) {
    if (events.length !== 0) blockers.push("an empty execution ledger returned events");
    return empty;
  }
  if (events.length !== 1) {
    blockers.push("execution ledger tail must contain exactly one durable progress event");
    return empty;
  }
  const latest = record(events[0]);
  const event = record(latest.event);
  const nextActIndex = nonNegativeInteger(event.next_act_index);
  const totalActCount = nonNegativeInteger(event.total_act_count);
  const currentThread = activeThread(event.active_thread);
  // Events from before the account was pinned carry none.
  const accountId = event.account_id === undefined || event.account_id === null
    ? null
    : text(event.account_id);
  if (accountId !== null && !X_ID.test(accountId)) {
    blockers.push("execution ledger progress names an account id that is not an X id");
  }
  if (nonNegativeInteger(latest.version) !== version) {
    blockers.push("execution ledger tail version does not match after_version");
  }
  if (event.schema !== "twitter.execution.progress.v1" || event.type !== "twitter.execution.progress") {
    blockers.push("execution ledger requires the compact twitter.execution.progress.v1 shape");
  }
  if (text(event.plan_digest) !== planDigest) {
    blockers.push("execution ledger progress is bound to a different plan digest");
  }
  if (totalActCount !== acts.length) {
    blockers.push("execution ledger progress has a different total act count");
  }
  if (nextActIndex === null || nextActIndex > acts.length) {
    blockers.push("execution ledger next_act_index is outside the plan");
  }
  validateActiveThread(currentThread, acts, nextActIndex, blockers);
  return {
    version,
    nextActIndex: nextActIndex ?? 0,
    activeThread: currentThread,
    accountId: accountId || null,
  };
}

function validateActiveThread(current, acts, nextActIndex, blockers) {
  if (!current) return;
  const act = acts[current.act_index];
  const segmentCount = array(record(act?.params).texts).length;
  if (
    current.act_index !== nextActIndex
    || act?.kind !== "thread"
    || current.next_segment_index <= 0
    || current.next_segment_index >= segmentCount
    || !text(current.in_reply_to)
  ) {
    blockers.push("execution ledger active_thread is inconsistent with the next plan act");
  }
}

function validatePlan(value, acts, canonicalDigest, suppliedDigest) {
  const errors = [];
  if (!suppliedDigest) {
    errors.push("plan_digest is required; refusing unbound content");
  } else if (suppliedDigest !== canonicalDigest) {
    errors.push(
      `plan_digest mismatch: expected ${suppliedDigest}, canonical digest is ${canonicalDigest}`,
    );
  }
  if (value.decision !== "ready") errors.push("plan_json.decision must be ready");
  if (!principalHandle(value.principal)) {
    errors.push("plan_json.principal must name the account as account:@handle");
  }
  if (record(value.gates).human_approval_required !== true) {
    errors.push("plan_json must require human approval");
  }
  if (acts.length === 0) errors.push("plan_json.acts must be a non-empty array");
  if (acts.filter((act) => actKind(act?.kind)?.engagement).length > MAX_ENGAGEMENT_ACTS) {
    errors.push(`plan exceeds the ${MAX_ENGAGEMENT_ACTS}-act engagement cap`);
  }
  const seen = new Set();
  for (const act of acts) validateAct(act, seen, errors);
  return errors;
}

function validateAct(act, seen, errors) {
  if (!act || typeof act !== "object" || !text(act.act_id) || !text(act.kind)) {
    errors.push("every act needs act_id and kind");
    return;
  }
  if (seen.has(String(act.act_id))) errors.push(`duplicate act_id ${act.act_id}`);
  seen.add(String(act.act_id));
  if (!actKind(act.kind)) {
    errors.push(`unknown act kind ${act.kind}`);
    return;
  }
  const params = record(act.params);
  const missing = requiredParams(act.kind).filter((field) => missingValue(params[field]));
  if (missing.length > 0) {
    errors.push(`act ${act.act_id} is missing params: ${missing.join(", ")}`);
  }
  const malformed = ID_PARAMS
    .filter((field) => params[field] !== undefined && params[field] !== null && params[field] !== "")
    .filter((field) => typeof params[field] !== "string" || !X_ID.test(params[field]));
  if (malformed.length > 0) {
    errors.push(`act ${act.act_id} has ids that are not X ids: ${malformed.join(", ")}`);
  }
  if (["post", "reply", "quote"].includes(act.kind) && params.text !== undefined && !postText(params.text)) {
    errors.push(`act ${act.act_id} text must be a non-empty string of at most ${POST_TEXT_LIMIT} characters`);
  }
  if (act.kind === "thread" && params.texts !== undefined) {
    const texts = params.texts;
    if (
      !Array.isArray(texts)
      || texts.length === 0
      || texts.length > MAX_THREAD_SEGMENTS
      || !texts.every(postText)
    ) {
      errors.push(
        `thread ${act.act_id} texts must be 1 to ${MAX_THREAD_SEGMENTS} non-empty strings of at most ${POST_TEXT_LIMIT} characters`,
      );
    }
  }
}

const ID_PARAMS = ["post_id", "target_user_id", "in_reply_to", "quote_of"];
const X_ID = /^[0-9]{1,19}$/;
// The connector's bound on one post's text, in characters.
const POST_TEXT_LIMIT = 4000;

// Text is published verbatim, so it must already be a string.
function postText(value) {
  return typeof value === "string" && value.trim() !== "" && [...value].length <= POST_TEXT_LIMIT;
}

function requiredParams(kind) {
  return {
    post: ["text"],
    reply: ["text", "in_reply_to"],
    quote: ["text", "quote_of"],
    thread: ["texts"],
    delete_post: ["post_id"],
    unfollow: ["target_user_id"],
    follow: ["target_user_id"],
    mute: ["target_user_id"],
    block: ["target_user_id"],
    like: ["post_id"],
    repost: ["post_id"],
  }[kind] ?? [];
}

function missingValue(value) {
  return value === undefined
    || value === null
    || value === ""
    || (Array.isArray(value) && value.length === 0);
}

function batchIdempotencyKey(planDigest, expectedVersion) {
  return planDigest ? `twitter:${planDigest}:v${expectedVersion + 1}` : "";
}
