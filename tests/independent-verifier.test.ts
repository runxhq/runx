import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";

import { afterAll, describe, expect, it } from "vitest";

const run = promisify(execFile);
const root = path.resolve(import.meta.dirname, "..");
const corpus = path.join(root, "fixtures/receipt-verify");
const verifier = path.join(root, "tools/verify/verify.mjs");
const directories: string[] = [];

afterAll(async () => {
  await Promise.all(directories.map((directory) => rm(directory, { recursive: true, force: true })));
});

/** The standalone verifier's verdict on a document, against the corpus key. */
async function verify(document: string): Promise<{ readonly verified: boolean; readonly output: string }> {
  const { public_key_base64: pubkey } = JSON.parse(await readFile(path.join(corpus, "verifier.json"), "utf8"));
  try {
    const { stdout } = await run(process.execPath, [verifier, document, "--pubkey", pubkey]);
    return { verified: true, output: stdout };
  } catch (error) {
    return { verified: false, output: String((error as { stdout?: string }).stdout ?? error) };
  }
}

async function altered(name: string, change: (receipt: Record<string, any>) => void): Promise<string> {
  const receipt = JSON.parse(await readFile(path.join(corpus, name, "receipt.json"), "utf8"));
  change(receipt);
  const directory = await mkdtemp(path.join(os.tmpdir(), "runx-independent-verifier-"));
  directories.push(directory);
  const document = path.join(directory, "receipt.json");
  await writeFile(document, JSON.stringify(receipt));
  return document;
}

describe("the independent receipt verifier", () => {
  it("agrees with Rust on every production-signed corpus receipt", async () => {
    for (const [name, valid] of [
      ["valid-production", true],
      ["valid-wide-keys", true],
      ["tampered-body", false],
      ["tampered-signature", false],
    ] as const) {
      const verdict = await verify(path.join(corpus, name, "receipt.json"));
      expect(verdict.verified, `${name}\n${verdict.output}`).toBe(valid);
    }
  });

  it("refuses a signed document outside the receipt's published shape", async () => {
    for (const change of [
      (receipt: Record<string, any>) => {
        receipt.authority.enforcement.extra = true;
      },
      (receipt: Record<string, any>) => {
        receipt.signature.toString = "unexpected";
      },
      (receipt: Record<string, any>) => {
        delete receipt.authority.derivations;
      },
    ]) {
      const verdict = await verify(await altered("valid-production", change));
      expect(verdict.verified).toBe(false);
      expect(verdict.output).toContain("[FAIL] document has the runx.receipt.v1 shape");
    }
  });
});
