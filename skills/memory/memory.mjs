// runx/memory: a person's words and an agent's observations, kept on
// `private` by level (memory.md). A worker with no I/O: every read and write
// is a step of run.yaml, or a call the index and search scripts make through
// ctx, so a run's record holds everything memory did.

const SCHEMA = "runx.context_entry.v1";
const SKILL = "runx/memory";
/// The most keys one page of a listing or a search asks a stream for.
const MAX_PAGE_KEYS = 100;
/// The fewest characters a bare line, `<handle> <date>`, holds, so a page
/// asks a stream for no more keys than the room left could show.
const SHORTEST_LINE = 96;
const NAME = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;
/// The purposes that make the person's words a standing rule, which every
/// index shows whatever its age.
const STANDING = new Set(["mandate", "procedure"]);
const KINDS = {
  said: {
    types: ["memory.remembered", "memory.corrected"],
    trust: "person_confirmed",
  },
  observed: {
    types: ["memory.observed"],
    trust: "untrusted_evidence",
  },
};

// SHA-256 over a string's UTF-8 bytes, as hex: a worker has no crypto.
const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

function utf8(text) {
  const bytes = [];
  for (const character of text) {
    let point = character.codePointAt(0);
    if (point < 0x80) {
      bytes.push(point);
    } else if (point < 0x800) {
      bytes.push(0xc0 | (point >> 6), 0x80 | (point & 0x3f));
    } else if (point < 0x10000) {
      bytes.push(0xe0 | (point >> 12), 0x80 | ((point >> 6) & 0x3f), 0x80 | (point & 0x3f));
    } else {
      bytes.push(
        0xf0 | (point >> 18),
        0x80 | ((point >> 12) & 0x3f),
        0x80 | ((point >> 6) & 0x3f),
        0x80 | (point & 0x3f),
      );
    }
  }
  return bytes;
}

function sha256(text) {
  const bytes = utf8(text);
  const bits = bytes.length * 8;
  bytes.push(0x80);
  while (bytes.length % 64 !== 56) bytes.push(0);
  for (let shift = 56; shift >= 0; shift -= 8) {
    bytes.push(shift >= 32 ? Math.floor(bits / 2 ** shift) & 0xff : (bits >>> shift) & 0xff);
  }
  const hash = new Uint32Array([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ]);
  const words = new Uint32Array(64);
  const rotate = (value, by) => (value >>> by) | (value << (32 - by));
  for (let chunk = 0; chunk < bytes.length; chunk += 64) {
    for (let index = 0; index < 16; index += 1) {
      const at = chunk + index * 4;
      words[index] = (bytes[at] << 24) | (bytes[at + 1] << 16) | (bytes[at + 2] << 8) | bytes[at + 3];
    }
    for (let index = 16; index < 64; index += 1) {
      const low = words[index - 15];
      const high = words[index - 2];
      const s0 = rotate(low, 7) ^ rotate(low, 18) ^ (low >>> 3);
      const s1 = rotate(high, 17) ^ rotate(high, 19) ^ (high >>> 10);
      words[index] = (words[index - 16] + s0 + words[index - 7] + s1) >>> 0;
    }
    let [a, b, c, d, e, f, g, h] = hash;
    for (let index = 0; index < 64; index += 1) {
      const s1 = rotate(e, 6) ^ rotate(e, 11) ^ rotate(e, 25);
      const choose = (e & f) ^ (~e & g);
      const first = (h + s1 + choose + K[index] + words[index]) >>> 0;
      const s0 = rotate(a, 2) ^ rotate(a, 13) ^ rotate(a, 22);
      const majority = (a & b) ^ (a & c) ^ (b & c);
      const second = (s0 + majority) >>> 0;
      h = g;
      g = f;
      f = e;
      e = (d + first) >>> 0;
      d = c;
      c = b;
      b = a;
      a = (first + second) >>> 0;
    }
    const next = [a, b, c, d, e, f, g, h];
    for (let index = 0; index < 8; index += 1) hash[index] = (hash[index] + next[index]) >>> 0;
  }
  return Array.from(hash, (word) => word.toString(16).padStart(8, "0")).join("");
}

