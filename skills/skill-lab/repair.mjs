const SOURCE_BUDGET = 48 * 1024;
const BUNDLE_LIMIT = 16;

/** Give a repair author the target's source and file paths in one turn. */
export async function sources(ctx, { brief }) {
  const { target_dir, target_files: files, target } = await ctx.read(brief);
  const relative = (file) => file.path.slice(target_dir.length + 1);
  const modules = files.filter((file) => {
    const path = relative(file);
    return /\.(?:m?js|ts)$/.test(path) && !/\.(?:test|spec)\./.test(path) && !/^(?:fixtures|tests|evals)\//.test(path);
  });
  const manifests = files.filter((file) => relative(file) === "run.yaml");
  const examples = files.filter((file) => /^tests\/[^/]+\.ya?ml$/.test(relative(file)))
    .sort((a, b) => a.bytes - b.bytes);
  const paths = [];
  let remaining = SOURCE_BUDGET;
  for (const file of [...manifests, ...modules, ...examples]) {
    if (file.bytes > remaining || paths.length === BUNDLE_LIMIT) continue;
    paths.push(file.path);
    remaining -= file.bytes;
  }
  const captured = paths.length > 0
    ? await readFiles(ctx, paths, { max_bytes: SOURCE_BUDGET, on_missing: "report" })
    : [];
  return {
    files: captured.filter((file) => !file.truncated).map(({ path, contents }) => ({
      path,
      contents: contents.split("\n").map((line, index) => `${index + 1}\t${line}`).join("\n"),
    })),
    paths: files.map(({ path }) => path),
    methods: (target?.methods ?? []).map(({ name, description, input_schema, outputs }) => ({
      name,
      description: description ?? null,
      inputs: Object.keys(input_schema.properties ?? {}),
      outputs: [...new Set(outputs.map((output) => output.name))],
    })),
    inspection_error: target?.error ?? null,
  };
}

async function readFiles(ctx, paths, options = {}) {
  const call = await ctx.tool("fs.read_bundle", { paths, ...options });
  return (await ctx.read(call.file_read_bundle)).files;
}

/** Assemble whole files for native binding using the source shown to the author. */
export async function draft(ctx, { change, source }) {
  const { decision, summary, edits, writes, deletes } = await ctx.read(change);
  if (decision !== "write" && (edits.length || writes.length || deletes.length)) {
    throw new Error("Only a write decision can change files.");
  }
  const snapshot = await ctx.read(source);
  const paths = new Set(await ctx.read(snapshot.paths));
  const edited = [...new Set(edits.map(({ path }) => path))];
  for (const path of edited) {
    if (!paths.has(path)) throw new Error(`The edit names a file outside the inspected package: ${path}`);
  }
  for (const { path } of writes) {
    if (paths.has(path)) throw new Error(`Use an exact edit for the existing file ${path}.`);
  }
  const files = await ctx.read(snapshot.files);
  const contents = new Map(files.map(({ path, contents }) => [
    path,
    contents.split("\n").map((line) => line.slice(line.indexOf("\t") + 1)).join("\n"),
  ]));
  const absent = edited.filter((path) => !contents.has(path));
  if (absent.length > 0) {
    for (const file of await readFiles(ctx, absent)) {
      if (!file.truncated) contents.set(file.path, file.contents);
    }
  }
  const updated = edited.map((path) => {
    const text = contents.get(path);
    if (text === undefined) throw new Error(`Could not read the full contents of ${path}.`);
    return { path, contents: applyEdits(path, text, edits.filter((edit) => edit.path === path)) };
  });
  return {
    change_draft: {
      decision, summary,
      writes: [...updated, ...writes],
      deletes,
    },
  };
}

/** Resolve offsets against the unchanged file before applying any replacements. */
function applyEdits(path, text, edits) {
  const lines = text.split("\n");
  const offsets = [0];
  for (const line of lines) offsets.push(offsets.at(-1) + line.length + 1);
  const ranges = edits.map(({ line, old, new: replacement }) => {
    const start = offsets[line - 1];
    const end = offsets[line];
    const at = text.indexOf(old, start);
    const next = text.indexOf(old, at + 1);
    if (start === undefined || end === undefined || at < start || at >= end || (next >= 0 && next < end)) {
      throw new Error(`The edit must start exactly once on line ${line} of ${path}.`);
    }
    return { start: at, end: at + old.length, replacement };
  }).sort((a, b) => b.start - a.start);
  let boundary = text.length;
  for (const { start, end, replacement } of ranges) {
    if (end > boundary) throw new Error(`Edits overlap in ${path}.`);
    text = text.slice(0, start) + replacement + text.slice(end);
    boundary = start;
  }
  return text;
}
