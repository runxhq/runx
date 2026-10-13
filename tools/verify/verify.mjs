#!/usr/bin/env node
//
// Independent receipt verifier. Uses ONLY the Node standard library (no runx,
// no third-party crypto). The point: you do not have to trust runx to believe a
// runx receipt. You recompute the canonical body hash yourself, and you verify
// the Ed25519 signature yourself, with a tool you already have.
//
// Needs Node.js 22 or later, whose JSON.parse hands a reviver each number's
// source text.
//
// Usage:
//   node verify.mjs <receipt.json> --issuer-type <local|hosted|ci> --kid <kid> --pubkey <base64-raw-ed25519>
//   node verify.mjs <receipt.json> --jwks <jwks-file-or-url>
//   [--walk-ancestry] [--receipt-dir <dir>]
//   [--schema <receipt.schema.json>]  (default: this repository's schemas/receipt.schema.json)
//
// You name the keys you trust: one issuer type, key id and public key, or a
// JWKS whose keys each carry a `kid` and an `issuer_type`. A receipt verifies
// only under a trusted key of its own issuer type and key id; nothing is
// trusted by default.
//
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

if (Number(process.versions.node.split(".")[0]) < 22) {
  process.stderr.write("verify.mjs needs Node.js 22 or later to read receipt numbers exactly\n");
  process.exit(2);
}

// The one receipt-tree rule every runx verifier applies (runx-receipts
// MAX_RECEIPT_TREE_DEPTH and MAX_RECEIPT_TREE_RECEIPTS): each receipt committed
// once by its parent, at most 1024 receipts and 64 levels below the root.
const MAX_ANCESTRY_DEPTH = 64;
const MAX_TREE_RECEIPTS = 1024;