/// The stream a level names for one kind, as memory.md's table names it.
/// A level that needs a workspace, a project or a brand it wasn't given is
/// a visible missing input, never a default.
function streamOf(kind, { level, project, brand, workspace_id: workspace }) {
  const named = (value, what) => {
    if (typeof value !== "string" || !NAME.test(value)) {
      throw new Error(`level ${level} needs ${what}, and none was given`);
    }
    return value;
  };
  switch (level) {
    case "personal":
      return `${kind}:personal`;
    case "workspace":
      return `${kind}:workspace:${named(workspace, "a workspace: this run has no workspace id")}`;
    case "project":
      return `${kind}:project:${named(workspace, "a workspace: this run has no workspace id")}:${named(project, "a project")}`;
    case "brand":
      return `${kind}:brand:${named(workspace, "a workspace: this run has no workspace id")}:${named(brand, "a brand")}`;
    default:
      throw new Error(`level ${level} is none of personal, workspace, project or brand`);
  }
}

/// A memory stream's kind, or nothing when the stream is none of memory's.
function kindOf(stream) {
  const [kind, level, ...rest] = stream.split(":");
  if (!(kind in KINDS)) return undefined;
  const shaped =
    (level === "personal" && rest.length === 0) ||
    (level === "workspace" && rest.length === 1) ||
    ((level === "project" || level === "brand") && rest.length === 2);
  return shaped && rest.every((part) => part.length > 0) ? kind : undefined;
}

function handleOf(stream, key, version) {
  return `memory:${stream}#${key}@${version}`;
}

