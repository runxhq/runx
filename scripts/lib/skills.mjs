import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";

import { validateMethodManifestYamlBatch, validateSkillMarkdownBatch } from "./native-parser.mjs";

/**
 * Each package directory's `run.yaml`, validated as the runtime validates it:
 * as the package its `SKILL.md` names. Two native parser processes, however
 * many packages.
 * @param {readonly string[]} directories
 * @param {NodeJS.ProcessEnv} [env]
 * @returns {{ name: string, manifest: Record<string, any> }[]}
 */
export function packageManifests(directories, env = process.env) {
  const skills = validateSkillMarkdownBatch(
    directories.map((directory) => readFileSync(path.join(directory, "SKILL.md"), "utf8")),
    env,
  );
  const manifests = validateMethodManifestYamlBatch(
    directories.map((directory, index) => ({
      skill: skills[index].name,
      yaml: readFileSync(path.join(directory, "run.yaml"), "utf8"),
    })),
    env,
  );
  return directories.map((_, index) => ({ name: skills[index].name, manifest: manifests[index] }));
}

/**
 * The skills `root`'s skills/ holds: each directory with a SKILL.md and a
 * run.yaml, named by its directory, with the catalog visibility its run.yaml
 * declares as the native parser reads it (a skill that declares no
 * visibility is internal). In name order.
 * @param {string} root
 * @param {NodeJS.ProcessEnv} [env]
 * @param {ReadonlySet<string> | null} [selected]
 * @returns {{ name: string, visibility: string }[]}
 */
export function repositorySkills(root, env = process.env, selected = null) {
  const skillsRoot = path.join(root, "skills");
  const names = readdirSync(skillsRoot, { withFileTypes: true })
    .filter((entry) => (!selected || selected.has(entry.name))
      && entry.isDirectory()
      && existsSync(path.join(skillsRoot, entry.name, "SKILL.md"))
      && existsSync(path.join(skillsRoot, entry.name, "run.yaml")))
    .map((entry) => entry.name)
    .sort();
  const packages = packageManifests(names.map((name) => path.join(skillsRoot, name)), env);
  return names.map((name, index) => {
    const { name: declared, manifest } = packages[index];
    if (declared !== name) {
      throw new Error(`skills/${name}/SKILL.md names the skill ${declared}`);
    }
    return { name, visibility: manifest.catalog?.visibility ?? "internal" };
  });
}
