// The renamed tree a compiling check builds: every file the repo holds,
// renamed and moved as the rename would write them, in a scratch directory.

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";

/** Writes the renamed tree at `dir`; returns it and the map from a moved path back to its original. */
export function renamedTree({ root, writes, moves, dir }) {
  rmSync(dir, { recursive: true, force: true });
  const listed = execFileSync("git", ["-C", root, "ls-files", "--cached", "--others", "--exclude-standard"], { encoding: "utf8", maxBuffer: 1 << 28 })
    .split("\n")
    .filter((file) => file && existsSync(path.join(root, file)));
  const written = new Map(writes);
  for (const file of listed) {
    const to = path.join(dir, moves.get(file) ?? file);
    mkdirSync(path.dirname(to), { recursive: true });
    if (written.has(file)) writeFileSync(to, written.get(file));
    else copyFileSync(path.join(root, file), to);
  }
  return { tree: dir, back: new Map([...moves].map(([from, to]) => [to, from])) };
}
