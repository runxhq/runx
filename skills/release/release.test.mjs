import { test } from "node:test";
import assert from "node:assert/strict";
import { inspectProfile, interpretPhase } from "./release.mjs";
const digest = "sha256:" + "a".repeat(64);
const execution = (version) => ({
  schema: "runx.command.execution.v1",
  command_digest: digest,
  decision: "completed",
  exit_code: 0,
  timed_out: false,
  stdout_truncated: false,
  stderr_truncated: false,
  json: { status: "verified", version, channel: "fixture", locators: ["fixture://release"] },
});
const context = (version) => ({
  schema: "runx/release.context.v1",
  profile_id: "qa",
  profile_digest: digest,
  version,
  channel: "fixture",
  commands: { verify: { command_digest: digest } },
});
test("verification never accepts a different version through truncation or whitespace normalization", () => {
  const version = "1.2.3+" + "a".repeat(122);
  assert.throws(
    () =>
      interpretPhase({
        phase: "verify",
        release_context: context(version),
        command_execution: execution(version + "-different"),
      }),
    /not truncated/,
  );
  assert.equal(
    interpretPhase({
      phase: "verify",
      release_context: context("1.2.3"),
      command_execution: execution("1.2.3 "),
    }).verification.status,
    "failed",
  );
});
test("profile conversion preserves exact arguments and leaves argv limits to native command planning", () => {
  const argv = ["node", ...Array(80).fill("arg"), "  significant  ", "x".repeat(5000)];
  const commands = Object.fromEntries(["prepare", "publish", "verify"].map((p) => [p, { argv }]));
  const r = inspectProfile({
    channel: "fixture",
    version: "1.2.3",
    project_root: ".",
    profile_file: {
      path: "release.json",
      content_digest: digest,
      contents: JSON.stringify({
        schema: "runx/release.profile.v1",
        id: "qa",
        channel: "fixture",
        commands,
      }),
    },
  });
  assert.deepEqual(r.release_context.commands.publish.args, argv.slice(1));
});
test("unsupported profile command fields are refused rather than silently ignored", () => {
  const command = { argv: ["node", "publish.mjs"], env: { CUSTOM_MODE: "different" } };
  assert.throws(
    () =>
      inspectProfile({
        channel: "fixture",
        version: "1.2.3",
        project_root: ".",
        profile_file: {
          path: "release.json",
          content_digest: digest,
          contents: JSON.stringify({
            schema: "runx/release.profile.v1",
            id: "qa",
            channel: "fixture",
            commands: { prepare: command, publish: command, verify: command },
          }),
        },
      }),
    /supports only/,
  );
});
