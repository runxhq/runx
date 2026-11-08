// A hosted registry on loopback, for the Node tests and scripts that drive a
// built runx: one stateful service speaking the hosted registry's wire
// contract (the routes runx reads, and the publish route it writes), which
// keeps what is published and signs each version with a key the runtime is
// told to trust, stating each version's tier in a signed tier statement. It
// is the Node twin of crates/runx-runtime/tests/registry_fixture.rs, which
// serves the Rust tests;
// Cloud's own contract test holds the service side.
//
// The server runs on a worker thread, so a caller that drives runx with
// spawnSync never blocks the registry runx is talking to.

import { createHash, generateKeyPairSync, sign } from "node:crypto";
import http from "node:http";
import { Worker, isMainThread, parentPort, workerData } from "node:worker_threads";

const SIGNED_MANIFEST_SCHEMA = "runx.registry.signed_manifest.v1";
const TIER_STATEMENT_SCHEMA = "runx.registry.tier_statement.v1";
/** When a statement the registry issues holds: before any run, until long after. */
const HELD = { issued_at: "2026-11-01T00:00:00Z", expires_at: "2099-01-01T00:00:00Z" };
const KEY_ID = "runx-loopback-registry-ed25519";
const SIGNER_ID = "runx-loopback-registry";
const PRINCIPAL_ID = "rxp_loopback_publisher";
const TOKEN = "rxk_loopback_publisher";

/**
 * Start a loopback registry. `publisher` is the namespace a publish goes into
 * when it names none, and the one the runtime trusts the registry's key to
 * sign. Resolves to its URL; `env`, the environment a runx reaching it runs
 * with; `publisherEnv`, that with the token a publish authenticates with; and
 * `stop()`.
 */
export function startLoopbackRegistry({ publisher = "acme" } = {}) {
  const worker = new Worker(new URL(import.meta.url), {
    workerData: { loopbackRegistry: true, publisher },
  });
  return new Promise((resolve, reject) => {
    worker.once("error", reject);
    worker.once("message", ({ url, publicKeyBase64 }) => {
      const env = {
        RUNX_REGISTRY_KEYS: JSON.stringify([
          { key_id: KEY_ID, public_key: publicKeyBase64, publisher, source: url },
        ]),
        RUNX_PUBLIC_API_ALLOW_PRIVATE_NETWORK: "1",
      };
      resolve({
        url,
        env,
        publisherEnv: { ...env, RUNX_PUBLIC_API_TOKEN: TOKEN },
        stop: () => worker.terminate(),
      });
    });
  });
}

if (!isMainThread && workerData?.loopbackRegistry) {
  serve(workerData.publisher);
}

