import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const PACKAGE_NAME = "postmortem-maker";
export const PACKAGE_VERSION = "0.1.0";

const PACKAGE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const MAX_STDIN_BYTES = 2 * 1024 * 1024;
const MAX_EVENT_TEXT = 12_000;

export function packageRoot() {
  return PACKAGE_ROOT;
}

export async function readEnvelope() {
  const chunks = [];
  let size = 0;
  for await (const chunk of process.stdin) {
    size += Buffer.byteLength(chunk);
    if (size > MAX_STDIN_BYTES) {
      throw new Error("input exceeds the 2 MiB execution boundary");
    }
    chunks.push(chunk);
  }
  let raw = Buffer.concat(chunks).toString("utf8").trim();
  if (!raw) {
    const envJson = process.env.RUNX_INPUTS_JSON;
    if (typeof envJson === "string" && envJson.trim()) {
      raw = envJson.trim();
    } else {
      const envPath = process.env.RUNX_INPUTS_PATH;
      if (typeof envPath === "string" && envPath.trim()) {
        try {
          raw = fs.readFileSync(envPath, "utf8").trim();
        } catch {
          throw new Error("runtime input file could not be read");
        }
      }
    }
  }
  if (!raw) return {};
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("runner input must be one JSON object");
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("runner input must be a JSON object");
  }
  return parsed;
}

export function field(envelope, name) {
  if (envelope?.inputs && Object.prototype.hasOwnProperty.call(envelope.inputs, name)) {
    return envelope.inputs[name];
  }
  if (envelope?.context && Object.prototype.hasOwnProperty.call(envelope.context, name)) {
    return envelope.context[name];
  }
  if (Object.prototype.hasOwnProperty.call(envelope ?? {}, name)) {
    return envelope[name];
  }
  return undefined;
}

export async function runCli(handler) {
  try {
    const envelope = await readEnvelope();
    const result = await handler(envelope);
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } catch (error) {
    const message = error instanceof Error ? error.message : "step failed";
    process.stderr.write(`${JSON.stringify({ error: { code: "step_failed", message } })}\n`);
    process.exitCode = 1;
  }
}

export function requireObject(value, name) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${name} must be a JSON object`);
  }
  return value;
}

export function requireString(value, name, { maxLength = 2_000 } = {}) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${name} must be a non-empty string`);
  }
  if (value.length > maxLength) {
    throw new Error(`${name} exceeds its ${maxLength} character limit`);
  }
  return value.trim();
}

export function clampText(value, maxLength = MAX_EVENT_TEXT) {
  const text = typeof value === "string" ? value : "";
  if (text.length <= maxLength) return text;
  return `${text.slice(0, maxLength)}\n[truncated]`;
}

function canonicalize(value) {
  if (value === null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map(canonicalize);
  return Object.keys(value)
    .sort()
    .reduce((result, key) => {
      result[key] = canonicalize(value[key]);
      return result;
    }, {});
}

export function canonicalJson(value) {
  return JSON.stringify(canonicalize(value));
}

export function sha256(value) {
  const bytes = typeof value === "string" ? value : canonicalJson(value);
  return `sha256:${crypto.createHash("sha256").update(bytes, "utf8").digest("hex")}`;
}

export function nowIso() {
  return new Date().toISOString();
}

export function eventCitation(event, quote = event.text) {
  return {
    event_id: event.id,
    author: event.author,
    at: event.at,
    url: event.url,
    quote,
  };
}

export function normalizeEvents(thread, sourceRef) {
  requireObject(thread, "incident thread");
  const events = thread.events;
  if (!Array.isArray(events)) throw new Error("incident thread events must be an array");
  if (events.length > 200) throw new Error("incident thread is limited to 200 events");

  const seen = new Set();
  const normalized = events.map((event, index) => {
    requireObject(event, `incident thread event ${index}`);
    const id = requireString(event.id, `incident thread event ${index}.id`, { maxLength: 200 });
    if (seen.has(id)) throw new Error(`incident thread contains duplicate event id: ${id}`);
    seen.add(id);
    if (typeof event.text !== "string" || event.text.trim() === "") {
      throw new Error(`incident thread event ${index}.text must be a non-empty string`);
    }
    const text = clampText(event.text);
    const at = typeof event.at === "string" && event.at.trim() ? event.at.trim() : null;
    const author = typeof event.author === "string" && event.author.trim() ? event.author.trim() : "unknown";
    const url = typeof event.url === "string" && event.url.trim() ? event.url.trim() : `${sourceRef}#${encodeURIComponent(id)}`;
    return { id, at, author, url, text };
  });

  normalized.sort((left, right) => {
    const leftTime = left.at ? Date.parse(left.at) : Number.NaN;
    const rightTime = right.at ? Date.parse(right.at) : Number.NaN;
    if (Number.isFinite(leftTime) && Number.isFinite(rightTime) && leftTime !== rightTime) {
      return leftTime - rightTime;
    }
    return events.findIndex((event) => event.id === left.id) - events.findIndex((event) => event.id === right.id);
  });

  return normalized;
}

export function stateRoot() {
  const configured = process.env.RUNX_STATE_DIR;
  if (configured && path.isAbsolute(configured)) {
    return path.join(configured, PACKAGE_NAME);
  }
  return path.join(PACKAGE_ROOT, ".runx", PACKAGE_NAME);
}

export function targetRef(target) {
  if (!target || typeof target !== "object" || Array.isArray(target)) return "local://runx-postmortem-maker/default";
  return typeof target.data_source_ref === "string" && target.data_source_ref.trim()
    ? target.data_source_ref.trim()
    : "local://runx-postmortem-maker/default";
}

export function outboxFile(target) {
  const key = crypto.createHash("sha256").update(targetRef(target), "utf8").digest("hex");
  return path.join(stateRoot(), `${key}.json`);
}

function emptyOutbox(target) {
  return {
    schema: "runx.postmortem-maker.outbox.v1",
    store_ref: targetRef(target),
    version: 0,
    messages: [],
  };
}

export function readOutboxState(target) {
  const file = outboxFile(target);
  if (!fs.existsSync(file)) return emptyOutbox(target);
  let parsed;
  try {
    parsed = JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    throw new Error("publication outbox is not valid JSON");
  }
  if (!parsed || parsed.schema !== "runx.postmortem-maker.outbox.v1" || !Number.isInteger(parsed.version) || parsed.version < 0 || !Array.isArray(parsed.messages)) {
    throw new Error("publication outbox has an invalid shape");
  }
  return parsed;
}

export function writeOutboxState(target, state) {
  const file = outboxFile(target);
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  const temporary = `${file}.${process.pid}.tmp`;
  fs.writeFileSync(temporary, `${JSON.stringify(state)}\n`, { encoding: "utf8", mode: 0o600 });
  fs.renameSync(temporary, file);
}

export function publicOutboxSummary(state) {
  return {
    schema: state.schema,
    store_ref: state.store_ref,
    version: state.version,
    messages: state.messages.map((message) => ({
      message_id: message.message_id,
      idempotency_key: message.idempotency_key,
      content_digest: message.content_digest,
      postmortem_digest: message.postmortem_digest,
      aggregate_ref: message.aggregate_ref,
    })),
  };
}

export function isMainModule(metaUrl) {
  return process.argv[1] && path.resolve(process.argv[1]) === path.resolve(fileURLToPath(metaUrl));
}
