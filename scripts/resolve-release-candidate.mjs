import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { candidateFromTag, readReleaseCandidate } from "./lib/release-candidate.mjs";

const tag = process.env.GITHUB_REF_NAME;
if (!/^cli-v\d+\.\d+\.\d+$/u.test(tag ?? "")) throw new Error("a CLI release tag is required");
function git(args) { return execFileSync("git", args, { encoding: "utf8" }).trim(); }
if (git(["cat-file", "-t", `refs/tags/${tag}`]) !== "tag") {
  throw new Error("CLI publication requires an annotated tag bound to its rehearsal");
}
const candidate = candidateFromTag(git(["for-each-ref", "--format=%(contents)", `refs/tags/${tag}`]));
const result = await readReleaseCandidate({
  ...candidate,
  repository: process.env.GITHUB_REPOSITORY,
  commit: git(["rev-parse", "HEAD"]),
  githubToken: process.env.GH_TOKEN,
});
const outputs = [`candidate_run_id=${result.run_id}`,
  ...Object.entries(result.artifacts).map(([key, artifact]) => `${key}_id=${artifact.id}`)];
appendFileSync(process.env.GITHUB_OUTPUT, `${outputs.join("\n")}\n`);
process.stdout.write(`${JSON.stringify(result)}\n`);
