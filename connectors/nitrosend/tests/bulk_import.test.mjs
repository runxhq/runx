import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { invokeBulkImport } from "../tools/nitrosend/bulk_import/run.mjs";

function response(status, body) {
  return { ok: status >= 200 && status < 300, status, text: async () => body };
}

function mcpResponse(data) {
  return response(200, JSON.stringify({
    jsonrpc: "2.0",
    id: "fixture",
    result: { content: [{ type: "text", text: JSON.stringify({ data }) }] },
  }));
}

test("streams the file and never returns signed upload material", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "nitrosend-import-"));
  const csvPath = path.join(directory, "contacts.csv");
  fs.writeFileSync(csvPath, "email,first_name\nfixture@example.com,Fixture\n");
  let apiCalls = 0;
  let uploadCalls = 0;
  try {
    const result = await invokeBulkImport({
      csv_path: csvPath,
      source_id: "fixture-signup",
      consent_basis: "First-party signup form opt-in",
      dry_run: false,
      idempotency_key: "import-1",
    }, {
      apiKey: "fixture-key",
      fetchImpl: async (url, request) => {
        if (String(url).startsWith("https://uploads.example.com/")) {
          uploadCalls += 1;
          assert.equal(request.headers.get("content-length"), String(fs.statSync(csvPath).size));
          for await (const _chunk of request.body) {
            // Consume the stream as the upload endpoint would.
          }
          return response(200, "");
        }
        apiCalls += 1;
        return apiCalls === 1
          ? mcpResponse({
              signed_id: "fixture-signed-id",
              direct_upload: {
                url: "https://uploads.example.com/contact.csv?signature=secret",
                headers: { "content-type": "text/csv", "x-upload-token": "secret" },
              },
            })
          : mcpResponse({ import_id: 42, status: "processing", total_rows: 1 });
      },
    });

    assert.equal(result.decision, "ok");
    assert.equal(result.result.data.import_id, 42);
    assert.equal(result.evidence.upload.signed_url_retained, false);
    assert.equal(apiCalls, 2);
    assert.equal(uploadCalls, 1);
    assert.equal(JSON.stringify(result).includes("fixture-signed-id"), false);
    assert.equal(JSON.stringify(result).includes("uploads.example.com"), false);
    assert.equal(JSON.stringify(result).includes("x-upload-token"), false);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("streams a multi-chunk file with its exact signed length", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "nitrosend-import-"));
  const csvPath = path.join(directory, "contacts.csv");
  const csv = `email,first_name\n${"fixture@example.com,Fixture\n".repeat(8_192)}`;
  fs.writeFileSync(csvPath, csv);
  const expectedSize = fs.statSync(csvPath).size;
  let apiCalls = 0;
  let uploadedBytes = 0;
  let uploadChunks = 0;
  try {
    const result = await invokeBulkImport({
      csv_path: csvPath,
      source_id: "fixture-signup",
      consent_basis: "First-party signup form opt-in",
      dry_run: false,
      idempotency_key: "import-multi-chunk",
    }, {
      apiKey: "fixture-key",
      fetchImpl: async (url, request) => {
        if (String(url).startsWith("https://uploads.example.com/")) {
          assert.equal(request.headers.get("content-length"), String(expectedSize));
          for await (const chunk of request.body) {
            uploadedBytes += chunk.length;
            uploadChunks += 1;
          }
          return response(200, "");
        }
        apiCalls += 1;
        return apiCalls === 1
          ? mcpResponse({
              signed_id: "fixture-signed-id",
              direct_upload: {
                url: "https://uploads.example.com/contact.csv?signature=secret",
                headers: { "content-type": "text/csv", "x-upload-token": "secret" },
              },
            })
          : mcpResponse({ import_id: 43, status: "processing", total_rows: 8_192 });
      },
    });

    assert.equal(result.decision, "ok");
    assert.equal(result.result.data.import_id, 43);
    assert.equal(uploadedBytes, expectedSize);
    assert.ok(uploadChunks > 1);
    assert.equal(apiCalls, 2);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("refuses non-consensual sources before any provider call", async () => {
  let called = false;
  const result = await invokeBulkImport({
    csv_path: "/tmp/contacts.csv",
    source_id: "broker-list",
    consent_basis: "Purchased from a data broker",
    dry_run: true,
    idempotency_key: "import-1",
  }, {
    apiKey: "fixture-key",
    fetchImpl: async () => {
      called = true;
      return response(500, "unexpected");
    },
  });
  assert.equal(result.decision, "refused");
  assert.equal(called, false);
});

test("keeps dry-run reservations bounded and does not upload", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "nitrosend-import-"));
  const csvPath = path.join(directory, "contacts.csv");
  fs.writeFileSync(csvPath, "email\nfixture@example.com\n");
  let calls = 0;
  try {
    const result = await invokeBulkImport({
      csv_path: csvPath,
      source_id: "fixture-signup",
      consent_basis: "First-party signup opt-in",
      dry_run: true,
      idempotency_key: "import-dry-run",
    }, {
      apiKey: "fixture-key",
      fetchImpl: async () => {
        calls += 1;
        return mcpResponse({ valid: true, signed_id: "must-not-escape" });
      },
    });

    assert.equal(result.decision, "ok");
    assert.equal(result.source_id, "fixture-signup");
    assert.equal(calls, 1);
    assert.equal(JSON.stringify(result).includes("must-not-escape"), false);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("blocks disallowed upload destinations before sending file bytes", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "nitrosend-import-"));
  const csvPath = path.join(directory, "contacts.csv");
  fs.writeFileSync(csvPath, "email\nfixture@example.com\n");
  let calls = 0;
  try {
    const result = await invokeBulkImport({
      csv_path: csvPath,
      source_id: "fixture-signup",
      consent_basis: "First-party signup opt-in",
      dry_run: false,
      idempotency_key: "import-localhost",
    }, {
      apiKey: "fixture-key",
      fetchImpl: async () => {
        calls += 1;
        return mcpResponse({
          signed_id: "must-not-escape",
          direct_upload: {
            url: "https://127.0.0.1/upload?signature=secret",
            headers: { "x-upload-token": "secret" },
          },
        });
      },
    });

    assert.equal(result.decision, "provider_error");
    assert.equal(calls, 1);
    assert.equal(JSON.stringify(result).includes("must-not-escape"), false);
    assert.equal(JSON.stringify(result).includes("signature=secret"), false);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});

test("returns a redacted provider error when transport fails", async () => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "nitrosend-import-"));
  const csvPath = path.join(directory, "contacts.csv");
  fs.writeFileSync(csvPath, "email\nfixture@example.com\n");
  try {
    const credential = ["nskey", "live", "do_not_expose"].join("_");
    const result = await invokeBulkImport({
      csv_path: csvPath,
      source_id: "fixture-signup",
      consent_basis: "First-party signup opt-in",
      dry_run: true,
      idempotency_key: "import-network-error",
    }, {
      apiKey: "fixture-key",
      fetchImpl: async () => {
        throw new Error(`failed with ${credential}`);
      },
    });

    assert.equal(result.decision, "provider_error");
    assert.equal(JSON.stringify(result).includes(credential), false);
    assert.match(result.blockers[0], /\[REDACTED\]/u);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
});
