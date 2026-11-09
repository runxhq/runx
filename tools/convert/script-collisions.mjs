// Which renamed bare words collide in TypeScript, JavaScript and Astro, read
// from the TypeScript AST and its binder (TypeScript 7's API). A renamed site
// collides when it makes a duplicate binding in one scope, a declaration that
// shadows a binding of the new name or is shadowed by one, a duplicate key in
// one object literal, type, interface, enum or class, or a reference that now
// resolves to a different binding. Whatever else its file holds under the new
// name, a string, a comment, a member, a key elsewhere or another scope's
// binding, is no collision. A file TypeScript can't parse is left to the
// file-level refusal.

import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import path from "node:path";

export const SCRIPT = /\.(ts|tsx|mts|cts|js|jsx|mjs|cjs|astro)$/;

const require = createRequire(import.meta.url);

function typescript() {
  try {
    return { API: require("typescript/unstable/sync").API, ast: require("typescript/unstable/ast") };
  } catch (error) {
    throw new Error(`the TypeScript check needs the workspace's typescript 7 and its unstable API (${error.message})`);
  }
}

/** Offsets between a source and its renamed text, from the renamer's edits. */
export function mapping(edits) {
  const marks = [];
  let delta = 0;
  for (const edit of [...edits].sort((a, b) => a.at - b.at)) {
    marks.push({ at: edit.at, renamed: edit.at + delta, from: edit.from.length, to: edit.to.length });
    delta += edit.to.length - edit.from.length;
  }
  const across = (pos, here, there, hereLength, thereLength) => {
    let shift = 0;
    for (const mark of marks) {
      if (pos < mark[here]) break;
      if (pos < mark[here] + mark[hereLength]) return mark[there] + Math.min(pos - mark[here], mark[thereLength]);
      shift = mark[there] + mark[thereLength] - (mark[here] + mark[hereLength]);
    }
    return pos + shift;
  };
  return {
    toRenamed: (pos) => across(pos, "at", "renamed", "from", "to"),
    toOriginal: (pos) => across(pos, "renamed", "at", "to", "from"),
  };
}

// ---- Astro: the frontmatter and the template's expressions are one module,
// as Astro compiles them, and each <script> is its own.

/** The end of a JavaScript string, template or comment starting at `i`, or -1. */
function skipLiteral(text, i) {
  const c = text[i];
  if (c === "'" || c === '"') {
    for (let j = i + 1; j < text.length; j += 1) {
      if (text[j] === "\\") j += 1;
      else if (text[j] === c || text[j] === "\n") return j + 1;
    }
    return text.length;
  }
  if (c === "`") {
    for (let j = i + 1; j < text.length; j += 1) {
      if (text[j] === "\\") j += 1;
      else if (text[j] === "`") return j + 1;
      else if (text[j] === "$" && text[j + 1] === "{") j = expressionEnd(text, j + 1) - 1;
    }
    return text.length;
  }
  if (c === "/" && text[i + 1] === "/") {
    const end = text.indexOf("\n", i);
    return end < 0 ? text.length : end;
  }
  if (c === "/" && text[i + 1] === "*") {
    const end = text.indexOf("*/", i + 2);
    return end < 0 ? text.length : end + 2;
  }
  return -1;
}

const VOID = new Set(["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"]);

/** The end of a JSX element starting at `i`: its tag, attributes and children. */
function elementEnd(text, i) {
  let j = i + 1;
  const name = /^[A-Za-z][\w.:-]*/.exec(text.slice(j))?.[0] ?? "";
  j += name.length;
  while (j < text.length) {
    if (text[j] === "{") j = expressionEnd(text, j);
    else if (text[j] === '"' || text[j] === "'") j = skipLiteral(text, j);
    else if (text.startsWith("/>", j)) return j + 2;
    else if (text[j] === ">") break;
    else j += 1;
  }
  j += 1;
  if (VOID.has(name.toLowerCase())) return j;
  const close = name === "" ? "</>" : `</${name}`;
  while (j < text.length) {
    if (text.startsWith(close, j) && (name === "" || !/[\w.:-]/.test(text[j + close.length] ?? ""))) {
      const end = text.indexOf(">", j);
      return end < 0 ? text.length : end + 1;
    }
    if (text[j] === "{") j = expressionEnd(text, j);
    else if (text[j] === "<" && /[A-Za-z]/.test(text[j + 1] ?? "")) j = elementEnd(text, j);
    else j += 1;
  }
  return text.length;
}

