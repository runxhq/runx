const QUERIES = new Set(["snapshot", "posts", "mentions", "search", "following", "followers"]);
const USER_QUERIES = new Set(["following", "followers"]);

export function prepareRead(inputs) {
  const query = text(inputs.query);
  const params = record(inputs.params);
  const maxItems = positiveInteger(inputs.max_items, 200, 10_000);
  const source = text(inputs.source) || "live";
  const packet = (decision, overrides = {}) => readPlan(query, params, maxItems, decision, overrides);

  if (!QUERIES.has(query)) {
    return {
      twitter_read_plan: packet("needs_input", {
        blockers: [`query must be one of: ${[...QUERIES].join(", ")}`],
      }),
    };
  }
  if (source === "archive") {
    if (!text(inputs.archive_file)) {
      return {
        twitter_read_plan: packet("needs_input", {
          source,
          blockers: ["archive_file is required for archive reads"],
        }),
      };
    }
    return {
      twitter_read_plan: packet("archive", { source: "archive", retrieved_via: "runtime_artifact" }),
    };
  }
  if (source !== "live") {
    return {
      twitter_read_plan: packet("needs_input", { blockers: ["source must be live or archive"] }),
    };
  }
  if (query === "search" && !text(params.q)) {
    return {
      twitter_read_plan: packet("needs_input", { blockers: ["params.q is required for search"] }),
    };
  }
  const [operation, input] = liveCall(query, params, maxItems);
  return {
    twitter_read_plan: packet("live", { connector_operation: operation, provider_input: input }),
  };
}

// The connector operation that answers a query: the authenticated account's
// own collection unless the query names another account.
function liveCall(query, params, maxItems) {
  if (query === "snapshot") return ["account.snapshot", {}];
  const page = {
    limit: maxItems,
    page_size: Math.max(10, Math.min(100, maxItems)),
    ...(text(params.pagination_token) ? { cursor: text(params.pagination_token) } : {}),
  };
  if (query === "search") return ["posts.search", { query: text(params.q), ...page }];
  const userId = text(params.user_id);
  return userId
    ? [`${query}.read`, { user_id: userId, ...page }]
    : [`account.${query}.read`, page];
}

export function normalizeRead(inputs) {
  const plan = record(inputs.read_plan);
  const operation = record(inputs.provider_operation);
  const result = record(operation.result);
  const kind = text(record(plan.query).kind);
  // Pages are read whole, so the items may run past max_items by part of a
  // page; every one is kept, and the next read starts where these stop.
  const items = array(USER_QUERIES.has(kind) ? result.users : result.posts)
    .map(record)
    .map(USER_QUERIES.has(kind) ? mapUser : mapPost);
  const next = text(result.next);
  return {
    twitter_read_result: readResult(plan, {
      account: result.account ? mapUser(record(result.account)) : null,
      items,
      truncated: Boolean(next),
      next_cursor: next || null,
      retrieved_via: text(operation.readback_uri) || "api.x.com",
    }),
  };
}

// A read that needs input before any provider call, as evidence with no
// items. The digest is of the empty item list.
export function blockedRead(inputs) {
  const plan = record(inputs.read_plan);
  return finalizeRead({
    read_result: readResult(plan, {
      decision: "needs_input",
      source: text(plan.source) || "live",
      blockers: array(plan.blockers),
    }),
    digest_result: { digest: EMPTY_ITEMS_DIGEST },
  });
}

// The canonical digest of [], what data.digest returns for no items.
const EMPTY_ITEMS_DIGEST = "sha256:4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945";

