import fs from "node:fs";
import path from "node:path";

import {
  PACKAGE_NAME,
  PACKAGE_VERSION,
  clampText,
  field,
  isMainModule,
  normalizeEvents,
  nowIso,
  packageRoot,
  readEnvelope,
  requireObject,
  requireString,
  runCli,
  sha256,
} from "./common.mjs";

const GITHUB_API_HOST = "api.github.com";
const GITHUB_HOST = "github.com";
const MAX_RESPONSE_BYTES = 1_500_000;

function safeFixturePath(ref) {
  const relative = requireString(ref, "incident_source.ref", { maxLength: 300 });
  if (!relative.startsWith("fixtures/") || relative.includes("..") || path.isAbsolute(relative)) {
    throw new Error("fixture sources must stay under fixtures/ and cannot contain ..");
  }
  const root = path.resolve(packageRoot());
  const candidate = path.resolve(root, relative);
  if (candidate !== root && !candidate.startsWith(`${root}${path.sep}`)) {
    throw new Error("fixture source escaped the package boundary");
  }
  return candidate;
}

function parseJsonFile(file) {
  try {
    return JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    throw new Error("incident fixture is not valid JSON");
  }
}

function threadFromObject(value, ref, readMode) {
  const thread = requireObject(value, "incident thread");
  const title = typeof thread.title === "string" && thread.title.trim() ? thread.title.trim() : "Untitled incident";
  const threadUrl = typeof thread.url === "string" && thread.url.trim() ? thread.url.trim() : ref;
  const events = normalizeEvents(thread, threadUrl);
  if (events.length === 0) throw new Error("incident source contained no readable events");
  const sourceDigest = sha256({
    ref,
    title,
    events,
  });
  return {
    source: {
      kind: "incident_thread",
      ref,
      read_mode: readMode,
      events_read: events.length,
      source_digest: sourceDigest,
      observed_at: nowIso(),
    },
    title,
    url: threadUrl,
    events,
  };
}

function toApiIssueUrl(value) {
  const parsed = new URL(requireString(value, "incident_source.url", { maxLength: 2_000 }));
  if (parsed.protocol !== "https:") throw new Error("incident source must use HTTPS");

  if (parsed.hostname === GITHUB_API_HOST && /^\/repos\/[^/]+\/[^/]+\/issues\/\d+$/.test(parsed.pathname)) {
    return parsed.toString();
  }
  if (parsed.hostname === GITHUB_HOST) {
    const match = parsed.pathname.match(/^\/([^/]+)\/([^/]+)\/issues\/(\d+)(?:\/)?$/);
    if (match) return `https://${GITHUB_API_HOST}/repos/${match[1]}/${match[2]}/issues/${match[3]}`;
  }
  throw new Error("thread_url supports public GitHub issue URLs only");
}

async function fetchJson(url) {
  let response;
  try {
    response = await fetch(url, {
      headers: {
        accept: "application/vnd.github+json",
        "user-agent": `${PACKAGE_NAME}/${PACKAGE_VERSION}`,
      },
      signal: AbortSignal.timeout(15_000),
    });
  } catch {
    throw new Error("incident source fetch failed");
  }
  const length = Number(response.headers.get("content-length") ?? 0);
  if (Number.isFinite(length) && length > MAX_RESPONSE_BYTES) throw new Error("incident source response is too large");
  let body;
  try {
    body = await response.text();
  } catch {
    throw new Error("incident source response could not be read");
  }
  if (Buffer.byteLength(body, "utf8") > MAX_RESPONSE_BYTES) throw new Error("incident source response is too large");
  if (!response.ok) throw new Error(`incident source returned HTTP ${response.status}`);
  try {
    return JSON.parse(body);
  } catch {
    throw new Error("incident source did not return JSON");
  }
}

function githubThread(issue, comments, requestedUrl) {
  requireObject(issue, "GitHub issue response");
  const issueNumber = String(issue.number ?? "issue");
  const issueUrl = typeof issue.html_url === "string" && issue.html_url ? issue.html_url : requestedUrl;
  const events = [
    {
      id: `issue-${issueNumber}`,
      at: issue.created_at ?? null,
      author: issue.user?.login ?? "unknown",
      url: issueUrl,
      text: clampText(typeof issue.body === "string" ? issue.body : ""),
    },
  ];
  if (!events[0].text) events.shift();
  if (Array.isArray(comments)) {
    for (const comment of comments.slice(0, 100)) {
      if (!comment || typeof comment !== "object" || typeof comment.body !== "string" || !comment.body.trim()) continue;
      events.push({
        id: `comment-${String(comment.id ?? events.length)}`,
        at: comment.created_at ?? null,
        author: comment.user?.login ?? "unknown",
        url: comment.html_url ?? `${issueUrl}#issuecomment-${String(comment.id ?? events.length)}`,
        text: clampText(comment.body),
      });
    }
  }
  return {
    title: typeof issue.title === "string" && issue.title.trim() ? issue.title.trim() : `GitHub issue #${issueNumber}`,
    url: issueUrl,
    events,
  };
}

async function readGitHubIssue(source) {
  const requestedUrl = toApiIssueUrl(source.url ?? source.ref);
  const issue = await fetchJson(requestedUrl);
  const commentsUrl = typeof issue.comments_url === "string" ? issue.comments_url : `${requestedUrl}/comments`;
  const comments = Number(issue.comments ?? 0) > 0 ? await fetchJson(`${commentsUrl}?per_page=100`) : [];
  return threadFromObject(githubThread(issue, comments, source.url ?? source.ref), source.url ?? source.ref, "github_issue_api");
}

function readFixture(source) {
  const file = safeFixturePath(source.ref);
  const parsed = parseJsonFile(file);
  const thread = parsed.thread ?? parsed.incident ?? parsed;
  return threadFromObject(thread, `fixture://${source.ref}`, "checked_in_fixture");
}

function readProjection(source) {
  const projection = source.projection ?? source.record ?? source.thread;
  if (!projection) throw new Error("read_projection source must contain a resolved projection");
  const ref = requireString(source.ref, "incident_source.ref", { maxLength: 2_000 });
  return threadFromObject(projection, ref, "incident_read_projection");
}

async function handler(envelope) {
  const source = requireObject(field(envelope, "incident_source"), "incident_source");
  const kind = requireString(source.kind, "incident_source.kind", { maxLength: 60 });
  let incident;
  if (kind === "thread_url" || kind === "github_issue") {
    incident = await readGitHubIssue(source);
  } else if (kind === "fixture") {
    incident = readFixture(source);
  } else if (kind === "inline") {
    incident = threadFromObject(source.thread, source.ref ?? "inline://incident", "inline_harness_fixture");
  } else if (kind === "read_projection") {
    incident = readProjection(source);
  } else {
    throw new Error("incident_source.kind must be thread_url, github_issue, read_projection, fixture, or inline");
  }
  return { incident };
}

if (isMainModule(import.meta.url)) {
  runCli(handler);
}

export { handler as readIncident };
