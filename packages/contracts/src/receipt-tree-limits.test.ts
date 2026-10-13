import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { RECEIPT_TREE_LIMITS } from "./index.js";

describe("receipt tree limits", () => {
  it("are the bounds runx-receipts applies", () => {
    const tree = readFileSync(new URL("../../../crates/runx-receipts/src/tree.rs", import.meta.url), "utf8");
    const bound = (name: string) => Number(new RegExp(`pub const ${name}: usize = (\\d+);`).exec(tree)?.[1]);
    expect(RECEIPT_TREE_LIMITS).toEqual({
      maxReceipts: bound("MAX_RECEIPT_TREE_RECEIPTS"),
      maxDepth: bound("MAX_RECEIPT_TREE_DEPTH"),
    });
  });
});
