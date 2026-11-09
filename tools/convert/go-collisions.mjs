// Which renamed bare words collide in Go, by building them: the renamed tree,
// in a scratch copy, through `go build ./...` and `go vet ./...`, beside the
// same two runs over the original module. A renamed site collides when an
// error or a vet finding the original doesn't have sits at it, and a new one
// no renamed site carries is refused where it is. A file no package builds
// is left to the file-level refusal. The runs are offline (GOPROXY=off), so a
// module's toolchain and dependencies come from the local module cache.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { renamedTree } from "./renamed-tree.mjs";
import { mapping } from "./script-collisions.mjs";

export const GO = /\.go$/;

/** The nearest directory above a file that holds a go.mod. */
function moduleOf(root, file) {
  for (let dir = path.dirname(file); ; dir = path.dirname(dir)) {
    if (existsSync(path.join(root, dir, "go.mod"))) return dir;
    if (dir === "." || dir === "/" || dir === "") return undefined;
  }
}

function go(args, cwd) {
  const ran = spawnSync("go", args, { cwd, encoding: "utf8", maxBuffer: 1 << 30, env: { ...process.env, GOPROXY: "off", GOFLAGS: "-mod=readonly", GOWORK: "off" } });
  if (ran.error) throw new Error(`go ${args[0]} couldn't run: ${ran.error.message}`);
  return ran;
}

/** Each `file:line:col: message` a go command printed, its path relative to the repo. */
function diagnostics(output, cwd, root) {
  const found = [];
  for (const line of output.split("\n")) {
    const m = /^(?:vet: )?(.+?\.go):(\d+):(\d+): (.*)$/.exec(line.trim());
    if (m) found.push({ file: path.relative(root, path.resolve(cwd, m[1])), line: Number(m[2]), column: Number(m[3]), message: m[4] });
  }
  return found;
}

/** The JavaScript index of a 1-based line and byte column in a text. */
function indexAt(text, line, column) {
  let start = 0;
  for (let n = 1; n < line; n += 1) start = text.indexOf("\n", start) + 1;
  const rest = text.slice(start, text.indexOf("\n", start) < 0 ? text.length : text.indexOf("\n", start));
  return start + Buffer.from(rest, "utf8").subarray(0, column - 1).toString("utf8").length;
}

/**
 * The renamed Go sites that collide, by file. `files` maps each Go file with
 * renamed sites to `{ original, renamed, edits, sites }`; `edited` holds every
 * renamed file's; `writes` and `moves` are the whole rename's.
 */
export function checkGo({ root, files, edited, writes, moves, cache }) {
  const verdicts = new Map();
  if (files.size === 0) return verdicts;
  const scratch = cache ?? path.join(tmpdir(), `runx-convert-go-${createHash("sha256").update(path.resolve(root)).digest("hex").slice(0, 12)}`);
  const byModule = new Map();
  for (const file of files.keys()) {
    const module = moduleOf(root, file);
    if (module === undefined) verdicts.set(file, { unread: "no go.mod holds it" });
    else byModule.set(module, [...(byModule.get(module) ?? []), file]);
  }
  if (byModule.size === 0) return verdicts;
  const { tree, back } = renamedTree({ root, writes, moves, dir: path.join(scratch, "renamed-go") });
  const textOf = (side, file) => (side === "renamed" ? edited.get(file)?.renamed : edited.get(file)?.original) ?? readFileSync(path.join(root, file), "utf8");

  for (const [module, members] of byModule) {
    const runs = {};
    for (const [side, cwd] of [["base", path.join(root, module)], ["renamed", path.join(tree, module)]]) {
      const build = go(["build", "./..."], cwd);
      const vet = go(["vet", "./..."], cwd);
      const base = side === "base" ? root : tree;
      const found = [...diagnostics(build.stderr, cwd, base).map((d) => ({ ...d, kind: "build" })), ...diagnostics(vet.stderr, cwd, base).map((d) => ({ ...d, kind: "vet" }))];
      const listed = go(["list", "-e", "-f", "{{$d := .Dir}}{{range .GoFiles}}{{$d}}/{{.}}\n{{end}}{{range .TestGoFiles}}{{$d}}/{{.}}\n{{end}}{{range .XTestGoFiles}}{{$d}}/{{.}}\n{{end}}", "./..."], cwd);
      runs[side] = { found, failed: side === "base" && (build.status !== 0 || vet.status !== 0), built: new Set(listed.stdout.split("\n").filter(Boolean)) };
    }
    if (runs.base.failed) {
      for (const file of members) verdicts.set(file, { unread: "the original module doesn't build and vet clean, so nothing compares against it" });
      continue;
    }
    // A finding, by its kind and where it sits in the original.
    const at = (side, d) => {
      const file = side === "base" ? d.file : back.get(d.file) ?? d.file;
      const index = indexAt(textOf(side, file), d.line, d.column);
      return { file, index: side === "renamed" && edited.has(file) ? mapping(edited.get(file).edits).toOriginal(index) : index };
    };
    const before = new Set(runs.base.found.map((d) => { const p = at("base", d); return `${d.kind}|${p.file}|${p.index}`; }));
    const seen = new Set();
    for (const d of runs.renamed.found) {
      const p = at("renamed", d);
      const key = `${d.kind}|${p.file}|${p.index}`;
      if (before.has(key) || seen.has(key)) continue;
      seen.add(key);
      const reason = `go ${d.kind}: ${d.message}`;
      // Go names a point, not a span: the site there, or else the renamed
      // sites on its line, such as a := whose left side was renamed.
      const sites = files.get(p.file)?.sites ?? [];
      const original = textOf("base", p.file);
      const lineOf = (index) => original.slice(0, index).split("\n").length;
      const exact = sites.filter((one) => p.index >= one.at && p.index < one.at + Math.max(one.token.length, 1));
      const hit = exact.length > 0 ? exact : sites.filter((one) => lineOf(one.at) === lineOf(p.index));
      const verdict = verdicts.get(p.file) ?? { refused: [] };
      if (hit.length > 0) for (const site of hit) verdict.refused?.push({ site, reason });
      else verdict.refused?.push({ site: { at: p.index, token: "", next: "" }, reason, unattributed: true });
      verdicts.set(p.file, verdict);
    }
    for (const file of members) {
      // go list names a package's files by their real paths.
      const built = path.join(tree, moves.get(file) ?? file);
      if (!existsSync(built) || !runs.renamed.built.has(realpathSync(built))) verdicts.set(file, { unread: "no package builds it" });
      else if (!verdicts.has(file)) verdicts.set(file, { refused: [] });
    }
  }
  return verdicts;
}
