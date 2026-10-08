import assert from "node:assert/strict";
import { readFile, realpath } from "node:fs/promises";
import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import vm from "node:vm";

const installRoot = await realpath(path.resolve(process.argv[2] ?? "."));
const resolved = JSON.parse(execFileSync(process.execPath, ["--input-type=module", "-e",
  "process.stdout.write(JSON.stringify(['@runxhq/client', '@runxhq/client/adapters', '@runxhq/contracts'].map((specifier) => import.meta.resolve(specifier))))",
], { cwd: installRoot, encoding: "utf8" }));
const [rootPath, adaptersPath, contractsPath] = await Promise.all(resolved.map((url) => realpath(fileURLToPath(url))));
for (const file of [rootPath, adaptersPath, contractsPath]) {
  const relative = path.relative(installRoot, file);
  assert(relative && relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative), `Export escapes clean install: ${file}`);
  assert(file.endsWith(".js"), `Export is not emitted JavaScript: ${file}`);
}
const client = await import(pathToFileURL(rootPath).href);
assert.equal(typeof client.createRunxClient, "function");
assert.equal(client.invoke, undefined, "Private process transport became public");
await import(pathToFileURL(contractsPath).href);

assert.equal(typeof vm.SourceTextModule, "function", "Run this check with --experimental-vm-modules");
const context = vm.createContext({});
const modules = new Map();
const adapterRoot = path.dirname(adaptersPath);
async function portableModule(file) {
  const canonical = await realpath(file);
  const relative = path.relative(adapterRoot, canonical);
  assert(!relative.startsWith("..") && !path.isAbsolute(relative), "Portable module escapes adapter subpath");
  if (modules.has(canonical)) return modules.get(canonical);
  const module = new vm.SourceTextModule(await readFile(canonical, "utf8"), { context, identifier: canonical });
  modules.set(canonical, module);
  await module.link(async (specifier, importer) => {
    assert(specifier.startsWith("./"), `Portable runtime imports external module: ${specifier}`);
    return await portableModule(path.resolve(path.dirname(importer.identifier), specifier));
  });
  return module;
}
const portable = await portableModule(adaptersPath);
await portable.evaluate();
const pending = { status: "needs_approval", outcome: "deferred", run_id: "run_package_check" };
assert.equal(portable.namespace.toOpenAiResponse(pending).structuredContent.runx, pending);
assert.equal(portable.namespace.toAnthropicResponse(pending).metadata.runx, pending);
process.stdout.write(JSON.stringify({ rootPath, adaptersPath, contractsPath, portableModules: modules.size, nodeGlobals: false }) + "\n");
