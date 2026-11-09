import assert from "node:assert/strict";
import { test } from "node:test";

import { rustExampleBinary } from "./rust-example.mjs";

// A cargo build replaces each example's file even when nothing needs
// building, so a test that ran one while another was starting the binary
// found it missing. With the prebuilt binary named, cargo never runs: here an
// empty PATH would make any cargo run fail.
test("an example named prebuilt is that binary, and cargo never runs", () => {
  for (const [example, variable] of [
    ["parser_eval", "RUNX_PARSER_EVAL_BIN"],
    ["kernel_eval_batch", "RUNX_KERNEL_EVAL_BIN"],
  ]) {
    const prebuilt = `/prebuilt/${example}`;
    assert.equal(rustExampleBinary(example, { PATH: "", [variable]: prebuilt }), prebuilt);
  }
});
