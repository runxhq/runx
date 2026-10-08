import { existsSync, readFileSync, readdirSync, realpathSync, statSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";

import {
  defineTool,
  failure,
  isRecord,
} from "@runxhq/extension-sdk";

function parseDocsInputs(value) {
  if (isRecord(value)) {
    return value;
  }
  if (typeof value === "string") {
    try {
      const parsed = JSON.parse(value);
      if (isRecord(parsed)) {
        return parsed;
      }
    } catch {
      return { mode: "config", description: value };
    }
  }
  return { mode: "config" };
}

function sourceyEnv(env) {
  const cleanEnv = { ...env };
  delete cleanEnv.RUNX_INPUTS_JSON;
  for (const key of Object.keys(cleanEnv)) {
    if (key.startsWith("RUNX_INPUT_")) {
      delete cleanEnv[key];
    }
  }
  return cleanEnv;
}

function collectGeneratedFiles(rootDir, currentDir = rootDir, files = [], maxFiles = 64) {
  if (!existsSync(currentDir) || files.length >= maxFiles) {
    return files;
  }

  const entries = readdirSync(currentDir, { withFileTypes: true })
    .sort((left, right) => left.name.localeCompare(right.name));

  for (const entry of entries) {
    if (files.length >= maxFiles) {
      break;
    }

    const absolutePath = path.join(currentDir, entry.name);
    if (entry.isDirectory()) {
      collectGeneratedFiles(rootDir, absolutePath, files, maxFiles);
      continue;
    }

    if (entry.isFile()) {
      files.push(path.relative(rootDir, absolutePath));
    }
  }

  return files;
}

function isDirectory(candidate) {
  try {
    return existsSync(candidate) && statSync(candidate).isDirectory();
  } catch {
    return false;
  }
}

function walkUp(startDir) {
  const dirs = [];
  let current = path.resolve(startDir);
  while (true) {
    dirs.push(current);
    const parent = path.dirname(current);
    if (parent === current) {
      return dirs;
    }
    current = parent;
  }
}

function findHeroiconsOutlineDir(project, sourceyBin) {
  const candidates = [];
  candidates.push(path.join(project, "node_modules", "heroicons", "24", "outline"));

  if (sourceyBin && (sourceyBin.includes(path.sep) || sourceyBin.startsWith("."))) {
    const resolvedSourcey = path.resolve(project, sourceyBin);
    const sourceyPath = existsSync(resolvedSourcey) ? realpathSync(resolvedSourcey) : resolvedSourcey;
    for (const dir of walkUp(path.dirname(sourceyPath))) {
      candidates.push(path.join(dir, "node_modules", "heroicons", "24", "outline"));
    }
  }

  for (const candidate of candidates) {
    if (isDirectory(candidate)) {
      return candidate;
    }
  }
  return undefined;
}

function loadHeroiconNames(outlineDir) {
  return new Set(
    readdirSync(outlineDir)
      .filter((file) => file.endsWith(".svg"))
      .map((file) => path.basename(file, ".svg")),
  );
}

function collectMarkdownFiles(rootDir, currentDir = rootDir, files = [], maxFiles = 512) {
  if (!isDirectory(currentDir) || files.length >= maxFiles) {
    return files;
  }

  const ignoredDirs = new Set([".git", ".sourcey", "dist", "build", "node_modules"]);
  const entries = readdirSync(currentDir, { withFileTypes: true })
    .sort((left, right) => left.name.localeCompare(right.name));

  for (const entry of entries) {
    if (files.length >= maxFiles) {
      break;
    }

    const absolutePath = path.join(currentDir, entry.name);
    if (entry.isDirectory()) {
      if (!ignoredDirs.has(entry.name)) {
        collectMarkdownFiles(rootDir, absolutePath, files, maxFiles);
      }
      continue;
    }

    if (entry.isFile() && /\.(md|mdx)$/i.test(entry.name)) {
      files.push(absolutePath);
    }
  }

  return files;
}

function extractCardIconReferences(filePath, sourceRoot) {
  const references = [];
  const lines = readFileSync(filePath, "utf8").split(/\r?\n/);
  const cardLinePattern = /(:{2,}card(?=[\s{])|<Card\b)/;
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (!cardLinePattern.test(line)) {
      continue;
    }
    const iconPattern = /\bicon\s*=\s*(?:"([^"]+)"|'([^']+)'|\{\s*["']([^"']+)["']\s*\})/g;
    let match = iconPattern.exec(line);
    while (match) {
      const icon = match[1] || match[2] || match[3] || "";
      if (icon.trim()) {
        references.push({
          icon: icon.trim(),
          path: path.relative(sourceRoot, filePath),
          line: index + 1,
          column: match.index + 1,
        });
      }
      match = iconPattern.exec(line);
    }
  }
  return references;
}