function arg(name) {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

function hasFlag(name) {
  return process.argv.includes(name);
}

// runx.receipt.c14n.v1: strip the envelope's own signature and digest,
// then serialize with keys sorted by Unicode code point, compact, standard
// JSON escaping. (oss/crates/runx-contracts/src/canonical_json/serializer.rs)
function canon(v) {
  if (v === null) return "null";
  if (typeof v === "boolean") return v ? "true" : "false";
  if (typeof v === "bigint" || typeof v === "number") return canonNumber(v);
  if (typeof v === "string") return JSON.stringify(v);
  if (Array.isArray(v)) return "[" + v.map(canon).join(",") + "]";
  return "{" + Object.keys(v).sort(byCodePoint).map((k) => JSON.stringify(k) + ":" + canon(v[k])).join(",") + "}";
}

// Rust orders keys by bytes, which is Unicode code point order. JavaScript's
// default sort compares UTF-16 code units, which disagrees above U+FFFF.
function byCodePoint(a, b) {
  const left = [...a];
  const right = [...b];
  for (let i = 0; i < Math.min(left.length, right.length); i += 1) {
    const order = left[i].codePointAt(0) - right[i].codePointAt(0);
    if (order !== 0) return order;
  }
  return left.length - right.length;
}

const I64_MIN = -(2n ** 63n);
const U64_MAX = 2n ** 64n - 1n;

// Integers in the 64-bit range read exactly, as the issuer reads them;
// JSON.parse alone rounds past 2^53. A number must be written the one way the
// issuer writes it: `1.0`, `1e2` or `-0` is refused, as the Rust reader
// refuses it, rather than hashed as `1`, `100` or `0`.
function parseReceiptJson(text) {
  return JSON.parse(text, (_key, value, context) => {
    if (typeof value !== "number") return value;
    if (/^-?\d+$/.test(context.source)) {
      const integer = BigInt(context.source);
      if (integer.toString() !== context.source) throw new Error(`number ${context.source} is not written canonically`);
      return integer >= I64_MIN && integer <= U64_MAX ? integer : value;
    }
    if (canonNumber(value) !== context.source || Number.isInteger(value)) {
      throw new Error(`number ${context.source} is not written canonically`);
    }
    return value;
  });
}

// Integers as written; every other number as JSON.stringify writes it.
function canonNumber(n) {
  if (typeof n === "bigint") return n.toString();
  if (!Number.isFinite(n)) throw new Error("non-finite numbers are not valid JSON");
  return JSON.stringify(n);
}

// The receipt's published JSON Schema (schemas/receipt.schema.json) decides
// its wire shape: the issuer writes exactly that, so a document outside it is
// not a receipt runx issued, whatever key signed it. The schema is inline and
// uses the assertions below; any other assertion keyword fails closed rather
// than passing unchecked.
const SCHEMA_ANNOTATIONS = new Set(["$id", "$schema", "title", "description", "x-runx-schema", "x-runx-packet"]);

function jsonType(value) {
  if (Array.isArray(value)) return "array";
  if (value === null) return "null";
  if (typeof value === "bigint") return "integer";
  if (typeof value === "number") return Number.isInteger(value) ? "integer" : "number";
  return typeof value;
}

function schemaFaults(schema, value, at, faults) {
  for (const keyword of Object.keys(schema)) {
    if (!SCHEMA_ANNOTATIONS.has(keyword) && !Object.hasOwn(ASSERTIONS, keyword)) {
      throw new Error(`receipt schema uses ${keyword} at ${at || "/"}, which this verifier does not check`);
    }
  }
  for (const [keyword, check] of Object.entries(ASSERTIONS)) {
    if (Object.hasOwn(schema, keyword)) check(schema, value, at, faults);
  }
  return faults;
}

const ASSERTIONS = {
  type(schema, value, at, faults) {
    const actual = jsonType(value);
    const allowed = [schema.type].flat();
    if (!allowed.includes(actual) && !(actual === "integer" && allowed.includes("number"))) {
      faults.push(`${at || "/"} must be ${allowed.join(" or ")}`);
    }
  },
  enum(schema, value, at, faults) {
    if (!schema.enum.some((option) => canon(option) === canon(value))) faults.push(`${at || "/"} is not an allowed value`);
  },
  const(schema, value, at, faults) {
    if (canon(schema.const) !== canon(value)) faults.push(`${at || "/"} must be ${JSON.stringify(schema.const)}`);
  },
  minLength(schema, value, at, faults) {
    if (typeof value === "string" && [...value].length < schema.minLength) faults.push(`${at || "/"} is too short`);
  },
  maxLength(schema, value, at, faults) {
    if (typeof value === "string" && [...value].length > schema.maxLength) faults.push(`${at || "/"} is too long`);
  },
  pattern(schema, value, at, faults) {
    if (typeof value === "string" && !new RegExp(schema.pattern, "u").test(value)) faults.push(`${at || "/"} does not match its pattern`);
  },
  minItems(schema, value, at, faults) {
    if (Array.isArray(value) && value.length < schema.minItems) faults.push(`${at || "/"} has too few items`);
  },
  items(schema, value, at, faults) {
    if (Array.isArray(value)) value.forEach((item, index) => schemaFaults(schema.items, item, `${at}/${index}`, faults));
  },
  required(schema, value, at, faults) {
    if (jsonType(value) !== "object") return;
    for (const key of schema.required) if (!Object.hasOwn(value, key)) faults.push(`${at}/${key} is required`);
  },
  properties(schema, value, at, faults) {
    if (jsonType(value) !== "object") return;
    for (const [key, member] of Object.entries(schema.properties)) {
      if (Object.hasOwn(value, key)) schemaFaults(member, value[key], `${at}/${key}`, faults);
    }
  },
  additionalProperties(schema, value, at, faults) {
    if (jsonType(value) !== "object") return;
    for (const key of Object.keys(value)) {
      if (schema.properties && Object.hasOwn(schema.properties, key)) continue;
      if (schema.additionalProperties === false) faults.push(`${at}/${key} is not a member`);
      else if (schema.additionalProperties !== true) schemaFaults(schema.additionalProperties, value[key], `${at}/${key}`, faults);
    }
  },
};

function readReceiptSchema() {
  const schemaPath = arg("--schema") ?? new URL("../../schemas/receipt.schema.json", import.meta.url);
  return JSON.parse(fs.readFileSync(schemaPath, "utf8"));
}

function publicKeyFromRaw(rawB64) {
  return publicKeyFromRawBytes(Buffer.from(rawB64, "base64"));
}

function publicKeyFromRawBytes(raw) {
  if (raw.length !== 32) throw new Error("pubkey must be 32 raw bytes");
  const spki = Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), raw]);
  return crypto.createPublicKey({ key: spki, format: "der", type: "spki" });
}

const USAGE = "usage: node verify.mjs <receipt.json> (--issuer-type <type> --kid <kid> --pubkey <b64> | --jwks <path-or-url>) [--walk-ancestry] [--receipt-dir <dir>] [--schema <path>]";
const receiptPath = process.argv[2];
if (!receiptPath || receiptPath.startsWith("--")) {
  console.error(USAGE);
  process.exit(2);
}
let trustedKeys;
let receiptSchema;
let walkAncestry = false;
let ancestryIndex = new Map();

/** The keys the caller trusts, each with the issuer type and key id it signs as. */
async function resolveTrustedKeys() {
  if (arg("--pubkey")) {
    const issuerType = arg("--issuer-type");
    const kid = arg("--kid");
    if (!issuerType || !kid) throw new Error("--pubkey requires --issuer-type and --kid");
    return [trustedKey(issuerType, kid, strictBase64(arg("--pubkey"), "base64"))];
  }
  if (arg("--jwks")) return trustedKeysFromJwks(arg("--jwks"));
  throw new Error(USAGE);
}

