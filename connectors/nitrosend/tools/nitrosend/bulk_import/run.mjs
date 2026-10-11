#!/usr/bin/env node

import { createHash, randomUUID } from "node:crypto";
import fs from "node:fs";
import { isIP } from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const API_URL = "https://api.nitrosend.com/mcp";
const SENSITIVE_KEYS = /authorization|api[_-]?key|bearer|credential|secret|token|signed[_-]?id|direct[_-]?upload/iu;
const SECRET_VALUE = /\b(?:nskey|wpkey)_(?:live|test)_[A-Za-z0-9_-]+\b/gu;
const SIGNED_URL = /https:\/\/[^\s"']+\?[^\s"']+/gu;

// runx-architecture-allow: transient-signed-upload
// This single adapter keeps the provider's signed PUT URL and headers out of graph state and receipts.

export async function invokeBulkImport(inputs, options = {}) {
  const fetchImpl = options.fetchImpl ?? fetch;
  const apiKey = options.apiKey ?? process.env.NITROSEND_API_KEY;
  const invalid = validateInputs(inputs);
  if (invalid) return packet(invalid.decision, inputs, null, null, invalid.blockers);
  if (!text(apiKey)) {
    return packet("needs_input", inputs, null, null, [
      "Nitrosend credential is missing; configure a Nitrosend Runx credential",
    ]);
  }

  const csvPath = inputs.csv_path;
  const metadata = await fileMetadata(csvPath);
  if (metadata.error) return packet("needs_input", inputs, null, null, [metadata.error]);

  const reservationCall = await invokeMcp({
    fetchImpl,
    apiKey,
    brandSid: inputs.brand_sid,
    arguments: providerArguments({
      upload: metadata.upload,
      source_id: inputs.source_id,
      consent_basis: inputs.consent_basis,
      dry_run: inputs.dry_run === true,
      idempotency_key: inputs.idempotency_key,
    }),
  });
  const reservation = reservationCall.output;
  if (reservation.decision !== "ok" || inputs.dry_run === true) {
    return { ...reservation, operation: "import_contacts_file", source_id: inputs.source_id };
  }

  const reserved = providerData(reservationCall.rawResult);
  const directUpload = reserved.direct_upload;
  const signedId = reserved.signed_id;
  if (!directUpload?.url || !directUpload?.headers || !signedId) {
    return packet("provider_error", inputs, reservation.evidence, null, [
      "Nitrosend did not return a complete authorized upload reservation",
    ]);
  }

  let uploadUrl;
  try {
    uploadUrl = admittedUploadUrl(directUpload.url);
  } catch (error) {
    return packet("provider_error", inputs, reservation.evidence, null, [safeError(error)]);
  }
  let uploadResponse;
  try {
    const uploadHeaders = new Headers(directUpload.headers);
    uploadHeaders.set("content-length", String(metadata.upload.byte_size));
    uploadResponse = await fetchImpl(uploadUrl, {
      method: "PUT",
      headers: uploadHeaders,
      body: fs.createReadStream(csvPath),
      duplex: "half",
    });
  } catch (error) {
    return packet("provider_error", inputs, reservation.evidence, null, [safeError(error)]);
  }
  if (!uploadResponse.ok) {
    return packet("provider_error", inputs, reservation.evidence, null, [
      `authorized CSV upload failed with HTTP ${uploadResponse.status}`,
    ]);
  }

  const finalizedCall = await invokeMcp({
    fetchImpl,
    apiKey,
    brandSid: inputs.brand_sid,
    arguments: providerArguments({
      signed_id: signedId,
      source_id: inputs.source_id,
      consent_basis: inputs.consent_basis,
      resource: "contacts",
      parser: "default",
      columns: inputs.columns,
      options: inputs.options,
      dry_run: false,
      idempotency_key: inputs.idempotency_key,
    }),
  });
  const finalized = finalizedCall.output;
  finalized.operation = "import_contacts_file";
  finalized.source_id = inputs.source_id;
  finalized.evidence = {
    ...finalized.evidence,
    upload: {
      filename: metadata.upload.filename,
      byte_size: metadata.upload.byte_size,
      checksum_verified: true,
      signed_url_retained: false,
    },
  };
  return finalized;
}

async function invokeMcp({ fetchImpl, apiKey, brandSid, arguments: args }) {
  const requestId = randomUUID();
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 30_000);
  let response;
  try {
    response = await fetchImpl(API_URL, {
      method: "POST",
      headers: {
        accept: "application/json, text/event-stream",
        authorization: `Bearer ${apiKey}`,
        "content-type": "application/json",
        ...(text(brandSid) ? { "x-brand-sid": text(brandSid) } : {}),
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: requestId,
        method: "tools/call",
        params: { name: "nitro_import_contacts", arguments: args },
      }),
      signal: controller.signal,
    });
  } catch (error) {
    return {
      output: packet("provider_error", {}, null, null, [safeError(error)]),
      rawResult: null,
    };
  } finally {
    clearTimeout(timeout);
  }

  let body;
  try {
    body = await response.text();
  } catch (error) {
    return {
      output: packet("provider_error", {}, null, null, [safeError(error)]),
      rawResult: null,
    };
  }
  const evidence = {
    request_id: requestId,
    http_status: response.status,
    credential_material: "redacted",
  };
  if (!response.ok) {
    const authority = response.status === 401 || response.status === 403;
    return {
      output: packet(
        authority ? "needs_input" : "provider_error",
        {},
        evidence,
        null,
        [authority ? "Nitrosend rejected the configured credential" : `Nitrosend returned HTTP ${response.status}`],
      ),
      rawResult: null,
    };
  }
  try {
    const rawResult = parseToolContent(parseJsonOrSse(body));
    const safeResult = redact(rawResult);
    const providerError = safeResult?.error === true || safeResult?.isError === true;
    return {
      output: packet(
        providerError ? "provider_error" : "ok",
        {},
        evidence,
        safeResult,
        providerError ? [safeResult?.message || "Nitrosend rejected the import"] : [],
      ),
      rawResult,
    };
  } catch (error) {
    return {
      output: packet("provider_error", {}, evidence, null, [safeError(error)]),
      rawResult: null,
    };
  }
}

