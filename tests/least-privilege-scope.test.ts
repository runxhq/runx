import { describe, expect, it } from "vitest";

import auditLeastPrivilege from "../skills/least-privilege/least-privilege.mjs";

describe("least-privilege scope review", () => {
  it("classifies exact scopes against verified receipts and never narrows a grant", () => {
    const result = auditLeastPrivilege({
      subject: "growth/lifecycle-campaign-send",
      granted_scopes: ["email:send", "repo:write", "payment:spend"],
      receipt_ids: ["rcpt_campaign_1"],
      ledger_evidence: {
        matched_receipts: [{ receipt_id: "rcpt_campaign_1" }],
        receipt_details: [{
          id: "rcpt_campaign_1",
          subject_ref: "growth/lifecycle-campaign-send",
          verification: { status: "verified" },
          authority: { exercised_scopes: [{ scope: "email:send" }, { scope: "repo:read" }] },
        }],
        chain_verification: { checked: true, intact: true, breaks: [] },
      },
    });
    const report = result.audit_report;

    expect(report.status).toBe("review_required");
    expect(report.scope_diff.map(({ granted_scope, classification }) => [granted_scope, classification])).toEqual([
      ["email:send", "recorded"],
      ["repo:write", "not_recorded"],
      ["payment:spend", "not_recorded"],
    ]);
    // Scope strings are opaque: a recorded repo:read never narrows repo:write.
    expect(report.retained_grant).toEqual(["email:send", "repo:write", "payment:spend"]);
    expect(result.attenuation_proposals).toEqual([]);
  });
});
