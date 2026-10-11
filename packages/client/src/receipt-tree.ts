import path from "node:path";
import { validateReceiptContract, type ReceiptContract } from "@runxhq/contracts";
import { readBoundedJson } from "./artifacts.js";
import type { ReceiptTree } from "./types.js";

/** A tree deeper than the native verifier walks is refused. */
export const MAX_RECEIPT_TREE_DEPTH = 64;
/** The most receipts one run's tree may hold. */
export const MAX_RECEIPT_TREE_MEMBERS = 1024;
/** The largest one stored receipt may be. */
export const MAX_RECEIPT_BYTES = 8 * 1024 * 1024;

const CONTENT_ADDRESS = /^sha256:[0-9a-f]{64}$/u;

/**
 * The signed receipt tree a run sealed, read from its receipt directory:
 * the root, then every receipt its `lineage.children` commit, in signed
 * order. Each child is the receipt its parent names by id, with the digest
 * its parent signed as the reference's locator. The tree is bounded in depth,
 * members and bytes, and holds no receipt twice. Signatures are the host's to
 * verify under its own trust.
 */
export async function readReceiptTree(rootId: string, receiptDir: string, budget: number): Promise<ReceiptTree> {
  let remaining = budget;
  const seen = new Set<string>();
  const read = async (id: string): Promise<ReceiptContract> => {
    if (!CONTENT_ADDRESS.test(id)) throw new Error(`Receipt id ${id} is not a content address.`);
    if (seen.has(id)) throw new Error(`Receipt ${id} appears twice in its tree.`);
    if (seen.size >= MAX_RECEIPT_TREE_MEMBERS) throw new Error(`Receipt tree exceeds ${MAX_RECEIPT_TREE_MEMBERS} receipts.`);
    seen.add(id);
    const stored = await readBoundedJson(path.join(receiptDir, `${id.replace(":", "-")}.json`), receiptDir, Math.min(remaining, MAX_RECEIPT_BYTES));
    remaining -= stored.bytes.byteLength;
    const receipt = validateReceiptContract(stored.value);
    if (receipt.id !== id) throw new Error(`Receipt file for ${id} holds ${receipt.id}.`);
    return receipt;
  };

  const root = await read(rootId);
  const descendants: ReceiptContract[] = [];
  let level: readonly ReceiptContract[] = [root];
  for (let depth = 1; level.length > 0; depth += 1) {
    const next: ReceiptContract[] = [];
    for (const parent of level) {
      for (const reference of parent.lineage.children) {
        if (depth > MAX_RECEIPT_TREE_DEPTH) throw new Error(`Receipt tree exceeds depth ${MAX_RECEIPT_TREE_DEPTH}.`);
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
