#!/usr/bin/env node
//
// Independent receipt verifier. Uses ONLY the Node standard library (no runx,
// no third-party crypto). The point: you do not have to trust runx to believe a
// runx receipt. You recompute the canonical body hash yourself, and you verify
// the Ed25519 signature yourself, with a tool you already have.
//
// Usage:
//   node verify.mjs <receipt.json> [--pubkey <base64-raw-ed25519>] [--seed <base64-32-byte-seed>]
//   node verify.mjs <receipt.json> [--jwks <jwks-file-or-url>] [--walk-ancestry] [--receipt-dir <dir>]
//   [--schema <receipt.schema.json>]  (default: this repository's schemas/receipt.schema.json)
//
// If neither --pubkey nor --seed is given, the public demo seed is used (the
// same throwaway test key the demo signs with). For a real deployment you pass
// the issuer's published --pubkey and trust nothing else.
//
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const DEMO_SEED_B64 = "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI="; // public test key
const MAX_ANCESTRY_DEPTH = 64;

function arg(name) {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

function hasFlag(name) {
  return process.argv.includes(name);
}

// runx.receipt.c14n.v1: strip the envelope's own signature/digest/metadata,
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
// JSON.parse alone rounds past 2^53.
function parseReceiptJson(text) {
  return JSON.parse(text, (_key, value, context) => {
    if (typeof value !== "number" || !/^-?\d+$/.test(context.source)) return value;
    const integer = BigInt(context.source);
    return integer >= I64_MIN && integer <= U64_MAX ? integer : value;
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

function publicKeyFromSeed(seedB64) {
  const seed = Buffer.from(seedB64, "base64");
  if (seed.length !== 32) throw new Error("seed must be 32 bytes");
  const pkcs8 = Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), seed]);
  const priv = crypto.createPrivateKey({ key: pkcs8, format: "der", type: "pkcs8" });
  return crypto.createPublicKey(priv);
}

function publicKeyFromRaw(rawB64) {
  return publicKeyFromRawBytes(Buffer.from(rawB64, "base64"));
}

function publicKeyFromRawBytes(raw) {
  if (raw.length !== 32) throw new Error("pubkey must be 32 raw bytes");
  const spki = Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), raw]);
  return crypto.createPublicKey({ key: spki, format: "der", type: "spki" });
}

function rawPubBytes(keyObj) {
  return keyObj.export({ format: "der", type: "spki" }).subarray(-32);
}

const receiptPath = process.argv[2];
if (!receiptPath || receiptPath.startsWith("--")) {
  console.error("usage: node verify.mjs <receipt.json> [--pubkey <b64>] [--seed <b64>] [--jwks <path-or-url>] [--walk-ancestry] [--receipt-dir <dir>] [--schema <path>]");
  process.exit(2);
}
let pub;
let receiptSchema;
let walkAncestry = false;
let ancestryIndex = new Map();

async function resolvePublicKey(receipt) {
  if (arg("--pubkey")) return publicKeyFromRaw(arg("--pubkey"));
  if (arg("--jwks")) return publicKeyFromJwks(arg("--jwks"), receipt.issuer);
  return publicKeyFromSeed(arg("--seed") ?? DEMO_SEED_B64);
}

async function publicKeyFromJwks(locator, issuer) {
  const jwks = JSON.parse(await readText(locator));
  if (!Array.isArray(jwks.keys)) {
    throw new Error("JWKS must contain a keys array");
  }
  for (const key of jwks.keys) {
    if (key?.kty !== "OKP" || key?.crv !== "Ed25519" || typeof key?.x !== "string") continue;
    if (issuer?.kid && key.kid !== issuer.kid) continue;
    const raw = Buffer.from(key.x, "base64url");
    const keyHash = "sha256:" + crypto.createHash("sha256").update(raw).digest("hex");
    if (issuer?.public_key_sha256 && keyHash !== issuer.public_key_sha256) continue;
    return publicKeyFromRawBytes(raw);
  }
  throw new Error(`JWKS did not contain Ed25519 key ${issuer?.kid ?? "<unknown>"}`);
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
  delete body.metadata;
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

function verifyOne(filePath, depth, seen, receipt) {
  const r = receipt ?? readReceipt(filePath);
  const prefix = depth === 0 ? "receipt" : `child depth=${depth}`;
  console.log(`${prefix}: ${filePath}`);
  console.log(`  id         : ${r.id}`);
  console.log(`  disposition: ${r.seal?.disposition} (${r.seal?.reason_code})`);
  console.log(`  issuer     : ${r.issuer?.type}/${r.issuer?.kid}`);
  console.log("");

  if (seen.has(r.id)) {
    check("ancestry is acyclic", false, `cycle at ${r.id}`);
    return;
  }
  seen.add(r.id);

  // 1. The document has the receipt's published wire shape.
  const faults = schemaFaults(receiptSchema, r, "", []);
  check("document has the runx.receipt.v1 shape", faults.length === 0, faults.join("; "));

  // 2. The named key matches the public key we are verifying with.
  const ourKeyHash = "sha256:" + crypto.createHash("sha256").update(rawPubBytes(pub)).digest("hex");
  check("issuer key matches the public key", ourKeyHash === r.issuer?.public_key_sha256,
    `issuer=${r.issuer?.public_key_sha256} ours=${ourKeyHash}`);

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
    const sig = Buffer.from(r.signature.value.split("base64:")[1], "base64url");
    sigOk = sig.length === 64 && crypto.verify(null, Buffer.from(r.digest), pub, sig);
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
    verifyOne(resolved.sourcePath, depth + 1, seen, child);
  }
  console.log("");
}

async function main() {
  const rootPath = path.resolve(receiptPath);
  const rootReceipt = readReceipt(rootPath);
  pub = await resolvePublicKey(rootReceipt);
  receiptSchema = readReceiptSchema();
  walkAncestry = hasFlag("--walk-ancestry");
  const receiptDir = arg("--receipt-dir") ?? path.dirname(rootPath);
  ancestryIndex = walkAncestry ? buildReceiptIndex(receiptDir) : new Map();

  verifyOne(rootPath, 0, new Set(), rootReceipt);
  console.log("");
  console.log(allPass
    ? (walkAncestry
      ? "VERIFIED: runx signed this receipt tree. Ancestry was walked offline with the Node standard library, trusting nothing from runx."
      : "VERIFIED: runx signed exactly this receipt content. Verified with the Node standard library, trusting nothing from runx.")
    : "NOT VERIFIED.");
  process.exit(allPass ? 0 : 1);
}

await main();
