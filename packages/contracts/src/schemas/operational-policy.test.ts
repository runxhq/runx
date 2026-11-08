import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { validateOperationalPolicyContract } from "./operational-policy.js";
import type { OperationalPolicyContract } from "../generated/contract-types.js";

const fixtureRoot = new URL("../../../../fixtures/operational-policy/", import.meta.url);

const validPolicy: OperationalPolicyContract = {
  schema: "runx.operational_policy.v1",
  schema_version: "runx.operational_policy.v1",
  policy_id: "provider-dev-flow",
  created_at: "2026-05-19T02:00:00Z",
  sources: [
    {
      source_id: "slack-bugs",
      provider: "slack",
      allowed_locators: ["slack://team/T123/channel/CBUGS"],
      allowed_actions: ["reply-only", "issue-intake", "issue-to-pr", "manual-review"],
      source_thread: {
        required: true,
        publish_mode: "reply",
        missing_behavior: "fail_closed",
      },
      minimum_confidence: 0.72,
    },
    {
      source_id: "sentry-production",
      provider: "sentry",
      allowed_locators: ["sentry://example/production"],
      allowed_actions: ["issue-intake", "issue-to-pr", "manual-review"],
      source_thread: {
        required: true,
        publish_mode: "reply",
        missing_behavior: "fail_closed",
      },
      adapter_policy: {
        sentry: {
          production_only: true,
          unresolved_only: true,
          regressed_only: true,
        },
      },
    },
  ],
  runners: [
    {
      runner_id: "local-review",
      kind: "local",
      state: "available",
      allowed_actions: ["issue-to-pr", "pr-review", "pr-fix-up", "merge-assist"],
      target_repos: ["example/api", "example/app"],
      scafld_required: true,
    },
  ],
  owner_routes: [
    {
      route_id: "api-kam",
      owners: ["Ops"],
      target_repos: ["example/api"],
      labels: ["runx", "api"],
      project: "Example Engineering",
    },
    {
      route_id: "app-chong",
      owners: ["Design"],
      target_repos: ["example/app"],
      labels: ["runx", "app"],
    },
  ],
  targets: [
    {
      repo: "example/api",
      runner_ids: ["local-review"],
      allowed_actions: ["issue-to-pr", "pr-review", "pr-fix-up", "merge-assist"],
      default_owner_route: "api-kam",
      scafld_required: true,
      base_branch: "main",
    },
    {
      repo: "example/app",
      runner_ids: ["local-review"],
      allowed_actions: ["issue-to-pr", "pr-review", "pr-fix-up", "merge-assist"],
      default_owner_route: "app-chong",
      scafld_required: true,
      base_branch: "main",
    },
  ],
  dedupe: {
    strategy: "source_digest",
    key_fields: ["source_locator", "digest", "target_repo"],
    on_duplicate: "reuse",
  },
  outcomes: {
    observe_provider: true,
    verification_required: true,
    close_source_issue: "when_verified",
    publish_final_source_thread_update: true,
  },
  permissions: {
    auto_merge: false,
    mutate_target_repo: true,
    require_human_merge_gate: true,
  },
};

describe("operational-policy schema", () => {
  it("accepts a valid multi-source, multi-target policy", () => {
    expect(validateOperationalPolicyContract(validPolicy)).toMatchObject({
      policy_id: "provider-dev-flow",
      permissions: {
        auto_merge: false,
        require_human_merge_gate: true,
      },
    });
  });

  it.each([
    "provider-like.json",
    "minimal-single-repo.json",
  ])("accepts positive fixture %s", (fixtureName) => {
    const policy = readPolicyFixture(fixtureName);

    expect(validateOperationalPolicyContract(policy)).toMatchObject({
      schema: "runx.operational_policy.v1",
      schema_version: "runx.operational_policy.v1",
    });
  });

  it.each([
    "invalid-schema-literal.json",
    "invalid-secret-field.json",
  ])("rejects schema-invalid fixture %s", (fixtureName) => {
    const policy = readPolicyFixture(fixtureName);

    expect(() => validateOperationalPolicyContract(policy)).toThrow();
  });

  it("rejects policy that enables auto-merge", () => {
    expect(() => validateOperationalPolicyContract({
      ...validPolicy,
      permissions: {
        ...validPolicy.permissions,
        auto_merge: true,
      },
    })).toThrow();
  });

  it("rejects source routes that can fall back when the source thread is missing", () => {
    expect(() => validateOperationalPolicyContract({
      ...validPolicy,
      sources: [{
        ...validPolicy.sources[0],
        source_thread: {
          ...validPolicy.sources[0].source_thread,
          missing_behavior: "post_to_root",
        },
      }],
    })).toThrow();
  });

  it("rejects target repos that are not owner/repo slugs", () => {
    expect(() => validateOperationalPolicyContract({
      ...validPolicy,
      targets: [{
        ...validPolicy.targets[0],
        repo: "example",
      }],
    })).toThrow();
  });

  it("rejects extra fields so secrets do not drift into policy", () => {
    expect(() => validateOperationalPolicyContract({
      ...validPolicy,
      github_token: "ghp_123",
    })).toThrow();
  });
});

function readPolicyFixture(fixtureName: string): unknown {
  return JSON.parse(readFileSync(new URL(fixtureName, fixtureRoot), "utf8")) as unknown;
}