function validateInputs(inputs) {
  if (!path.isAbsolute(text(inputs.csv_path)) || path.extname(inputs.csv_path).toLowerCase() !== ".csv") {
    return { decision: "needs_input", blockers: ["csv_path must be an absolute path to a .csv file"] };
  }
  if (!text(inputs.source_id) || !text(inputs.consent_basis)) {
    return { decision: "needs_input", blockers: ["source_id and consent_basis are required"] };
  }
  if (/purchased|scraped|data\s*broker/iu.test(inputs.consent_basis)) {
    return { decision: "refused", blockers: ["purchased, scraped, and data-broker contact sources are not permitted"] };
  }
  if (inputs.dry_run !== true && !text(inputs.idempotency_key)) {
    return { decision: "refused", blockers: ["a real contact import requires idempotency_key"] };
  }
  return null;
}

async function fileMetadata(csvPath) {
  let stat;
  try {
    stat = fs.statSync(csvPath);
  } catch {
    return { error: "the CSV file does not exist or is not readable" };
  }
  if (!stat.isFile() || stat.size === 0) {
    return { error: "csv_path must identify a non-empty regular file" };
  }
  const hash = createHash("md5");
  await new Promise((resolve, reject) => {
    const stream = fs.createReadStream(csvPath);
    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("error", reject);
    stream.on("end", resolve);
  });
  return {
    upload: {
      filename: path.basename(csvPath),
      content_type: "text/csv",
      byte_size: stat.size,
      checksum: hash.digest("base64"),
    },
  };
}

function admittedUploadUrl(rawUrl) {
  const url = new URL(rawUrl);
  if (url.protocol !== "https:") throw new Error("Nitrosend returned a non-HTTPS upload URL");
  if (url.hostname === "localhost" || isIP(url.hostname)) {
    throw new Error("Nitrosend returned a disallowed upload host");
  }
  return url;
}

function providerArguments(args) {
  const { source_id: _sourceId, consent_basis: _consentBasis, ...providerArgs } = args;
  return providerArgs;
}

function parseJsonOrSse(body) {
  const value = body.trim();
  if (!value) throw new Error("Nitrosend returned an empty MCP response");
  if (value.startsWith("{")) return JSON.parse(value);
  const payloads = value
    .split(/\r?\n/u)
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice(5).trim())
    .filter((line) => line && line !== "[DONE]");
  if (payloads.length === 0) throw new Error("Nitrosend returned an invalid MCP event stream");
  return JSON.parse(payloads.at(-1));
}

function parseToolContent(payload) {
  if (payload.error) throw new Error(payload.error.message || "Nitrosend MCP request failed");
  const content = payload.result?.content;
  if (!Array.isArray(content)) return payload.result ?? {};
  const value = content.find((item) => item?.type === "text")?.text;
  if (typeof value !== "string") return payload.result ?? {};
  try {
    const parsed = JSON.parse(value);
    if (parsed && typeof parsed === "object" && parsed.meta?.tool && Object.hasOwn(parsed, "result")) {
      return parsed.result;
    }
    return parsed;
  } catch {
    return { message: value };
  }
}

function packet(decision, inputs, evidence, result, blockers) {
  const safeResult = redact(result);
  return {
    decision,
    provider: "nitrosend",
    mode: "act",
    operation: "import_contacts_file",
    tool: "nitro_import_contacts",
    provider_ref: providerReference(safeResult),
    result: safeResult,
    evidence,
    blockers,
    source_id: text(inputs.source_id) || undefined,
  };
}

function providerData(result) {
  return result?.data ?? result?.result ?? result ?? {};
}

function providerReference(result) {
  const data = providerData(result);
  const id = data.import_id ?? data.id;
  return id === undefined || id === null ? null : `nitrosend:import_contacts_file:${id}`;
}

function redact(value) {
  if (Array.isArray(value)) return value.map(redact);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, child]) => [
      key,
      SENSITIVE_KEYS.test(key) ? "[REDACTED]" : redact(child),
    ]));
  }
  return typeof value === "string" ? redactText(value) : value;
}

function redactText(value) {
  return String(value)
    .replaceAll(SECRET_VALUE, "[REDACTED]")
    .replaceAll(SIGNED_URL, "[REDACTED_URL]")
    .slice(0, 2_000);
}

function safeError(error) {
  return redactText(error instanceof Error ? error.message : String(error));
}

function text(value) {
  return typeof value === "string" ? value.trim() : "";
}

async function main() {
  const raw = process.env.RUNX_INPUTS_PATH
    ? fs.readFileSync(process.env.RUNX_INPUTS_PATH, "utf8")
    : process.env.RUNX_INPUTS_JSON || "{}";
  process.stdout.write(`${JSON.stringify(await invokeBulkImport(JSON.parse(raw)), null, 2)}\n`);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  main().catch((error) => {
    process.stderr.write(`${JSON.stringify({ error: { message: safeError(error) } })}\n`);
    process.exitCode = 1;
  });
}
