import path from "node:path";
import { RECEIPT_TREE_LIMITS, canonicalJsonStringify, sha256Prefixed, validateReceiptContract, type ReceiptContract } from "@runxhq/contracts";
import { readBoundedJson } from "./artifacts.js";
import type { ReceiptTree } from "./types.js";

/** The largest one stored receipt may be. */
export const MAX_RECEIPT_BYTES = 8 * 1024 * 1024;

const CONTENT_ADDRESS = /^sha256:[0-9a-f]{64}$/u;

/**
 * What a receipt's body commits, recomputed rather than read from it: the
 * digest its signature covers (every field but `signature` and `digest`) and
 * the content address its id must be (that body without `id`).
 * An integer past 2^53 does not survive `JSON.parse`, so such a receipt fails
 * here rather than passing with a rounded body.
 */
export function receiptCommitments(receipt: ReceiptContract): { readonly id: string; readonly digest: string } {
  const { signature: _signature, digest: _digest, ...body } = receipt;
  const { id: _id, ...identity } = body;
  return {
    id: sha256Prefixed(canonicalJsonStringify(identity)),
    digest: sha256Prefixed(canonicalJsonStringify(body)),
  };
}

/**
 * The signed receipt tree a run sealed, read from its receipt directory:
 * the root, then every receipt its `lineage.children` commit, in signed
 * order. Each child is the receipt its parent names by id, with the digest
 * its parent signed as the reference's locator, and every receipt's id and
 * digest are recomputed from its body. The tree is bounded in depth,
 * members and bytes, and holds no receipt twice. Signatures are the host's to
 * verify under its own trust.
 */
export async function readReceiptTree(rootId: string, receiptDir: string, budget: number): Promise<ReceiptTree> {
  let remaining = budget;
  const seen = new Set<string>();
  const read = async (id: string): Promise<ReceiptContract> => {
    if (!CONTENT_ADDRESS.test(id)) throw new Error(`Receipt id ${id} is not a content address.`);
    if (seen.has(id)) throw new Error(`Receipt ${id} appears twice in its tree.`);
    if (seen.size >= RECEIPT_TREE_LIMITS.maxReceipts) throw new Error(`Receipt tree exceeds ${RECEIPT_TREE_LIMITS.maxReceipts} receipts.`);
    seen.add(id);
    const stored = await readBoundedJson(path.join(receiptDir, `${id.replace(":", "-")}.json`), receiptDir, Math.min(remaining, MAX_RECEIPT_BYTES));
    remaining -= stored.bytes.byteLength;
    const receipt = validateReceiptContract(stored.value);
    if (receipt.id !== id) throw new Error(`Receipt file for ${id} holds ${receipt.id}.`);
    const commitments = receiptCommitments(receipt);
    if (commitments.id !== id) throw new Error(`Receipt ${id} is not the content address of its body.`);
    if (commitments.digest !== receipt.digest) throw new Error(`Receipt ${id}'s digest is not its body's.`);
    return receipt;
  };

  const root = await read(rootId);
  const descendants: ReceiptContract[] = [];
  let level: readonly ReceiptContract[] = [root];
  for (let depth = 1; level.length > 0; depth += 1) {
    const next: ReceiptContract[] = [];
    for (const parent of level) {
      for (const reference of parent.lineage.children) {
        if (depth > RECEIPT_TREE_LIMITS.maxDepth) throw new Error(`Receipt tree exceeds depth ${RECEIPT_TREE_LIMITS.maxDepth}.`);
        const id = reference.type === "receipt" && reference.uri.startsWith("runx:receipt:")
          ? reference.uri.slice("runx:receipt:".length)
          : undefined;
        if (!id) throw new Error(`Receipt ${parent.id} names a child that is not a receipt.`);
        const child = await read(id);
        if (child.digest !== reference.locator) throw new Error(`Receipt ${id}'s digest differs from the one ${parent.id} signed.`);
        next.push(child);
      }
    }
    descendants.push(...next);
    level = next;
  }
  return { root, descendants };
}
