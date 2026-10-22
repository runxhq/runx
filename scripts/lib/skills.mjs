import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";

import { validateRunnerManifestYamlBatch } from "./native-parser.mjs";

/**
 * The skills `root`'s skills/ holds: each directory with a SKILL.md and a
 * run.yaml, named by its directory, with the catalog visibility and role its
 * run.yaml declares as the native parser reads them (a skill that declares
 * no visibility is internal). In name order.
 * @param {string} root
 * @returns {{ name: string, visibility: string, role: string | undefined }[]}
 */
export function repositorySkills(root, env = process.env) {
  const skillsRoot = path.join(root, "skills");
  const names = readdirSync(skillsRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory()
      && existsSync(path.join(skillsRoot, entry.name, "SKILL.md"))
      && existsSync(path.join(skillsRoot, entry.name, "run.yaml")))
    .map((entry) => entry.name)
    .sort();
  const manifests = validateRunnerManifestYamlBatch(
    names.map((name) => readFileSync(path.join(skillsRoot, name, "run.yaml"), "utf8")),
    env,
  );
  return names.map((name, index) => {
    const manifest = manifests[index];
    if (manifest.skill && manifest.skill !== name) {
      throw new Error(`skills/${name}/run.yaml names the skill ${manifest.skill}`);
    }
    return { name, visibility: manifest.catalog?.visibility ?? "internal", role: manifest.catalog?.role };
  });
}
