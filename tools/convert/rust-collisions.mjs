// Which renamed bare words collide in Rust, by compiling them: the renamed
// tree, in a scratch copy, through `cargo check` and `cargo clippy` with its
// shadow lints, beside the same two runs over the original tree. A renamed
// site collides when a diagnostic the original doesn't have, a compile error
// or a shadow warning, spans it. A site in a file no target compiles is left
// to the file-level refusal.

import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { mapping } from "./script-collisions.mjs";

export const RUST = /\.rs$/;
const SHADOWS = ["clippy::shadow_unrelated", "clippy::shadow_reuse", "clippy::shadow_same"];

/** The nearest directory above a file whose Cargo.toml is a workspace, or a package. */
function workspaceOf(root, file) {
  let found;
  for (let dir = path.dirname(file); ; dir = path.dirname(dir)) {
    const manifest = path.join(root, dir, "Cargo.toml");
    if (existsSync(manifest)) {
      if (/^\[workspace\]/m.test(readFileSync(manifest, "utf8"))) return dir;
      found ??= dir;
    }
    if (dir === "." || dir === "/" || dir === "") return found;
  }
}

/** Every compiler message of `cargo <command>` in `cwd`, and the source files it compiled. */
function cargo(command, cwd, target, extra) {
  const args = [command, "--workspace", "--all-targets", "--all-features", "--offline", "--message-format=json", ...extra];
  const ran = spawnSync("cargo", args, { cwd, encoding: "utf8", maxBuffer: 1 << 30, env: { ...process.env, CARGO_TARGET_DIR: target, CARGO_TERM_COLOR: "never" } });
  if (ran.error) throw new Error(`cargo ${command} couldn't run: ${ran.error.message}`);
  const messages = ran.stdout
    .split("\n")
    .filter((line) => line.startsWith("{"))
    .map((line) => JSON.parse(line))
    .filter((entry) => entry.reason === "compiler-message")
    .map((entry) => entry.message);
  return { messages, status: ran.status, stderr: ran.stderr };
}

/** The source files a target directory's dep-info says were compiled, as absolute paths. */
function compiled(target, cwd) {
  const files = new Set();
  const deps = path.join(target, "debug", "deps");
  if (!existsSync(deps)) return files;
  for (const name of readdirSync(deps).filter((one) => one.endsWith(".d"))) {
    for (const line of readFileSync(path.join(deps, name), "utf8").split("\n")) {
      const colon = line.indexOf(": ");
      if (colon <= 0) continue;
      // Dep-info names a workspace's own files relative to it.
      for (const file of line.slice(colon + 2).split(/(?<!\\) /)) if (file.endsWith(".rs")) files.add(path.resolve(cwd, file.replace(/\\ /g, " ")));
    }
  }
  return files;
}

/** A message's spans, its children's included. */
function spansOf(message) {
  return [...(message.spans ?? []), ...(message.children ?? []).flatMap((child) => child.spans ?? [])];
}

/** The JavaScript index of a UTF-8 byte offset in a text. */
function indexOfByte(text, byte) {
  return Buffer.from(text, "utf8").subarray(0, byte).toString("utf8").length;
}

/**
 * The renamed Rust sites that collide, by file. `files` maps each Rust file
 * with renamed sites to `{ original, renamed, edits, sites }`; `edited` holds
 * every renamed file's, so a diagnostic anywhere maps back; `writes` and
 * `moves` are the whole rename's, so the copy is the renamed tree exactly.
 */
