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

/** The corpus key of `issuerType`, trusted as `trustAs`. */
async function corpusTrust(issuerType: string, trustAs = issuerType): Promise<string[]> {
  const { keys } = JSON.parse(await readFile(path.join(corpus, "verifier.json"), "utf8"));
  const key = keys.find((candidate: { issuer_type: string }) => candidate.issuer_type === issuerType);
  return ["--issuer-type", trustAs, "--kid", key.kid, "--pubkey", key.public_key_base64];
}

/** The standalone verifier's verdict on a document, against the trusted key. */
async function verify(
  document: string,
  trust: Promise<string[]> = corpusTrust("hosted"),
): Promise<{ readonly verified: boolean; readonly output: string }> {
  try {
    const { stdout } = await run(process.execPath, [verifier, document, ...(await trust)]);
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
  it("agrees with Rust on every hosted-signed corpus receipt", async () => {
    for (const [name, valid] of [
      ["valid-production", true],
      ["valid-wide-text", true],
      ["tampered-body", false],
      ["tampered-signature", false],
    ] as const) {
      const verdict = await verify(path.join(corpus, name, "receipt.json"));
      expect(verdict.verified, `${name}\n${verdict.output}`).toBe(valid);
    }
  });

  it("verifies a receipt only under a trusted key of its own issuer type", async () => {
    const local = path.join(corpus, "broken-lineage-reference", "receipt.json");
    expect((await verify(local, corpusTrust("local"))).verified).toBe(true);
    const asHosted = await verify(local, corpusTrust("local", "hosted"));
    expect(asHosted.verified).toBe(false);
    expect(asHosted.output).toContain("[FAIL] issuer is a trusted key");
    const untrusted = await run(process.execPath, [verifier, local]).catch((error) => error);
    expect(untrusted.code).not.toBe(0);
  });

  it("refuses a second spelling of a valid signature", async () => {
    // The last character of an unpadded 64-byte signature carries four unused
    // bits; flipping one spells the same bytes in a form the signer never writes.
    const document = await altered("valid-production", (receipt) => {
      const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
      const value: string = receipt.signature.value;
      const flipped = alphabet[alphabet.indexOf(value.at(-1)!) ^ 1];
      receipt.signature.value = value.slice(0, -1) + flipped;
    });
    const verdict = await verify(document);
    expect(verdict.verified).toBe(false);
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