/// `memory:<stream>#<key>@<version>`, the version the digits after the last
/// `@`, refused unless it names one of memory's streams, of `kind` when one
/// is given.
function parsed(handle, kind) {
  const match = /^memory:([^#]+)#([^#]+)@([0-9]+)$/u.exec(handle ?? "");
  if (!match) throw new Error(`${handle} is no memory handle: memory:<stream>#<key>@<version>`);
  const [, stream, key, digits] = match;
  const version = Number(digits);
  const found = kindOf(stream);
  if (!found) throw new Error(`${handle} names ${stream}, which is none of memory's streams`);
  if (kind && found !== kind) throw new Error(`${handle} isn't a ${kind} entry`);
  if (!Number.isSafeInteger(version) || version < 1) {
    throw new Error(`${handle} names version ${digits}, and an entry's versions start at 1`);
  }
  return { stream, key, version, kind: found };
}

/// The entry a new `said` or `observed` record names: its stream, from its
/// level, and its key, the digest of its kind, level and content, so the same
/// words name one entry. An observation that supersedes another is that
/// entry's next version.
export function newEntry({ kind, level, project, brand, workspace_id, text, date, sources, supersedes }) {
  const stream = streamOf(kind, { level, project, brand, workspace_id });
  if (supersedes !== undefined && supersedes !== null) {
    const replaced = parsed(supersedes, "observed");
    if (replaced.stream !== stream) {
      throw new Error(`${supersedes} is on ${replaced.stream}, not on ${stream}, where this observation is kept`);
    }
    return { stream, key: replaced.key, version: replaced.version };
  }
  const key = sha256(JSON.stringify(["runx/memory.entry.v1", kind, stream, text, date, sources]));
  return { stream, key, version: 0 };
}

/// The entry a handle names, at its version, and the version a read of that
/// version reads after.
export function parseHandle({ handle, kind }) {
  const { stream, key, version } = parsed(handle, kind);
  return { stream, key, version, after_version: version - 1 };
}

/// What a purpose makes the person's words, as a gate says it.
const PURPOSES = {
  mandate: "a mandate, a rule every index shows",
  procedure: "a procedure, a rule every index shows",
  confirmed_memory: "something they told you",
};

/// When the content was true, and where it came from, as a gate says it.
function dated({ date, valid_until, sources }) {
  return `${date}${valid_until ? `, until ${valid_until}` : ""}, from ${sources.join(", ")}`;
}

const DESCRIBED = {
  "memory.remembered": ({ stream, data }) =>
    `Keep these exact words on ${stream}, as ${PURPOSES[data.purpose]}: "${data.text}" (${dated(data)}). The index shows them as: ${data.summary}`,
  "memory.corrected": ({ handle, data }) =>
    `Replace ${handle} with these exact words, as ${PURPOSES[data.purpose]}: "${data.text}" (${dated(data)}). The index shows them as: ${data.summary}`,
  "memory.disabled": ({ handle, data }) => `Stop ${handle} applying, because: ${data.reason}`,
  "memory.observed": ({ stream, handle, data }) =>
    `${handle ? `Supersede ${handle} with` : `Keep on ${stream}`} this observation: "${data.text}" (${dated(data)}). The index shows it as: ${data.summary}`,
};

/// Decide one write: refused unless the entry is where the write expects it,
/// and named by what a gate shows and the idempotency key the append carries.
/// Both cover the event's type, the version it follows and its content, its
/// end date and whether it stands as a rule among it, never the confirmation
/// the gate has yet to give. The person's words stand as a rule when their
/// purpose is a mandate or a procedure, which the append says as `standing`.
export function change({ entry, current, type, data, valid_until }) {
  const describe = DESCRIBED[type];
  if (!describe) throw new Error(`${type} is no memory event`);
  const at = current?.version ?? 0;
  const last = current?.last_event?.type;
  const handle = entry.version > 0 ? handleOf(entry.stream, entry.key, entry.version) : undefined;
  if (entry.version === 0 && at !== 0) {
    throw new Error(`these exact words are already kept, as ${handleOf(entry.stream, entry.key, at)}`);
  }
  if (entry.version > 0 && at !== entry.version) {
    throw new Error(
      at === 0
        ? `${handle} names no entry that's kept`
        : `${handle} isn't current: the entry is at version ${at}, as ${handleOf(entry.stream, entry.key, at)}`,
    );
  }
  if (last === "memory.disabled") {
    throw new Error(`${handle} is disabled, and a disabled entry doesn't change`);
  }
  if (type === "memory.observed" && entry.version > 0 && last !== "memory.observed") {
    throw new Error(`${handle} isn't an observation, so an observation can't supersede it`);
  }
  if (valid_until !== undefined && data.date !== undefined && valid_until < data.date) {
    throw new Error(`it ends on ${valid_until}, before its own date, ${data.date}`);
  }
  const standing = STANDING.has(data.purpose);
  const content = Object.fromEntries(Object.entries(data).filter(([field]) => field !== "confirmation"));
  if (valid_until !== undefined) content.valid_until = valid_until;
  if (standing) content.standing = true;
  return {
    reason: describe({ stream: entry.stream, handle, data: content }),
    idempotency_key: `memory:${sha256(JSON.stringify([type, entry.stream, entry.key, at, content]))}`,
    // Only the person's words have a purpose, which decides it.
    ...(data.purpose === undefined ? {} : { standing }),
  };
}

/// The handle of the version an append made, once its readback holds it.
export function entryHandle({ entry, appended, readback }) {
  const version = appended?.version;
  if (!Number.isSafeInteger(version) || readback?.version !== version) {
    throw new Error(`the append to ${entry.stream}#${entry.key} wasn't read back at its version`);
  }
  return { handle: handleOf(entry.stream, entry.key, version) };
}

function length(text) {
  return [...text].length;
}

/// A store event as the context entry a turn reads: a line for its index,
/// the entry whole as its data, and the labels its kind gives it. The
/// person's words read with their purpose, and a disabled version of them,
/// which holds none, as what the person told you.
function entryOf({ stream, key, event, method, summary }) {
  const kind = kindOf(stream);
  const handle = handleOf(stream, key, event.version);
  const text = event.data?.text ?? "";
  const data = {
    handle,
    kind,
    version: event.version,
    type: event.type,
    date: event.data?.date ?? null,
    text,
    summary: event.data?.summary ?? null,
    sources: event.data?.sources ?? [],
  };
  const purpose =
    kind === "observed"
      ? "observation"
      : event.type === "memory.disabled"
        ? "confirmed_memory"
        : event.data?.purpose;
  if (kind === "said") data.purpose = purpose;
  if (event.valid_until) data.valid_until = event.valid_until;
  if (event.type === "memory.disabled") data.reason = event.data?.reason ?? null;
  return {
    schema: SCHEMA,
    type: `memory.${kind}`,
    summary: summary ?? line({ handle, date: data.date, summary: data.summary }),
    data,
    labels: { purpose, trust: KINDS[kind].trust, sharing: "configured_model" },
    sources: data.sources.length > 0 ? data.sources : [handle],
    origin: {
      artifact_id: handle,
      step_id: null,
      producer: { skill: SKILL, method },
      created_at: event.committed_at,
      digest: `sha256:${sha256(text)}`,
      size_bytes: utf8(text).length,
    },
  };
}

/// An index line: `<handle> <date>`, and its summary after when it has one.
function line({ handle, date, summary }) {
  const head = `${handle} ${date ?? "undated"}`;
  return summary ? `${head} ${summary}` : head;
}

/// The entry a read of one version returned, whether or not it's current.
export function contextEntry({ handle, read }) {
  const { stream, key, version } = parsed(handle);
  const event = (read?.events ?? []).find((candidate) => candidate.version === version);
  if (!event) throw new Error(`${handle} names no version that's kept`);
  return { entry: entryOf({ stream, key, event, method: "read" }) };
}

/// The streams `levels` names, the person's words before observations, each
/// once, passing over a level that needs a workspace this run doesn't have.
function streamsOf(levels, workspace_id) {
  const streams = { said: [], observed: [] };
  for (const kind of Object.keys(KINDS)) {
    for (const level of levels) {
      if (level.level !== "personal" && (workspace_id === null || workspace_id === undefined)) continue;
      const stream = streamOf(kind, { ...level, workspace_id });
      if (!streams[kind].includes(stream)) streams[kind].push(stream);
    }
  }
  return streams;
}

/// A call's result, as the operation step it was recorded as holds it.
function resultOf(value) {
  const packet = value?.provider_operation;
  return packet?.result ?? packet?.data?.result ?? packet;
}

/// Whether a call was refused admission: a stream the run's authority
/// doesn't reach is passed over, never an error.
function refused(error) {
  return error?.code === "refused";
}

/// How many keys a page asks a stream for, with `room` characters left.
function pageKeysFor(room) {
  return Math.max(1, Math.min(MAX_PAGE_KEYS, Math.ceil(room / SHORTEST_LINE)));
}

/// One stream's keys, newest commit first, a page at a time, as `query`
/// selects them, each page `pageKeys()` keys. A stream the run's authority
/// doesn't reach reads as empty.
class StreamPages {
  constructor(ctx, stream, query, pageKeys) {
    Object.assign(this, { ctx, stream, query, pageKeys, keys: [], cursor: undefined, done: false });
  }

  async head() {
    if (this.keys.length === 0 && !this.done) {
      try {
        const page = resultOf(
          await this.ctx.operation("private/keys.list", {
            stream: this.stream,
            ...this.query,
            limit: this.pageKeys(),
            ...(this.cursor ? { cursor: this.cursor } : {}),
          }),
        );
        this.keys = page?.keys ?? [];
        this.cursor = page?.next_cursor ?? undefined;
        this.done = !this.cursor;
      } catch (error) {
        if (!refused(error)) throw error;
        this.keys = [];
        this.done = true;
      }
    }
    return this.keys[0];
  }

  take() {
    return this.keys.shift();
  }
}

/// The newer of two keys, as each stream lists them: newest commit first,
/// ties by key, then by the stream the levels name first.
function newer(left, right) {
  const at = (item) => item.last_event.committed_at;
  if (at(left) !== at(right)) return at(left) > at(right);
  return left.key < right.key;
}

/// Every key of `pages` newest first, merged across their streams, each
/// handed to `take` until it says stop.
async function walk(pages, take) {
  for (;;) {
    let from;
    for (const candidate of pages) {
      const head = await candidate.head();
      if (head && (!from || newer(head, await from.head()))) from = candidate;
    }
    if (!from) return;
    const head = from.take();
    if (!(await take(from.stream, head))) return;
  }
}

/// The date `ctx.now()` reads, `YYYY-MM-DD`, in UTC.
function today(ctx) {
  return new Date(ctx.now()).toISOString().slice(0, 10);
}

/// A bounded index for a turn. The person's standing rules first, every one,
/// oldest first, read from the standing keys apart, so the facts' walk never
/// visits one. Then facts, the person's other words before observations: with
/// a focus, those its search matches first, best first and newest first among
/// equals, and the rest newest first; without one, newest first. Each line is
/// `<handle> <date> <summary>`. An entry whose end date has passed by the
/// run's clock is passed over by the store, never read and dropped here.
///
/// The index fits its budget with every line bare first: every rule's line,
/// and a fact's line while it fits, read page by page and no further than
/// the last line that could fit. Then summaries go back from the top, while
/// they fit, so they're lost from the bottom up: the lowest-ranked fact's
/// first, a rule's last, the newest rule's before the oldest, which keeps the
/// request's prefix. A rule's line never goes, and when the rules alone pass
/// the budget the index says so. It says when it cut entries, which search
/// and read reach, and counts nothing.
export async function index(ctx, { levels, max_characters = 4096, focus, workspace_id }) {
  const streams = streamsOf(levels, workspace_id);
  const valid_at = today(ctx);
  const budget = max_characters;
  let used = 0;
  const kept = [];
  const add = (stream, key, event) => {
    const bare = line({ handle: handleOf(stream, key, event.version), date: event.data?.date });
    used += length(bare) + (kept.length > 0 ? 1 : 0);
    kept.push({ stream, key, event, bare });
  };
  const rules = [];
  for (const [order, stream] of streams.said.entries()) {
    const pages = new StreamPages(
      ctx,
      stream,
      { types: KINDS.said.types, standing: true, valid_at },
      () => MAX_PAGE_KEYS,
    );
    await walk([pages], (_, listed) => {
      rules.push({ stream, order, key: listed.key, event: listed.last_event });
      return true;
    });
  }
  rules.sort(
    (left, right) =>
      (left.event.committed_at < right.event.committed_at ? -1 : left.event.committed_at > right.event.committed_at ? 1 : 0) ||
      (left.key < right.key ? -1 : left.key > right.key ? 1 : 0) ||
      left.order - right.order,
  );
  for (const rule of rules) add(rule.stream, rule.key, rule.event);
  const over_budget = used > budget;
  let omitted = false;
  const shown = new Set();
  const room = () => budget - used - (kept.length > 0 ? 1 : 0);
  // A fact's line while it fits; the first that doesn't ends the facts.
  const fact = (stream, key, event) => {
    const bare = line({ handle: handleOf(stream, key, event.version), date: event.data?.date });
    if (length(bare) > room()) {
      omitted = true;
      return false;
    }
    add(stream, key, event);
    shown.add(`${stream}#${key}`);
    return true;
  };
  for (const kind of Object.keys(KINDS)) {
    const query = { types: KINDS[kind].types, standing: false, valid_at };
    if (focus) {
      const matched = [];
      for (const [order, stream] of streams[kind].entries()) {
        try {
          const page = resultOf(
            await ctx.operation("private/keys.search", { stream, query: focus, ...query, limit: pageKeysFor(room()) }),
          );
          for (const match of page?.matches ?? []) matched.push({ stream, order, ...match });
        } catch (error) {
          if (!refused(error)) throw error;
        }
      }
      matched.sort(
        (left, right) =>
          right.score - left.score ||
          (left.last_event.committed_at > right.last_event.committed_at ? -1 : left.last_event.committed_at < right.last_event.committed_at ? 1 : 0) ||
          (left.key < right.key ? -1 : left.key > right.key ? 1 : 0) ||
          left.order - right.order,
      );
      for (const match of matched) {
        if (!fact(match.stream, match.key, match.last_event)) break;
      }
    }
    if (omitted) break;
    const pages = streams[kind].map((stream) => new StreamPages(ctx, stream, query, () => pageKeysFor(room())));
    await walk(pages, (stream, listed) =>
      shown.has(`${stream}#${listed.key}`) ? true : fact(stream, listed.key, listed.last_event),
    );
    if (omitted) break;
  }
  // Summaries back from the top while they fit; the first that doesn't ends
  // them, so a line loses its summary only after every line below it has.
  let summaries = true;
  const entries = kept.map(({ stream, key, event, bare }) => {
    let summary = bare;
    if (summaries && event.data?.summary) {
      const full = `${bare} ${event.data.summary}`;
      const extra = length(full) - length(bare);
      if (used + extra <= budget) {
        used += extra;
        summary = full;
      } else {
        summaries = false;
      }
    }
    return entryOf({ stream, key, event, method: "index", summary });
  });
  return { entries, entries_omitted: omitted, over_budget };
}

/// Ranked matches among the current entries of the streams the run may
/// read: one search of each stream, merged by score, a person's words before
/// an observation, then newest first, then by key.
export async function search(ctx, { levels, query, limit = 10, workspace_id }) {
  const streams = streamsOf(levels, workspace_id);
  const found = [];
  for (const kind of Object.keys(KINDS)) {
    for (const stream of streams[kind]) {
      try {
        const result = resultOf(
          await ctx.operation("private/keys.search", {
            stream,
            query,
            types: KINDS[kind].types,
            limit,
          }),
        );
        for (const match of result?.matches ?? []) found.push({ kind, stream, ...match });
      } catch (error) {
        if (!refused(error)) throw error;
      }
    }
  }
  const rank = { said: 0, observed: 1 };
  found.sort(
    (left, right) =>
      right.score - left.score ||
      rank[left.kind] - rank[right.kind] ||
      (left.last_event.committed_at > right.last_event.committed_at
        ? -1
        : left.last_event.committed_at < right.last_event.committed_at
          ? 1
          : 0) ||
      (left.key < right.key ? -1 : left.key > right.key ? 1 : 0) ||
      (left.stream < right.stream ? -1 : left.stream > right.stream ? 1 : 0),
  );
  return {
    entries: found
      .slice(0, limit)
      .map(({ stream, key, last_event }) => entryOf({ stream, key, event: last_event, method: "search" })),
  };
}
