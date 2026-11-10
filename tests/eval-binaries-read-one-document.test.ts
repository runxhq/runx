import { spawn } from "node:child_process";

import { describe, expect, it } from "vitest";

import { rustExampleBinary } from "../scripts/lib/rust-example.mjs";

// The evaluators the repository's scripts spawn read one JSON document from
// stdin and answer at its end. A parent can hold a child's stdin open past a
// complete request, as one with the descriptor inherited elsewhere does, so a
// reader waiting for end of file never answers. Here the request is written
// whole and stdin is never closed: each answers and exits all the same.
async function answerWithStdinOpen(binary: string, request: string) {
  const child = spawn(binary, [], { stdio: ["pipe", "pipe", "inherit"] });
  const stdout: Buffer[] = [];
  child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
  const answered = new Promise<void>((resolve) => child.stdout.on("end", resolve));
  const exited = new Promise<number | null>((resolve) => child.on("exit", resolve));
  child.stdin.write(request);
  const [code] = await Promise.all([exited, answered]);
  child.stdin.destroy();
  return { code, answer: JSON.parse(Buffer.concat(stdout).toString("utf8")) };
}

describe("an evaluator reads one document from stdin and stops at its end", () => {
  it("parser_eval answers with stdin still open", async () => {
    const markdown = "---\nname: probe\ndescription: A probe skill.\n---\n# Probe\n\nProbe it.\n";
    const { code, answer } = await answerWithStdinOpen(
      rustExampleBinary("parser_eval"),
      JSON.stringify({ inputs: [{ kind: "parser.validateSkillMarkdown", markdown }] }),
    );
    expect(code).toBe(0);
    expect(answer.status).toBe("success");
    expect(answer.result.value).toHaveLength(1);
  });

  it("kernel_eval_batch answers with stdin still open", async () => {
    const { code, answer } = await answerWithStdinOpen(rustExampleBinary("kernel_eval_batch"), "[]");
    expect(code).toBe(0);
    expect(answer).toEqual([]);
  });
});