async function trustedKeysFromJwks(locator) {
  const jwks = JSON.parse(await readText(locator));
  if (!Array.isArray(jwks.keys)) {
    throw new Error("JWKS must contain a keys array");
  }
  return jwks.keys
    .filter((key) => key?.kty === "OKP" && key?.crv === "Ed25519" && typeof key?.x === "string"
      && typeof key?.kid === "string" && typeof key?.issuer_type === "string")
    .map((key) => trustedKey(key.issuer_type, key.kid, strictBase64(key.x, "base64url")));
}

// A key decodes only from its one spelling: Buffer.from skips characters it
// does not know and accepts missing padding, so the text must re-encode to
// itself.
function strictBase64(text, encoding) {
  const bytes = Buffer.from(text, encoding);
  if (bytes.toString(encoding) !== text) throw new Error(`key is not canonical ${encoding}`);
  return bytes;
}

function trustedKey(issuerType, kid, raw) {
  return {
    issuerType,
    kid,
    key: publicKeyFromRawBytes(raw),
    sha256: "sha256:" + crypto.createHash("sha256").update(raw).digest("hex"),
  };
}

async function readText(locator) {
  if (locator.startsWith("http://") || locator.startsWith("https://")) {
    const response = await fetch(locator);
    if (!response.ok) throw new Error(`failed to fetch JWKS: ${response.status}`);
    return response.text();
  }
  if (locator.startsWith("file://")) return fs.readFileSync(new URL(locator), "utf8");
  return fs.readFileSync(locator, "utf8");
}

function sha256Prefixed(text) {
  return "sha256:" + crypto.createHash("sha256").update(text, "utf8").digest("hex");
}

function readReceipt(filePath) {
  return parseReceiptJson(fs.readFileSync(filePath, "utf8"));
}

function receiptBody(r) {
  const body = { ...r };
  delete body.signature;
  delete body.digest;
  return body;
}

function receiptIdentityBody(r) {
  const body = receiptBody(r);
  delete body.id;
  return body;
}

function childReceiptId(ref) {
  if (ref?.type !== "receipt" || typeof ref?.uri !== "string") return undefined;
  return ref.uri.startsWith("runx:receipt:") ? ref.uri.slice("runx:receipt:".length) : undefined;
}

function buildReceiptIndex(dir) {
  const index = new Map();
  for (const filePath of candidateReceiptFiles(dir)) {
    let value;
    try {
      value = readReceipt(filePath);
    } catch {
      continue;
    }
    collectReceipts(value, filePath, index);
  }
  return index;
}

function candidateReceiptFiles(dir) {
  const files = [];
  try {
    for (const name of fs.readdirSync(dir)) {
      if (name !== "index.json" && name.endsWith(".json")) files.push(path.join(dir, name));
    }
  } catch {
    return files;
  }
  return files;
}

function collectReceipts(value, sourcePath, index) {
  if (!value || typeof value !== "object") return;
  if (value.schema === "runx.receipt.v1" && typeof value.id === "string") {
    const bucket = index.get(value.id) ?? [];
    bucket.push({ receipt: value, sourcePath });
    index.set(value.id, bucket);
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) collectReceipts(item, sourcePath, index);
    return;
  }
  for (const child of Object.values(value)) collectReceipts(child, sourcePath, index);
}

function resolveIndexedReceipt(id) {
  if (!/^[A-Za-z0-9:_-]+$/.test(id)) return { status: "unsafe" };
  const matches = ancestryIndex.get(id) ?? [];
  if (matches.length === 0) return { status: "missing" };
  if (matches.length > 1) return { status: "ambiguous", matches };
  return { status: "found", ...matches[0] };
}

let allPass = true;

function check(label, ok, detail) {
  allPass = allPass && ok;
  console.log(`  [${ok ? "PASS" : "FAIL"}] ${label}`);
  if (!ok) console.log(`         ${detail}`);
}