function editDistance(left, right) {
  const rows = Array.from({ length: left.length + 1 }, () => Array(right.length + 1).fill(0));
  for (let index = 0; index <= left.length; index += 1) {
    rows[index][0] = index;
  }
  for (let index = 0; index <= right.length; index += 1) {
    rows[0][index] = index;
  }
  for (let row = 1; row <= left.length; row += 1) {
    for (let col = 1; col <= right.length; col += 1) {
      const cost = left[row - 1] === right[col - 1] ? 0 : 1;
      rows[row][col] = Math.min(
        rows[row - 1][col] + 1,
        rows[row][col - 1] + 1,
        rows[row - 1][col - 1] + cost,
      );
    }
  }
  return rows[left.length][right.length];
}

function closestHeroiconName(icon, validNames) {
  const prefixMatch = [...validNames]
    .filter((name) => name.startsWith(`${icon}-`) || name.startsWith(icon))
    .sort((left, right) => left.length - right.length || left.localeCompare(right))[0];
  if (prefixMatch) {
    return prefixMatch;
  }

  const maxDistance = Math.max(3, Math.ceil(icon.length * 0.3));
  let best;
  for (const name of validNames) {
    const distance = editDistance(icon, name);
    if (distance <= maxDistance && (!best || distance < best.distance || (distance === best.distance && name < best.name))) {
      best = { name, distance };
    }
  }
  return best?.name;
}

function validateCardIcons(project, sourceRoot, sourceyBin) {
  if (!sourceRoot || !isDirectory(sourceRoot)) {
    return {
      checked: false,
      status: "skipped",
      icon_count: 0,
      invalid_count: 0,
      invalid_icons: [],
      reason: "No Sourcey markdown source directory was available for card icon validation.",
    };
  }

  const iconReferences = collectMarkdownFiles(sourceRoot)
    .flatMap((filePath) => extractCardIconReferences(filePath, sourceRoot));
  const outlineDir = findHeroiconsOutlineDir(project, sourceyBin);
  if (!outlineDir) {
    return {
      checked: false,
      status: "skipped",
      icon_count: iconReferences.length,
      invalid_count: 0,
      invalid_icons: [],
      reason: "Could not locate heroicons/24/outline for Sourcey card icon validation.",
    };
  }

  const validNames = loadHeroiconNames(outlineDir);
  const invalidIcons = iconReferences
    .filter((reference) => !validNames.has(reference.icon))
    .map((reference) => ({
      ...reference,
      suggestion: closestHeroiconName(reference.icon, validNames),
    }));

  return {
    checked: true,
    status: invalidIcons.length > 0 ? "invalid" : "valid",
    icon_count: iconReferences.length,
    invalid_count: invalidIcons.length,
    invalid_icons: invalidIcons.slice(0, 25),
    registry_source: outlineDir,
  };
}

