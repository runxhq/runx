import assert from "node:assert/strict";
import { test } from "node:test";
import { admitRequest, finalizeDraft } from "./contract-draft.mjs";
const job = {
  template: {
    id: "northstar-support",
    version: "2026-11",
    title: "Support Services Agreement",
    required_terms: ["service", "fee", "currency", "start_date"],
    clauses: [
      {
        id: "parties",
        heading: "Parties",
        baseline:
          "{{provider.legal_name}} provides services to {{customer.legal_name}}.",
      },
      {
        id: "service",
        heading: "Services",
        baseline: "Services: {{terms.service}}",
      },
      {
        id: "fees",
        heading: "Fees",
        baseline: "Customer pays {{terms.currency}} {{terms.fee}} per month.",
      },
      {
        id: "start",
        heading: "Commencement",
        baseline: "Services begin {{terms.start_date}}.",
      },
    ],
  },
  parties: {
    provider: { legal_name: "Northstar Services Ltd (fixture)" },
    customer: { legal_name: "Example Product Co (fixture)" },
  },
  terms: {
    service: "Inbox triage and a weekly support report.",
    fee: "1200.00",
    currency: "USD",
    start_date: "2026-12-01",
  },
};
const draft = {
  title: "Support Services Agreement",
  clauses: [
    {
      id: "parties",
      heading: "Parties",
      text: "Northstar Services Ltd (fixture) provides services to Example Product Co (fixture).",
    },
    {
      id: "service",
      heading: "Services",
      text: "Services: Inbox triage and a weekly support report.",
    },
    {
      id: "fees",
      heading: "Fees",
      text: "Customer pays USD 1200.00 per month.",
    },
    {
      id: "start",
      heading: "Commencement",
      text: "Services begin 2026-12-01.",
    },
  ],
  deviations: [],
};
const digest = "sha256:" + "a".repeat(64);
function result(change = () => {}, source = job) {
  const input = {
    ...structuredClone(source),
    draft_doc: structuredClone(draft),
    draft_context: admitRequest(source).draft_context,
    template_digest: digest,
    parties_digest: digest,
    terms_digest: digest,
  };
  change(input);
  return finalizeDraft(input).contract_draft;
}
test("retains the complete ordered contract and exact clause text", () => {
  const written = result();
  assert.equal(written.decision, "drafted");
  assert.deepEqual(written.document.clauses, draft.clauses);
  assert.equal(written.delivery_performed, false);
  const spaced = structuredClone(job);
  spaced.template.clauses[0].baseline =
    "  " + spaced.template.clauses[0].baseline + "  ";
  const exact = result((i) => {
    i.draft_doc.clauses[0].text = "  " + i.draft_doc.clauses[0].text + "  ";
  }, spaced);
  assert.equal(exact.decision, "drafted");
  assert.equal(
    exact.document.clauses[0].text,
    "  " + draft.clauses[0].text + "  ",
  );
});
test("refuses unknown and repeated clauses rather than silently dropping content", () => {
  for (const change of [
    (i) =>
      i.draft_doc.clauses.push({
        id: "extra",
        heading: "Extra",
        text: "Additional legal obligation.",
      }),
    (i) => i.draft_doc.clauses.push({ ...i.draft_doc.clauses[0] }),
    (i) => (i.draft_doc.clauses[0].heading = "New heading"),
    (i) => (i.draft_doc.title = "New title"),
  ]) {
    const refused = result(change);
    assert.equal(refused.decision, "refused");
    assert.equal(refused.document, null);
    assert.equal(refused.send_proposal, null);
  }
});
test("requires real, unique deviations and retains their exact baseline and text", () => {
  const accepted = result((i) => {
    i.draft_doc.clauses[1].text = "Services: Expanded support scope.";
    i.draft_doc.deviations = [
      {
        clause_id: "service",
        reason: "Explicitly supplied revised scope for review.",
      },
    ];
  });
  assert.equal(accepted.decision, "drafted");
  assert.equal(accepted.deviations[0].baseline, draft.clauses[1].text);
  for (const change of [
    (i) =>
      (i.draft_doc.deviations = [
        { clause_id: "service", reason: "No actual change." },
      ]),
    (i) => {
      i.draft_doc.clauses[1].text = "Changed.";
      i.draft_doc.deviations = [
        { clause_id: "service", reason: "one" },
        { clause_id: "service", reason: "two" },
      ];
    },
  ])
    assert.equal(result(change).decision, "refused");
});
test("unresolved or inherited evidence stops before drafting and cannot become a deviation", () => {
  const missing = structuredClone(job);
  delete missing.parties.provider;
  assert.equal(admitRequest(missing).draft_context.path, "stop");
  const repeated = structuredClone(job);
  repeated.template.clauses.push({ ...repeated.template.clauses[0] });
  assert.equal(admitRequest(repeated).draft_context.path, "stop");
  for (const baseline of [
    "{{customer.toString}}",
    "{{customer.unknown}}",
    "{{terms.object}}",
    "{{terms.array}}",
    "{{customer.legal-name}}",
  ]) {
    const bad = structuredClone(job);
    bad.template.clauses[0].baseline = baseline;
    bad.terms.object = { name: "not scalar" };
    bad.terms.array = [{ name: "not scalar" }];
    assert.equal(admitRequest(bad).draft_context.path, "stop");
  }
});