function verifyOne(filePath, depth, seen, ancestors, receipt) {
  const r = receipt ?? readReceipt(filePath);
  const prefix = depth === 0 ? "receipt" : `child depth=${depth}`;
  console.log(`${prefix}: ${filePath}`);
  console.log(`  id         : ${r.id}`);
  console.log(`  disposition: ${r.seal?.disposition}${r.seal?.reason ? ` (${r.seal.reason})` : ""}`);
  console.log(`  issuer     : ${r.issuer?.type}/${r.issuer?.kid}`);
  console.log("");

  if (ancestors.has(r.id)) {
    check("ancestry is acyclic", false, `cycle at ${r.id}`);
    return;
  }
  if (seen.has(r.id)) {
    check("each receipt is committed once in its tree", false, `${r.id} is committed twice`);
    return;
  }
  seen.add(r.id);
  check("the tree holds at most its receipt bound", seen.size <= MAX_TREE_RECEIPTS,
    `receipts=${seen.size} bound=${MAX_TREE_RECEIPTS}`);
  if (seen.size > MAX_TREE_RECEIPTS) return;

  // 1. The document has the receipt's published wire shape.
  const faults = schemaFaults(receiptSchema, r, "", []);
  check("document has the runx.receipt.v1 shape", faults.length === 0, faults.join("; "));

  // 2. The receipt names a key we trust: its issuer type and key id select
  // it, and the key hash it carries is that key's.
  const trusted = trustedKeys.find((key) => key.issuerType === r.issuer?.type && key.kid === r.issuer?.kid);
  check("issuer is a trusted key", Boolean(trusted), `issuer=${r.issuer?.type}/${r.issuer?.kid}`);
  check("issuer key matches the trusted public key", trusted?.sha256 === r.issuer?.public_key_sha256,
    `issuer=${r.issuer?.public_key_sha256} trusted=${trusted?.sha256}`);

  // 3. The digest is the canonical hash of THIS receipt's body (content binding).
  const recomputed = sha256Prefixed(canon(receiptBody(r)));
  check("digest is the hash of the receipt body", recomputed === r.digest,
    `receipt=${r.digest} recomputed=${recomputed}`);

  // 4. The id is content-addressed: the body without the id it derives.
  const recomputedId = sha256Prefixed(canon(receiptIdentityBody(r)));
  check("id is the hash of the receipt identity body", recomputedId === r.id,
    `receipt=${r.id} recomputed=${recomputedId}`);

  // 5. The Ed25519 signature is valid over that digest.
  let sigOk = false, sigDetail = "";
  try {
    // One wire form: "base64:" and the canonical unpadded URL-safe encoding.
    const encoded = r.signature.value.startsWith("base64:") ? r.signature.value.slice("base64:".length) : "";
    const sig = Buffer.from(encoded, "base64url");
    if (!/^[A-Za-z0-9_-]+$/.test(encoded) || sig.toString("base64url") !== encoded) {
      throw new Error("signature is not base64: canonical unpadded URL-safe Base64");
    }
    sigOk = Boolean(trusted) && sig.length === 64 && crypto.verify(null, Buffer.from(r.digest), trusted.key, sig);
    sigDetail = `alg=${r.signature.alg} sigBytes=${sig.length}`;
  } catch (e) { sigDetail = String(e); }
  check("signature is valid over the digest", sigOk, sigDetail);

  const children = Array.isArray(r.lineage?.children) ? r.lineage.children : [];
  if (!walkAncestry) {
    console.log("");
    return;
  }
  check("ancestry depth stays bounded", depth < MAX_ANCESTRY_DEPTH, `depth=${depth}`);
  for (const [index, childRef] of children.entries()) {
    const childId = childReceiptId(childRef);
    check(`lineage child ${index} uses runx receipt ref`, Boolean(childId),
      `ref=${JSON.stringify(childRef)}`);
    if (!childId) continue;
    const expectedDigest = childRef.locator;
    const resolved = resolveIndexedReceipt(childId);
    check(`lineage child ${index} receipt resolves locally`, resolved.status === "found",
      `status=${resolved.status}`);
    if (resolved.status !== "found") continue;
    const child = resolved.receipt;
    check(`lineage child ${index} ref resolves by id`, child.id === childId,
      `child=${child.id} expected=${childId}`);
    if (expectedDigest) {
      check(`lineage child ${index} locator matches child digest`, child.digest === expectedDigest,
        `locator=${expectedDigest} child=${child.digest}`);
    }
    console.log("");
    verifyOne(resolved.sourcePath, depth + 1, seen, new Set([...ancestors, r.id]), child);
  }
  console.log("");
}

async function main() {
  const rootPath = path.resolve(receiptPath);
  const rootReceipt = readReceipt(rootPath);
  trustedKeys = await resolveTrustedKeys();
  receiptSchema = readReceiptSchema();
  walkAncestry = hasFlag("--walk-ancestry");
  const receiptDir = arg("--receipt-dir") ?? path.dirname(rootPath);
  ancestryIndex = walkAncestry ? buildReceiptIndex(receiptDir) : new Map();

  verifyOne(rootPath, 0, new Set(), new Set(), rootReceipt);
  console.log("");
  console.log(allPass
    ? (walkAncestry
      ? "VERIFIED: a trusted key signed this receipt tree. Ancestry was walked offline with the Node standard library, trusting nothing from runx."
      : "VERIFIED: a trusted key signed exactly this receipt content. Verified with the Node standard library, trusting nothing from runx.")
    : "NOT VERIFIED.");
  process.exit(allPass ? 0 : 1);
}

await main();
