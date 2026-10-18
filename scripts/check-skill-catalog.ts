// The hosted registry admits a public skill only with a taxonomy category and
// a catalog page carrying a display title and a summary. Apply that same rule,
// from @runxhq/contracts, to every skill as the CLI packages it, so a skill
// the registry would refuse never reaches a release.
// usage: tsx scripts/check-skill-catalog.ts --runx-bin <runx>
import { execFile } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import { availableParallelism } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

// The source, not the package build: the skills lane checks before any build.
import { assertPublicSkillCatalog } from "../packages/contracts/src/catalog-presentation.js";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const binIndex = process.argv.indexOf("--runx-bin");
const runxBin = binIndex === -1 ? "" : path.resolve(process.argv[binIndex + 1] ?? "");
if (!runxBin) {
  throw new Error("usage: tsx scripts/check-skill-catalog.ts --runx-bin <runx>");
}

interface PackagedSkill {
  readonly registry: {
    readonly package: {
      readonly metadata: Parameters<typeof assertPublicSkillCatalog>[0]["metadata"];
      readonly package_files: readonly { readonly path: string; readonly content: string }[];
    };
  };
}

const skills = readdirSync(path.join(root, "skills"), { withFileTypes: true })
  .filter((entry) => entry.isDirectory() && existsSync(path.join(root, "skills", entry.name, "SKILL.md")))
  .map((entry) => entry.name)
  .sort();
const run = promisify(execFile);
const refused: string[] = [];
async function check(skill: string): Promise<void> {
  const { stdout } = await run(runxBin, ["registry", "package", path.join("skills", skill), "--json"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const packaged = (JSON.parse(stdout) as PackagedSkill).registry.package;
  try {
    assertPublicSkillCatalog({ metadata: packaged.metadata, packageFiles: packaged.package_files });
  } catch (error) {
    refused.push(`${skill}: ${error instanceof Error ? error.message : String(error)}`);
  }
}
const queue = [...skills];
await Promise.all(Array.from({ length: availableParallelism() }, async () => {
  for (let skill = queue.shift(); skill; skill = queue.shift()) await check(skill);
}));
refused.sort();
if (refused.length > 0) {
  console.error(`The registry would refuse ${refused.length} public skill(s):\n${refused.map((line) => `  ${line}`).join("\n")}`);
  process.exit(1);
}
console.log(`${skills.length} skills meet the registry's public catalog rule`);