function serve(defaultPublisher) {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const publicKeyBase64 = Buffer.from(publicKey.export({ format: "der", type: "spki" }))
    .subarray(-32)
    .toString("base64");
  /** Every version of each skill, oldest first; the last is current. */
  const skills = new Map();
  let installs = 0;

  const server = http.createServer((request, response) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => {
      let reply;
      try {
        reply = route(request, Buffer.concat(chunks).toString("utf8"));
      } catch (error) {
        reply = [400, { status: "error", error: { code: "invalid_request", detail: String(error) } }];
      }
      const [status, body] = reply;
      const text = JSON.stringify(body);
      response.writeHead(status, {
        "content-type": "application/json",
        "content-length": Buffer.byteLength(text),
      });
      response.end(text);
    });
  });

  function route(request, body) {
    const url = new URL(request.url, "http://loopback");
    const segments = url.pathname.split("/").filter(Boolean).map(decodeURIComponent);
    const authorized = /^Bearer \S+/u.test(request.headers.authorization ?? "");
    const method = request.method;
    const [v1, kind, ...rest] = segments;
    if (v1 !== "v1") return notFound();
    if (kind === "me" && rest.length === 0 && method === "GET") {
      return authorized
        ? success({ principal: { principal_id: PRINCIPAL_ID } })
        : [401, { status: "error", error: { code: "unauthorized" } }];
    }
    if (kind === "connectors") {
      return rest.length === 0 && method === "GET" ? success({ connectors: [] }) : notFound();
    }
    if (kind !== "skills") return notFound();
    if (rest.length === 0) {
      if (method === "POST" && authorized) return publish(JSON.parse(body));
      if (method === "GET") return success({ skills: search(url.searchParams) });
      return notFound();
    }
    if (rest.length === 1 && method === "GET") {
      // No skill holds a root name here.
      return notFound();
    }
    const [publisher, nameAndVersion, ...tail] = rest;
    if (tail.length === 0 && method === "GET") {
      const [name, version] = splitVersion(nameAndVersion);
      const record = find(`${publisher}/${name}`, version);
      return record ? success({ skill: { ...pick(record, DETAIL_FIELDS), attestations: [] } }) : notFound();
    }
    const skillId = `${publisher}/${nameAndVersion}`;
    if (tail.length === 1 && tail[0] === "acquire" && method === "POST") {
      const record = find(skillId, JSON.parse(body || "{}").version ?? undefined);
      if (!record) return notFound();
      installs += 1;
      return success({
        install_count: installs,
        acquisition: { ...pick(record, ACQUISITION_FIELDS), attestations: [] },
      });
    }
    if (tail.length === 1 && tail[0] === "versions" && method === "GET") {
      const versions = skills.get(skillId);
      if (!versions) return notFound();
      return success({
        versions: [...versions].reverse().map((record) => ({ version: record.version })),
        has_more: false,
        next_cursor: null,
      });
    }
    if (tail.length === 3 && tail[0] === "versions" && tail[2] === "record" && method === "GET") {
      const record = find(skillId, tail[1]);
      return record ? success({ record }) : notFound();
    }
    return notFound();
  }

  /** A publish, as `POST /v1/skills` takes it: versions are immutable. */
  function publish(request) {
    const files = [...(request.package_files ?? [])].sort(byPath);
    const contentDigest = registryContentDigest([
      { path: "SKILL.md", content: request.markdown },
      ...(typeof request.profile_document === "string"
        ? [{ path: "run.yaml", content: request.profile_document }]
        : []),
      ...files,
    ]);
    const version = request.version ?? `sha-${contentDigest.slice("sha256:".length, "sha256:".length + 12)}`;
    const publisher = request.publisher ?? defaultPublisher;
    const { name, description } = frontmatter(request.markdown);
    const skillId = `${publisher}/${name}`;
    const existing = find(skillId, version);
    if (existing && existing.signed_manifest.content_digest !== contentDigest) {
      return [409, { status: "error", error: { code: "version_conflict" } }];
    }
    const reference = `${skillId}@${version}`;
    const record = existing ?? {
      skill_id: skillId,
      name,
      ...(description ? { description } : {}),
      version,
      digest: sha256Prefixed(request.markdown),
      signed_manifest: signManifest(skillId, version, contentDigest),
      tier_statement: stateTier(skillId, version, contentDigest, "community"),
      markdown: request.markdown,
      ...(typeof request.profile_document === "string"
        ? {
            profile_document: request.profile_document,
            profile_digest: sha256Prefixed(request.profile_document),
          }
        : {}),
      ...(files.length > 0
        ? {
            package_files: files,
            bundle_digest: sha256Prefixed(
              JSON.stringify({ files: files.map(({ content, path }) => ({ content, path })) }),
            ),
          }
        : {}),
      method_names: [],
      trust_tier: "community",
      required_scopes: [],
      tags: [],
      publisher: { kind: "organization", id: `pub_${publisher}`, handle: publisher },
      install_command: posixCommand(["runx", "registry", "install", reference]),
      run_command: posixCommand(["runx", "skill", reference]),
      created_at: "2026-11-08T00:00:00Z",
      updated_at: "2026-11-08T00:00:00Z",
    };
    if (!existing) {
      skills.set(skillId, [...(skills.get(skillId) ?? []), record]);
    }
    return success({
      publish: {
        status: existing ? "unchanged" : "published",
        skill_id: skillId,
        publisher,
        name,
        version,
        digest: record.digest,
        ...(record.profile_digest ? { profile_digest: record.profile_digest } : {}),
        trust_tier: record.trust_tier,
        install_command: record.install_command,
        run_command: record.run_command,
        public_url: `http://loopback/x/${reference}`,
      },
    });
  }

  function search(params) {
    const needle = (params.get("q") ?? "").toLowerCase();
    const limit = Number(params.get("limit") ?? 20);
    return [...skills.values()]
      .map((versions) => versions.at(-1))
      .filter((record) =>
        [record.skill_id, record.name, record.description ?? ""].some((text) =>
          text.toLowerCase().includes(needle)))
      .slice(0, limit)
      .map((record) => ({
        ...pick(record, [
          "skill_id", "name", "description", "version", "digest", "method_names",
          "profile_digest", "required_scopes", "tags", "trust_tier", "install_command", "run_command",
        ]),
        profile_mode: record.profile_document === undefined ? "portable" : "profiled",
      }));
  }

  function find(skillId, version) {
    const versions = skills.get(skillId) ?? [];
    return version === undefined ? versions.at(-1) : versions.find((record) => record.version === version);
  }

  function signManifest(name, version, contentDigest) {
    const unsigned = {
      content_digest: contentDigest,
      schema: SIGNED_MANIFEST_SCHEMA,
      signer: { id: SIGNER_ID, key_id: KEY_ID },
      subject: { kind: "skill", name },
      version,
    };
    // The canonical JSON of the envelope without its signature: keys in
    // byte order at every level, as written above.
    const signature = sign(null, Buffer.from(JSON.stringify(unsigned), "utf8"), privateKey);
    return {
      schema: SIGNED_MANIFEST_SCHEMA,
      subject: unsigned.subject,
      version,
      content_digest: contentDigest,
      signer: unsigned.signer,
      signature: { alg: "ed25519", value: `base64:${signature.toString("base64url")}` },
    };
  }

  function stateTier(name, version, contentDigest, tier) {
    const unsigned = {
      content_digest: contentDigest,
      expires_at: HELD.expires_at,
      issued_at: HELD.issued_at,
      schema: TIER_STATEMENT_SCHEMA,
      signer: { id: SIGNER_ID, key_id: KEY_ID },
      subject: { kind: "skill", name },
      tier,
      version,
    };
    // The canonical JSON of the statement without its signature, as the
    // manifest's: keys in byte order at every level, as written above.
    const signature = sign(null, Buffer.from(JSON.stringify(unsigned), "utf8"), privateKey);
    return {
      schema: TIER_STATEMENT_SCHEMA,
      subject: unsigned.subject,
      version,
      content_digest: contentDigest,
      tier,
      issued_at: HELD.issued_at,
      expires_at: HELD.expires_at,
      signer: unsigned.signer,
      signature: { alg: "ed25519", value: `base64:${signature.toString("base64url")}` },
    };
  }

  server.listen(0, "127.0.0.1", () => {
    parentPort.postMessage({ url: `http://127.0.0.1:${server.address().port}`, publicKeyBase64 });
  });
}