export function normalizeArchivePage(inputs) {
  const plan = record(inputs.read_plan);
  const page = record(inputs.runx_page);
  const prior = record(page.state);
  const items = array(prior.items);
  const blockers = array(prior.blockers).map(text).filter(Boolean);
  const maxItems = number(plan.max_items, 200);
  let scanned = number(prior.scanned, 0);
  let truncated = prior.truncated === true;
  const kind = text(record(plan.query).kind);

  if (plan.decision !== "archive") {
    blockers.push("archive page execution requires an archive read plan");
  }
  for (const encoded of array(page.records)) {
    let raw;
    try {
      raw = record(JSON.parse(String(encoded)));
    } catch {
      blockers.push("runtime-framed archive record could not be decoded");
      break;
    }
    scanned += 1;
    if (items.length >= maxItems) {
      truncated = true;
      break;
    }
    if (kind === "following" || kind === "followers") {
      const value = record(raw.following ?? raw.follower ?? raw);
      items.push({
        id: value.accountId ?? value.id,
        username: null,
        name: null,
        description: "",
        metrics: null,
      });
    } else {
      items.push(mapArchivePost(record(raw.tweet ?? raw)));
    }
  }

  const state = { items, blockers, scanned, truncated };
  const done = blockers.length > 0 || truncated || page.eof === true;
  const runx_page = { state, done };
  if (!done) return { runx_page };
  return {
    runx_page,
    twitter_read_result: readResult(plan, {
      decision: blockers.length > 0 ? "needs_input" : "ok",
      source: "archive",
      items,
      truncated,
      retrieved_via: text(page.artifact_uri),
      blockers,
    }),
  };
}

export function finalizeRead(inputs) {
  const value = record(inputs.read_result);
  const items = array(value.items);
  return {
    twitter_evidence: {
      decision: text(value.decision) || "ok",
      source: text(value.source) || "live",
      query: record(value.query),
      account: value.account ?? null,
      items,
      item_count: items.length,
      truncated: value.truncated === true,
      next_cursor: text(value.next_cursor) || null,
      provenance: {
        retrieved_via: text(value.retrieved_via),
        content_digest: text(record(inputs.digest_result).digest),
      },
      blockers: array(value.blockers),
    },
  };
}

function readPlan(query, params, maxItems, decision, overrides = {}) {
  return {
    decision,
    source: "live",
    query: { kind: query, params },
    max_items: maxItems,
    connector_operation: null,
    provider_input: null,
    retrieved_via: "api.x.com",
    blockers: [],
    ...overrides,
  };
}

function readResult(plan, overrides = {}) {
  return {
    decision: "ok",
    source: text(plan.source) || "live",
    query: record(plan.query),
    account: null,
    items: [],
    truncated: false,
    next_cursor: null,
    retrieved_via: text(plan.retrieved_via) || "api.x.com",
    blockers: [],
    ...overrides,
  };
}

function mapPost(raw) {
  const metrics = record(raw.public_metrics);
  return {
    id: raw.id,
    text: raw.text,
    created_at: raw.created_at ?? null,
    metrics: {
      likes: number(metrics.like_count, 0),
      reposts: number(metrics.retweet_count, 0),
      replies: number(metrics.reply_count, 0),
      quotes: number(metrics.quote_count, 0),
      impressions: metrics.impression_count ?? null,
    },
    has_link: array(record(raw.entities).urls).length > 0,
    in_reply_to: raw.in_reply_to_user_id ?? null,
  };
}

function mapArchivePost(tweet) {
  return {
    id: tweet.id_str ?? tweet.id,
    text: tweet.full_text ?? tweet.text ?? "",
    created_at: tweet.created_at ?? null,
    metrics: {
      likes: number(tweet.favorite_count, 0),
      reposts: number(tweet.retweet_count, 0),
      replies: null,
      quotes: null,
      impressions: null,
    },
    has_link: array(record(tweet.entities).urls).length > 0,
    in_reply_to: tweet.in_reply_to_status_id_str ?? null,
  };
}

function mapUser(raw) {
  const metrics = record(raw.public_metrics);
  return {
    id: raw.id,
    username: raw.username,
    name: raw.name,
    description: raw.description ?? "",
    metrics: {
      followers: number(metrics.followers_count, 0),
      following: number(metrics.following_count, 0),
      posts: number(metrics.tweet_count, 0),
    },
  };
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function array(value) {
  return Array.isArray(value) ? value : [];
}

function text(value) {
  return typeof value === "string" ? value.trim() : "";
}

function number(value, fallback) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function positiveInteger(value, fallback, cap) {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed > 0 ? Math.min(parsed, cap) : fallback;
}
