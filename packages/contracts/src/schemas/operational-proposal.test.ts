import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import {
  validateOperationalProposalContract,
} from "./operational-proposal.js";

const fixtureRoot = new URL("../../../../fixtures/contracts/operational-proposal/", import.meta.url);
const compositionFixtureRoot = new URL("../../../../fixtures/operational-proposal/public/", import.meta.url);

describe("operational proposal schema", () => {
  it.each([
    "proposal-prepared.json",
    "proposal-blocked.json",
  ])("accepts positive fixture %s", (fixtureName) => {
    const proposal = readExpected(fixtureName);

    expect(validateOperationalProposalContract(proposal)).toMatchObject({
      schema: "runx.operational_proposal.v1",
      redaction_status: expect.any(String),
      source_ref: expect.objectContaining({
        type: expect.any(String),
        uri: expect.any(String),
      }),
      authority: {
        proposal_only: true,
        mutation_authority_granted: false,
        publication_authority_granted: false,
        final_decision_authority_granted: false,
      },
    });
    expect(JSON.stringify(proposal)).not.toMatch(/github|slack/i);
  });

  it.each([
    "invalid-authority-claim.json",
    "invalid-missing-redaction.json",
    "invalid-missing-source-ref.json",
    "invalid-provider-specific-field.json",
    "invalid-product-specific-field.json",
  ])("rejects invalid fixture %s", (fixtureName) => {
    expect(() => validateOperationalProposalContract(readExpected(fixtureName))).toThrow();
  });

  it("accepts provider-neutral composition path proposals", () => {
    const fixture = JSON.parse(readFileSync(new URL("composition-paths.json", compositionFixtureRoot), "utf8")) as {
      readonly paths: readonly {
        readonly path_id: string;
        readonly proposal: unknown;
      }[];
    };

    expect(JSON.stringify(fixture)).not.toMatch(/github|slack/i);
    expect(fixture.paths.map((path) => path.path_id)).toEqual([
      "read_only_check",
      "create_issue",
      "build_fix_without_prior_check",
      "escalation_proposal",
      ["outreach", "proposal"].join("_"),
      "manual_review",
      "no_action",
    ]);

    for (const path of fixture.paths) {
      const proposal = validateOperationalProposalContract(path.proposal);
      const proposalWire = JSON.stringify(proposal);

      expect(proposal.source_ref.type).toBe("thread");
      expect(proposal.authority).toMatchObject({
        proposal_only: true,
        mutation_authority_granted: false,
        publication_authority_granted: false,
        final_decision_authority_granted: false,
      });
      expect(proposalWire).not.toContain("github_issue_url");
      expect(proposalWire).not.toContain("github_pr_url");
      expect(proposalWire).not.toContain("slack://");
      expect(proposalWire).not.toContain("https://github.com");
    }
  });
});

function readExpected(fixtureName: string): unknown {
  const fixture = JSON.parse(readFileSync(new URL(fixtureName, fixtureRoot), "utf8")) as {
    readonly expected: unknown;
  };
  return fixture.expected;
}
