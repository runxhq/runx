import { readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { evaluateParserRequestResults } from "./lib/native-parser.mjs";

interface ParserFixture {
  readonly name: string;
  readonly scope: string;
  readonly input: Readonly<Record<string, unknown>>;
  readonly expected: unknown;
}

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const fixtureRoot = path.join(workspaceRoot, "fixtures", "parser");
const check = process.argv.includes("--check");
const selectedScope = process.argv.find((argument) => argument.startsWith("--scope="))?.slice(8);
const fixtures: Array<{ path: string; fixture: ParserFixture }> = [];
for (const scope of await parserScopes()) {
  if (selectedScope && scope !== selectedScope) continue;
  const directory = path.join(fixtureRoot, scope);
  const entries = (await readdir(directory)).filter((entry) => entry.endsWith(".json")).sort();
  for (const entry of entries) {
    const fixturePath = path.join(directory, entry);
    const fixture = JSON.parse(await readFile(fixturePath, "utf8")) as ParserFixture;
    fixtures.push({ path: fixturePath, fixture });
  }
}
const results = evaluateParserRequestResults(
  fixtures.map(({ fixture }) => parserRequest(fixture.scope, fixture.input)),
) as ParserResult[];
for (const [index, { path: fixturePath, fixture }] of fixtures.entries()) {
  const expected = expectedResult(results[index], fixture.scope);
  const updated = `${JSON.stringify({ ...fixture, expected })}\n`;
  if (check) {
    const current = await readFile(fixturePath, "utf8");
    if (current !== updated) throw new Error(`parser fixture is stale: ${path.relative(workspaceRoot, fixturePath)}`);
  } else {
    await writeFile(fixturePath, updated, "utf8");
  }
}

console.log(`${check ? "checked" : "generated"} parser fixtures through the native parser`);

async function parserScopes(): Promise<readonly string[]> {
  const entries = await readdir(fixtureRoot, { withFileTypes: true });
  return entries.filter((entry) => entry.isDirectory()).map((entry) => entry.name).sort();
}

interface ParserResult {
  readonly status: "success" | "failure";
  readonly value?: unknown;
  readonly error?: { readonly code?: string; readonly message?: string };
}

function expectedResult(result: ParserResult | undefined, scope: string): unknown {
  if (result?.status === "success" && result.value !== undefined) {
    return { validated: result.value };
  }
  if (result?.status === "failure" && result.error?.message) {
    const kind = result.error.code === "parse_error" ? "parse" : "validation";
    return { rejection: { kind, message: result.error.message } };
  }
  throw new Error(`native parser returned an unexpected batch result for ${scope}`);
}

function parserRequest(
  scope: string,
  input: Readonly<Record<string, unknown>>,
): Readonly<Record<string, unknown>> {
  switch (scope) {
    case "skills":
      return { kind: "parser.validateSkillMarkdown", ...input };
    case "method-manifests":
      return { kind: "parser.validateMethodManifestYaml", ...input };
    case "tool-manifests":
      return {
        kind: typeof input.json === "string"
          ? "parser.validateToolManifestJson"
          : "parser.validateToolManifestYaml",
        ...input,
      };
    case "installs":
      return { kind: "parser.validateSkillInstall", ...input };
    default:
      throw new Error(`unsupported parser fixture scope: ${scope}`);
  }
}