function decodeHtmlEntities(text) {
  return text
    .replace(/&nbsp;/gi, " ")
    .replace(/&amp;/gi, "&")
    .replace(/&lt;/gi, "<")
    .replace(/&gt;/gi, ">")
    .replace(/&quot;/gi, '"')
    .replace(/&#39;/gi, "'");
}

function stripHtml(html) {
  return decodeHtmlEntities(
    html
      .replace(/<script\b[^>]*>[\s\S]*?<\/script>/gi, " ")
      .replace(/<style\b[^>]*>[\s\S]*?<\/style>/gi, " ")
      .replace(/<[^>]+>/g, " "),
  )
    .replace(/\s+/g, " ")
    .trim();
}

function extractTagText(html, tagName) {
  const match = html.match(new RegExp(`<${tagName}\\b[^>]*>([\\s\\S]*?)<\\/${tagName}>`, "i"));
  if (!match) {
    return undefined;
  }
  const text = stripHtml(match[1]);
  return text || undefined;
}

function extractHeadingTexts(html, maxHeadings = 4) {
  const headings = [];
  const matcher = /<h[1-3]\b[^>]*>([\s\S]*?)<\/h[1-3]>/gi;
  let match = matcher.exec(html);
  while (match && headings.length < maxHeadings) {
    const text = stripHtml(match[1]);
    if (text) {
      headings.push(text);
    }
    match = matcher.exec(html);
  }
  return headings;
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function extractBalancedElement(html, startIndex, tagName) {
  const tagPattern = new RegExp(`</?${escapeRegExp(tagName)}\\b[^>]*>`, "gi");
  tagPattern.lastIndex = startIndex;
  let depth = 0;
  let match = tagPattern.exec(html);
  while (match) {
    const token = match[0];
    const isClosing = token.startsWith("</");
    const isSelfClosing = /\/\s*>$/.test(token);
    if (!isClosing && !isSelfClosing) {
      depth += 1;
    } else if (isClosing) {
      depth -= 1;
    }
    if (depth === 0) {
      return html.slice(startIndex, tagPattern.lastIndex);
    }
    match = tagPattern.exec(html);
  }
  return undefined;
}

function extractElementById(html, id) {
  const matcher = new RegExp(`<([a-z0-9-]+)\\b[^>]*\\bid\\s*=\\s*(["'])${escapeRegExp(id)}\\2[^>]*>`, "i");
  const match = matcher.exec(html);
  if (!match || match.index === undefined) {
    return undefined;
  }
  return extractBalancedElement(html, match.index, match[1]);
}

function extractFirstElement(html, tagName) {
  const matcher = new RegExp(`<${escapeRegExp(tagName)}\\b[^>]*>`, "i");
  const match = matcher.exec(html);
  if (!match || match.index === undefined) {
    return undefined;
  }
  return extractBalancedElement(html, match.index, tagName);
}

function removeHtmlSubtrees(html, tagNames) {
  let output = html;
  for (const tagName of tagNames) {
    const pattern = new RegExp(`<${escapeRegExp(tagName)}\\b[^>]*>[\\s\\S]*?<\\/${escapeRegExp(tagName)}>`, "gi");
    output = output.replace(pattern, " ");
  }
  return output;
}

function extractRenderedContentHtml(html) {
  const content =
    extractElementById(html, "content-area") ??
    extractFirstElement(html, "main") ??
    extractFirstElement(html, "body") ??
    html;
  return removeHtmlSubtrees(content, ["nav", "aside", "script", "style", "noscript"]);
}

function buildIndexEvidence(indexPath) {
  if (!existsSync(indexPath)) {
    return {};
  }

  const stats = statSync(indexPath);
  if (!stats.isFile()) {
    return {};
  }

  const html = readFileSync(indexPath, "utf8");
  const contentHtml = extractRenderedContentHtml(html);
  const excerpt = stripHtml(contentHtml).slice(0, 1200);
  return {
    index_title: extractTagText(html, "title") ?? null,
    index_headings: extractHeadingTexts(contentHtml),
    index_excerpt: excerpt || null,
  };
}

export default defineTool({
  name: "sourcey.build",
  run({ inputs, env }) {
    const inputBase = env.RUNX_CWD || process.cwd();
    const project = path.resolve(inputBase, inputs.project);
    const docsInputs = parseDocsInputs(inputs.docs_inputs);
    const sourcey = inputs.sourcey_bin || env.SOURCEY_BIN || "sourcey";
    const outputDir = path.resolve(project, inputs.output_dir || ".sourcey/runx-docs");
    const command = /\.(mjs|cjs|js)$/.test(sourcey) ? process.execPath : sourcey;
    const sourceyArgs = /\.(mjs|cjs|js)$/.test(sourcey) ? [sourcey] : [];
    const mode = String(docsInputs.mode || "config");
    let buildCwd = project;
    let docsSourceRoot;

    sourceyArgs.push("build");
    if (mode === "openapi") {
      const spec = docsInputs.spec || docsInputs.openapi;
      if (!spec) {
        throw new Error("docs_inputs.spec or docs_inputs.openapi is required when docs_inputs.mode is 'openapi'.");
      }
      sourceyArgs.push(path.resolve(project, String(spec)));
    } else if (mode === "config") {
      const configPath = path.resolve(project, String(docsInputs.config || "sourcey.config.ts"));
      if (!existsSync(configPath)) {
        throw new Error(`Sourcey config not found: ${configPath}`);
      }
      buildCwd = path.dirname(configPath);
      docsSourceRoot = buildCwd;
      const configFile = path.basename(configPath);
      if (configFile !== "sourcey.config.ts") {
        sourceyArgs.push("--config", configFile);
      }
    } else {
      throw new Error(`Unsupported docs_inputs.mode: ${mode}`);
    }
    sourceyArgs.push("-o", outputDir, "--quiet");

    const failureReport = (extra = {}) => ({
      project,
      brand_name: inputs.brand_name,
      homepage_url: inputs.homepage_url,
      docs_inputs: docsInputs,
      output_dir: outputDir,
      command: "sourcey build",
      sourcey_bin: sourcey,
      sourcey_args: sourceyArgs,
      cwd: buildCwd,
      generated: false,
      index_path: path.join(outputDir, "index.html"),
      generated_files: [],
      index_title: null,
      index_headings: [],
      index_excerpt: null,
      ...extra,
    });

    const result = spawnSync(command, sourceyArgs, {
      cwd: buildCwd,
      env: sourceyEnv(env),
      encoding: "utf8",
      shell: false,
    });

    if (result.error) {
      return failure(
        failureReport({
          error: result.error.message,
          stdout: result.stdout ?? "",
          stderr: result.stderr ?? "",
        }),
        { stderr: result.error.message },
      );
    }
    if (result.status !== 0) {
      return failure(
        failureReport({
          exit_code: result.status ?? 1,
          stdout: result.stdout ?? "",
          stderr: result.stderr ?? "",
        }),
        { exitCode: result.status ?? 1, stderr: result.stderr ?? "" },
      );
    }

    const indexPath = path.join(outputDir, "index.html");
    const generated = existsSync(indexPath);
    return {
      project,
      brand_name: inputs.brand_name,
      homepage_url: inputs.homepage_url,
      docs_inputs: docsInputs,
      output_dir: outputDir,
      command: "sourcey build",
      cwd: buildCwd,
      generated,
      index_path: indexPath,
      generated_files: collectGeneratedFiles(outputDir),
      icon_validation: validateCardIcons(project, docsSourceRoot, sourcey),
      ...buildIndexEvidence(indexPath),
    };
  },
});