/** The end of a `{...}` expression starting at `i`, past its closing brace. */
function expressionEnd(text, i) {
  let depth = 0;
  for (let j = i; j < text.length; ) {
    const literal = skipLiteral(text, j);
    if (literal >= 0) {
      j = literal;
      continue;
    }
    const c = text[j];
    if (c === "<" && /[A-Za-z>]/.test(text[j + 1] ?? "") && /[(,=?:&|!{}[\]\s>]$|^$/.test(text.slice(Math.max(i, j - 1), j).replace(/\s+$/, " "))) {
      j = elementEnd(text, j);
      continue;
    }
    if (c === "{") depth += 1;
    if (c === "}") {
      depth -= 1;
      if (depth === 0) return j + 1;
    }
    j += 1;
  }
  return text.length;
}

/**
 * The units TypeScript reads in a source: the file itself, or an Astro file's
 * module and scripts, each with the map between its text and the source's.
 */
export function units(file, text) {
  const ext = path.extname(file);
  if (ext !== ".astro") return [{ ext, text, segments: [{ unit: 0, source: 0, length: text.length }] }];
  const fence = /^\s*---\r?\n/.exec(text);
  const close = fence ? text.indexOf("\n---", fence[0].length - 1) : -1;
  const module = { ext: ".tsx", text: "", segments: [] };
  const scripts = [];
  const take = (unit, from, to, before = "", after = "") => {
    unit.text += before;
    unit.segments.push({ unit: unit.text.length, source: from, length: to - from });
    unit.text += text.slice(from, to) + after;
  };
  let at = 0;
  if (fence && close >= 0) {
    take(module, fence[0].length, close + 1);
    at = text.indexOf("\n", close + 1) + 1 || text.length;
  }
  while (at < text.length) {
    if (text.startsWith("<!--", at)) {
      const end = text.indexOf("-->", at);
      at = end < 0 ? text.length : end + 3;
    } else if (/^<(style|script)\b/i.test(text.slice(at, at + 8))) {
      const tag = /^<(style|script)\b/i.exec(text.slice(at, at + 8))[1].toLowerCase();
      const open = text.indexOf(">", at);
      const end = text.toLowerCase().indexOf(`</${tag}`, open);
      if (open < 0 || end < 0) break;
      // A script is JavaScript unless its type says otherwise, as JSON-LD does.
      const type = /\btype\s*=\s*["']([^"']*)["']/i.exec(text.slice(at, open))?.[1];
      if (tag === "script" && (!type || /module|javascript|typescript/i.test(type))) {
        const script = { ext: ".ts", text: "", segments: [] };
        take(script, open + 1, end);
        scripts.push(script);
      }
      at = end + 3 + tag.length;
    } else if (text[at] === "{") {
      const end = expressionEnd(text, at);
      const inside = text.slice(at + 1, end - 1);
      // An expression is a statement of the module; a spread attribute is a
      // spread, and an expression holding only a comment is none.
      if (inside.replace(/\/\*[\s\S]*?\*\/|\/\/[^\n]*/g, "").trim() !== "") {
        const spread = inside.trimStart().startsWith("...");
        take(module, at + 1, end - 1, spread ? ";[" : ";(", spread ? "];\n" : ");\n");
      }
      at = end;
    } else at += 1;
  }
  return [module, ...scripts];
}

function locate(unit, pos, side) {
  for (const segment of unit.segments) {
    const [from, to] = side === "source" ? [segment.unit, segment.source] : [segment.source, segment.unit];
    if (pos >= from && pos <= from + segment.length) return to + (pos - from);
  }
  return undefined;
}

// ---- What a node is

function kinds(ast) {
  const K = ast.SyntaxKind;
  return {
    K,
    FUNCTIONS: new Set([K.FunctionDeclaration, K.FunctionExpression, K.ArrowFunction, K.MethodDeclaration, K.Constructor, K.GetAccessor, K.SetAccessor, K.ClassStaticBlockDeclaration]),
    SIGNATURES: new Set([K.FunctionType, K.ConstructorType, K.CallSignature, K.ConstructSignature, K.MethodSignature, K.IndexSignature]),
    BLOCKS: new Set([K.Block, K.CaseBlock, K.SourceFile, K.ModuleBlock, K.ForStatement, K.ForInStatement, K.ForOfStatement, K.CatchClause]),
    MEMBERS: new Set([K.PropertyAssignment, K.ShorthandPropertyAssignment, K.PropertySignature, K.PropertyDeclaration, K.MethodDeclaration, K.MethodSignature, K.GetAccessor, K.SetAccessor, K.EnumMember, K.JsxAttribute]),
    CONTAINERS: new Set([K.ObjectLiteralExpression, K.TypeLiteral, K.InterfaceDeclaration, K.ClassDeclaration, K.ClassExpression, K.EnumDeclaration, K.JsxAttributes]),
  };
}

// An identifier's text is its token, so it starts its length before its end.
const startOf = (node) => node.end - node.text.length;

function nearest(node, set, K, stopAtFunctions) {
  for (let at = node.parent; at; at = at.parent) {
    if (set.has(at.kind)) return at;
    if (stopAtFunctions?.has(at.kind)) return at;
    if (at.kind === K.SourceFile) return at;
  }
  return undefined;
}

/** Every binding a unit declares: its name, start, scope and namespaces. */
function declarations(sf, ast) {
  const { K, FUNCTIONS, SIGNATURES, BLOCKS } = kinds(ast);
  const out = [];
  const VALUE = 1;
  const TYPE = 2;
  const blockScope = (node) => nearest(node, BLOCKS, K, FUNCTIONS);
  const functionScope = (node) => nearest(node, new Set([K.SourceFile, K.ModuleBlock]), K, FUNCTIONS);
  const bind = (name, scope, namespaces, signatureOnly = false) => {
    if (!name) return;
    if (name.kind === K.Identifier) out.push({ name: name.text, start: startOf(name), node: name, scope, namespaces, signatureOnly });
    else if (name.kind === K.ObjectBindingPattern || name.kind === K.ArrayBindingPattern) {
      for (const element of name.elements ?? []) if (element.kind === K.BindingElement) bind(element.name, scope, namespaces, signatureOnly);
    }
  };
  const visit = (node) => {
    switch (node.kind) {
      case K.VariableDeclaration: {
        const list = node.parent;
        if (list?.parent?.kind === K.CatchClause) bind(node.name, list.parent, VALUE);
        else if (list?.kind === K.VariableDeclarationList) bind(node.name, list.flags & ast.NodeFlags.BlockScoped ? blockScope(list) : functionScope(list), VALUE);
        break;
      }
      case K.Parameter: {
        const owner = node.parent;
        const signatureOnly = SIGNATURES.has(owner?.kind) || (FUNCTIONS.has(owner?.kind) && owner.body === undefined);
        bind(node.name, owner, VALUE, signatureOnly);
        break;
      }
      case K.FunctionDeclaration:
        bind(node.name, blockScope(node), VALUE, node.body === undefined);
        break;
      case K.ClassDeclaration:
      case K.EnumDeclaration:
      case K.ModuleDeclaration:
        bind(node.name, blockScope(node), VALUE | TYPE);
        break;
      case K.FunctionExpression:
      case K.ClassExpression:
        bind(node.name, node, VALUE);
        break;
      case K.InterfaceDeclaration:
      case K.TypeAliasDeclaration:
        bind(node.name, blockScope(node), TYPE);
        break;
      case K.TypeParameter:
        bind(node.name, node.parent, TYPE);
        break;
      case K.ImportClause:
      case K.NamespaceImport:
      case K.ImportSpecifier:
      case K.ImportEqualsDeclaration:
        bind(node.name, sf, VALUE | TYPE);
        break;
      default:
    }
    node.forEachChild(visit);
  };
  visit(sf);
  return out;
}

/** Whether an identifier is resolved through scopes, not a member, key or label. */
function isReference(node, ast) {
  const { K } = kinds(ast);
  const parent = node.parent;
  if (!parent) return false;
  switch (parent.kind) {
    case K.PropertyAccessExpression:
      return parent.expression === node;
    case K.QualifiedName:
      return parent.left === node;
    case K.PropertyAssignment:
    case K.PropertySignature:
    case K.PropertyDeclaration:
    case K.MethodDeclaration:
    case K.MethodSignature:
    case K.GetAccessor:
    case K.SetAccessor:
    case K.EnumMember:
    case K.JsxAttribute:
    case K.NamedTupleMember:
      return parent.name !== node;
    case K.BindingElement:
    case K.ImportSpecifier:
      return parent.propertyName !== node;
    case K.ExportSpecifier:
      return parent.propertyName ? parent.propertyName === node : true;
    case K.LabeledStatement:
    case K.BreakStatement:
    case K.ContinueStatement:
    case K.MetaProperty:
      return false;
    case K.JsxOpeningElement:
    case K.JsxSelfClosingElement:
    case K.JsxClosingElement:
      return parent.tagName !== node || /^[A-Z]/.test(node.text);
    default:
      return true;
  }
}

const encloses = (outer, inner) => {
  for (let at = inner?.parent; at; at = at.parent) if (at === outer) return true;
  return false;
};

const lineOf = (text, pos) => text.slice(0, pos).split("\n").length;

/**
 * The renamed sites that collide, by file. Each file is
 * `{ file, original, renamed, edits, sites: [{ at, token, next }] }`, `at` an
 * offset in `original`; each verdict names its site and reason. A file the
 * check can't read is `{ unread: reason }`, for the file-level refusal.
 */
export function checkScripts(files) {
  if (files.length === 0) return new Map();
  const { API, ast } = typescript();
  const { K, MEMBERS, CONTAINERS } = kinds(ast);
  const dir = mkdtempSync(path.join(tmpdir(), "runx-script-collisions-"));
  const verdicts = new Map();
  let api;
  try {
    const plan = [];
    const names = [];
    files.forEach((entry, index) => {
      const before = units(entry.file, entry.original);
      const after = units(entry.file, entry.renamed);
      if (before.length !== after.length || before.some((unit, u) => unit.segments.length !== after[u].segments.length)) {
        verdicts.set(entry.file, { unread: "its renamed text splits into different parts" });
        return;
      }
      const pair = before.map((unit, u) => {
        const o = path.join(dir, "o", `${index}-${u}${unit.ext}`);
        const r = path.join(dir, "r", `${index}-${u}${unit.ext}`);
        mkdirSync(path.dirname(o), { recursive: true });
        mkdirSync(path.dirname(r), { recursive: true });
        writeFileSync(o, unit.text);
        writeFileSync(r, after[u].text);
        names.push(o, r);
        return { o, r, before: unit, after: after[u] };
      });
      plan.push({ entry, pair, map: mapping(entry.edits) });
    });
    const config = path.join(dir, "tsconfig.json");
    writeFileSync(
      config,
      JSON.stringify({
        compilerOptions: { noLib: true, types: [], allowJs: true, checkJs: false, jsx: "preserve", noEmit: true, noResolve: true, moduleDetection: "force", target: "esnext", module: "esnext" },
        files: names,
      }),
    );
    api = new API({ cwd: dir });
    const project = api.updateSnapshot({ openProjects: [config] }).getProject(config);
    const { program, checker } = project;

    for (const { entry, pair, map } of plan) {
      const refused = [];
      const next = new Set(entry.sites.map((site) => site.next));
      let unread;
      for (const { o, r, before, after } of pair) {
        const sfO = program.getSourceFile(o);
        const sfR = program.getSourceFile(r);
        if (!sfO || !sfR || program.getSyntacticDiagnostics(o).length > 0) {
          unread = "TypeScript doesn't parse it";
          break;
        }
        // Positions: unit O <-> source O <-> source R <-> unit R.
        const rToO = (pos) => {
          const source = locate(after, pos, "source");
          return source === undefined ? undefined : locate(before, map.toOriginal(source), "unit");
        };
        const toR = (at) => {
          const unit = locate(before, at, "unit");
          return unit === undefined ? undefined : locate(after, map.toRenamed(at), "unit");
        };
        const sourceLine = (oPos) => lineOf(entry.original, locate(before, oPos, "source") ?? 0);
        const sites = entry.sites.map((site) => ({ site, r: toR(site.at) })).filter((one) => one.r !== undefined);
        if (sites.length === 0) continue;
        if (program.getSyntacticDiagnostics(r).length > 0) {
          for (const { site } of sites) refused.push({ site, reason: "the renamed file doesn't parse" });
          continue;
        }
        const siteAt = new Map(sites.map((one) => [one.r, one.site]));
        // The name a token had in the original: an identifier, or a string's text.
        const tokenO = (oPos) => {
          const token = oPos === undefined ? undefined : ast.getTouchingToken(sfO, oPos);
          if (!token) return undefined;
          if (token.kind === K.Identifier) return startOf(token) === oPos ? token.text : undefined;
          return ast.getTokenPosOfNode(token, sfO) === oPos ? token.text : undefined;
        };
        const nameStart = (name, sf) => (name.kind === K.Identifier ? startOf(name) : ast.getTokenPosOfNode(name, sf));

        // A binder diagnostic the original didn't have: a duplicate binding.
        // The binder reports each of the two declarations, so the renamed one
        // carries the refusal, and one with no renamed site is unattributed.
        const seen = new Set(program.getBindDiagnostics(o).map((d) => `${d.code}@${d.start ?? d.pos}`));
        const fresh = program.getBindDiagnostics(r).filter((d) => !seen.has(`${d.code}@${rToO(d.start ?? d.pos)}`));
        const message = (d) => `TS${d.code}: ${d.text ?? d.messageText}`;
        const attributed = new Set();
        for (const diagnostic of fresh) {
          const site = siteAt.get(diagnostic.start ?? diagnostic.pos);
          if (!site) continue;
          refused.push({ site, reason: `a second binding in one scope (${message(diagnostic)})` });
          attributed.add(message(diagnostic));
        }
        for (const diagnostic of fresh) {
          const at = diagnostic.start ?? diagnostic.pos;
          if (siteAt.has(at) || attributed.has(message(diagnostic))) continue;
          refused.push({ site: { at: locate(before, rToO(at) ?? 0, "source") ?? 0, token: "", next: "" }, reason: message(diagnostic), unattributed: true });
        }

        // A duplicate key in one object literal, type, interface, enum or class:
        // a name, or a string that is the whole key.
        for (const { site, r: pos } of sites) {
          const token = ast.getTouchingToken(sfR, pos);
          if (!token) continue;
          if (token.kind === K.Identifier ? startOf(token) !== pos : token.kind !== K.StringLiteral || ast.getTokenPosOfNode(token, sfR) + 1 !== pos || token.text !== site.next) continue;
          const member = token.parent;
          if (!MEMBERS.has(member?.kind) || member.name !== token || !CONTAINERS.has(member.parent?.kind)) continue;
          const siblings = member.parent.kind === K.EnumDeclaration ? member.parent.members : member.parent.kind === K.JsxAttributes ? member.parent.properties : (member.parent.properties ?? member.parent.members ?? []);
          for (const sibling of siblings) {
            const name = sibling.name;
            if (sibling === member || !name || (name.kind !== K.Identifier && name.kind !== K.StringLiteral) || name.text !== token.text) continue;
            const at = rToO(nameStart(name, sfR));
            if (tokenO(at) !== site.token) refused.push({ site, reason: `a second ${token.text} key in one ${ast.SyntaxKind[member.parent.kind]}, beside line ${sourceLine(at)}'s` });
          }
        }

        // A declaration that shadows a binding of the new name, or is shadowed by one.
        const declsR = declarations(sfR, ast).filter((decl) => next.has(decl.name));
        for (const { site, r: pos } of sites) {
          const decl = declsR.find((one) => one.start === pos);
          if (!decl || decl.signatureOnly) continue;
          for (const other of declsR) {
            if (other === decl || other.signatureOnly || other.name !== decl.name || (other.namespaces & decl.namespaces) === 0) continue;
            const inner = encloses(other.scope, decl.node) && other.scope !== decl.scope ? "shadows" : encloses(decl.scope, other.node) && other.scope !== decl.scope ? "is shadowed by" : undefined;
            if (!inner) continue;
            if (tokenO(rToO(other.start)) === site.token) continue;
            refused.push({ site, reason: `${inner} the binding of ${decl.name} at line ${sourceLine(rToO(other.start))}` });
          }
        }

        // A reference that resolves to a different binding than it did.
        const refsR = [];
        const visit = (node) => {
          if (node.kind === K.Identifier && next.has(node.text) && isReference(node, ast)) refsR.push(node);
          node.forEachChild(visit);
        };
        visit(sfR);
        const refsO = refsR.map((node) => {
          const oPos = rToO(startOf(node));
          const token = oPos === undefined ? undefined : ast.getTouchingToken(sfO, oPos);
          return token && token.kind === K.Identifier && startOf(token) === oPos ? token : undefined;
        });
        if (refsO.some((node) => node === undefined)) {
          unread = "its renamed identifiers don't line up with its own";
          break;
        }
        const symbolsR = refsR.length ? checker.getSymbolAtLocation(refsR) : [];
        const symbolsO = refsO.length ? checker.getSymbolAtLocation(refsO) : [];
        const identity = (symbol, name, toO) => {
          if (!symbol) return { key: `unresolved ${name}`, at: [] };
          const at = (symbol.declarations ?? []).map((handle) => {
            const node = handle.resolve();
            const start = node?.name ? startOf(node.name) : node ? node.pos : undefined;
            return start === undefined ? undefined : toO(start);
          });
          return { key: at.map((pos) => `${pos}`).sort().join(","), at };
        };
        const where = (list) => {
          const lines = [...new Set(list.map((p) => sourceLine(p)))].sort((a, b) => a - b);
          if (lines.length === 0) return "no binding in the file";
          return lines.length === 1 ? `line ${lines[0]}'s binding` : `the bindings at lines ${lines.slice(0, -1).join(", ")} and ${lines.at(-1)}`;
        };
        // A renamed declaration that captures references it didn't have: one
        // reason per site, naming each line it captures.
        const captured = new Map();
        refsR.forEach((node, i) => {
          const now = identity(symbolsR[i], node.text, rToO);
          const was = identity(symbolsO[i], refsO[i].text, (pos) => pos);
          if (now.key === was.key) return;
          const pos = startOf(node);
          const site = siteAt.get(pos);
          if (site) {
            refused.push({ site, reason: `now resolves to ${where(now.at)}, not ${where(was.at)}` });
            return;
          }
          const culprits = (symbolsR[i]?.declarations ?? []).map((handle) => handle.resolve()).map((decl) => (decl?.name ? siteAt.get(startOf(decl.name)) : undefined)).filter(Boolean);
          for (const culprit of culprits) captured.set(culprit, [...(captured.get(culprit) ?? []), sourceLine(rToO(pos))]);
          if (culprits.length === 0) refused.push({ site: { at: locate(before, rToO(pos), "source") ?? 0, token: refsO[i].text, next: refsO[i].text }, reason: `${node.text} now resolves to ${where(now.at)}, not ${where(was.at)}`, unattributed: true });
        });
        for (const [site, lines] of captured) refused.push({ site, reason: `captures the ${site.next} at line${lines.length > 1 ? "s" : ""} ${[...new Set(lines)].join(", ")}, which resolved elsewhere` });
      }
      verdicts.set(entry.file, unread ? { unread } : { refused });
    }
  } finally {
    try {
      api?.close();
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }
  return verdicts;
}