export function checkRust({ root, files, edited, writes, moves, cache }) {
  const verdicts = new Map();
  if (files.size === 0) return verdicts;
  const scratch = cache ?? path.join(tmpdir(), `runx-convert-rust-${createHash("sha256").update(path.resolve(root)).digest("hex").slice(0, 12)}`);
  const byWorkspace = new Map();
  for (const file of files.keys()) {
    const workspace = workspaceOf(root, file);
    if (workspace === undefined) verdicts.set(file, { unread: "no Cargo manifest holds it" });
    else byWorkspace.set(workspace, [...(byWorkspace.get(workspace) ?? []), file]);
  }
  if (byWorkspace.size === 0) return verdicts;

  // The renamed tree: every file the repo holds, renamed and moved as the rename would.
  const tree = path.join(scratch, "renamed");
  rmSync(tree, { recursive: true, force: true });
  const listed = execFileSync("git", ["-C", root, "ls-files", "--cached", "--others", "--exclude-standard"], { encoding: "utf8", maxBuffer: 1 << 28 })
    .split("\n")
    .filter((file) => file && existsSync(path.join(root, file)));
  const written = new Map(writes);
  for (const file of listed) {
    const to = path.join(tree, moves.get(file) ?? file);
    mkdirSync(path.dirname(to), { recursive: true });
    if (written.has(file)) writeFileSync(to, written.get(file));
    else copyFileSync(path.join(root, file), to);
  }
  const back = new Map([...moves].map(([from, to]) => [to, from]));

  for (const [workspace, members] of byWorkspace) {
    const runs = {};
    for (const [side, cwd] of [["base", path.join(root, workspace)], ["renamed", path.join(tree, moves.get(path.join(workspace, "Cargo.toml"))?.replace(/\/Cargo\.toml$/, "") ?? workspace)]]) {
      const target = path.join(scratch, `target-${side}`);
      const check = cargo("check", cwd, target, []);
      const clippy = cargo("clippy", cwd, target, ["--", ...SHADOWS.flatMap((lint) => ["-W", lint])]);
      runs[side] = { cwd, messages: [...check.messages, ...clippy.messages], compiled: compiled(target, cwd), failed: side === "base" && (check.status !== 0 || clippy.status !== 0) };
    }
    if (runs.base.failed) {
      for (const file of members) verdicts.set(file, { unread: "the original tree doesn't compile, so nothing compares against it" });
      continue;
    }
    // A diagnostic, by its code and where its primary span starts in the original.
    const original = (file) => edited.get(file)?.original ?? readFileSync(path.join(root, file), "utf8");
    const locate = (side, span) => {
      const where = path.relative(root, path.resolve(runs[side].cwd, span.file_name));
      const file = side === "base" ? where : back.get(path.relative(tree, path.resolve(runs[side].cwd, span.file_name))) ?? path.relative(tree, path.resolve(runs[side].cwd, span.file_name));
      const entry = edited.get(file);
      const text = side === "base" || !entry ? original(file) : entry.renamed;
      const start = indexOfByte(text, span.byte_start);
      const end = indexOfByte(text, span.byte_end);
      if (side === "base" || !entry) return { file, start, end };
      const map = mapping(entry.edits);
      return { file, start: map.toOriginal(start), end: map.toOriginal(end) };
    };
    const relevant = (message) => message.level === "error" || SHADOWS.includes(message.code?.code);
    const keyOf = (side, message) => {
      const primary = (message.spans ?? []).find((span) => span.is_primary) ?? message.spans?.[0];
      if (!primary) return undefined;
      const at = locate(side, primary);
      return `${message.code?.code ?? message.level}|${at.file}|${at.start}`;
    };
    const before = new Set(runs.base.messages.filter(relevant).map((message) => keyOf("base", message)));
    const seen = new Set();
    for (const message of runs.renamed.messages.filter(relevant)) {
      const key = keyOf("renamed", message);
      if (!key || before.has(key) || seen.has(key)) continue;
      seen.add(key);
      const reason = `${message.code?.code ?? message.level}: ${message.message}`;
      const spans = spansOf(message).map((span) => locate("renamed", span));
      let hit = false;
      for (const span of spans) {
        for (const site of files.get(span.file)?.sites ?? []) {
          if (site.at < span.start || site.at >= Math.max(span.end, span.start + 1)) continue;
          const verdict = verdicts.get(span.file) ?? { refused: [] };
          verdict.refused.push({ site, reason });
          verdicts.set(span.file, verdict);
          hit = true;
        }
      }
      if (!hit) {
        const at = spans[0];
        const verdict = verdicts.get(at.file) ?? { refused: [] };
        verdict.refused?.push({ site: { at: at.start, token: "", next: "" }, reason, unattributed: true });
        verdicts.set(at.file, verdict);
      }
    }
    // A file no target compiled was never checked.
    for (const file of members) {
      const compiledAs = path.join(tree, moves.get(file) ?? file);
      if (!runs.renamed.compiled.has(compiledAs)) verdicts.set(file, { unread: "no cargo target compiles it" });
      else if (!verdicts.has(file)) verdicts.set(file, { refused: [] });
    }
  }
  return verdicts;
}