const DETAIL_FIELDS = [
  "skill_id", "name", "description", "version", "digest", "signed_manifest", "markdown",
  "profile_digest", "package_files", "bundle_digest", "method_names", "trust_tier",
  "required_scopes", "tags", "publisher", "install_command", "run_command",
];

const ACQUISITION_FIELDS = [
  "skill_id", "name", "version", "digest", "signed_manifest", "tier_statement", "markdown", "profile_document",
  "profile_digest", "package_files", "bundle_digest", "method_names", "trust_tier", "publisher",
];

function success(body) {
  return [200, { status: "success", ...body }];
}

function notFound() {
  return [404, { status: "error", error: { code: "not_found" } }];
}

function pick(record, fields) {
  return Object.fromEntries(fields.filter((field) => record[field] !== undefined).map((field) => [field, record[field]]));
}

function splitVersion(segment) {
  const at = segment.lastIndexOf("@");
  return at > 0 ? [segment.slice(0, at), segment.slice(at + 1)] : [segment, undefined];
}

function byPath(left, right) {
  return left.path < right.path ? -1 : left.path > right.path ? 1 : 0;
}

/** `sha256:` over the canonical JSON of every file of a version, in path order. */
function registryContentDigest(files) {
  const sorted = [...files].sort(byPath);
  return sha256Prefixed(JSON.stringify({ files: sorted.map(({ content, path }) => ({ content, path })) }));
}

function sha256Prefixed(text) {
  return `sha256:${createHash("sha256").update(text, "utf8").digest("hex")}`;
}

/** The manual's frontmatter `name` and `description`. */
function frontmatter(markdown) {
  const match = /^---\r?\n([\s\S]*?)\r?\n---/u.exec(markdown);
  const field = (key) => {
    const line = match?.[1].split(/\r?\n/u).find((entry) => entry.startsWith(`${key}:`));
    return line?.slice(key.length + 1).trim().replace(/^["']|["']$/gu, "");
  };
  const name = field("name");
  if (!name) throw new Error("the manual names no skill");
  return { name, description: field("description") };
}

/** A command a person copies into a POSIX shell, as runx renders one. */
function posixCommand(argv) {
  return {
    shell: "posix",
    argv,
    copy_text: argv
      .map((argument) => (/^[A-Za-z0-9@%+:,./_-]+$/u.test(argument)
        ? argument
        : `'${argument.replaceAll("'", "'\\''")}'`))
      .join(" "),
  };
}
