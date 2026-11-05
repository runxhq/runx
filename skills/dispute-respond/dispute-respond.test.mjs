import assert from "node:assert/strict";
import { test } from "node:test";
import { prepareContext, prepareResponse, admitFiling, confirmFiling } from "./dispute-respond.mjs";

const now = "2026-11-04T00:00:00Z";
const base = { account_id: "acct_northstar", dispute_id: "du_case42", observed_at: now, acquisition: "prior_evidence" };
const wrap = (operation, target, result) => ({ data: {
  schema: "runx.provider.operation.v1", provider: "stripe", operation, target,
  status: "success", access: "read", finality: "confirmed", result,
} });
const dispute = { dispute_id: "du_case42", charge_id: "ch_original42", payment_id: "pi_original42", amount_minor: 150, currency: "usd", reason: "product_not_received", status: "needs_response", due_by: 1893456000, past_due: false, has_evidence: false, submission_count: 0, evidence: {}, livemode: false };
const charge = { charge_id: "ch_original42", payment_id: "pi_original42", amount_minor: 200, currency: "usd", amount_refunded: 0, status: "succeeded" };
const source = { id: "delivery-42", kind: "delivery", content: "Report 42 was downloaded by the buyer at 2026-11-03T01:00:00Z.", source: "merchant://orders/42/access-log" };
const contextInputs = () => ({ ...base, case_evidence: wrap("dispute.read", "stripe://acct_northstar/disputes/du_case42", structuredClone(dispute)), charge_evidence: wrap("payment.stripe.charge.read", "stripe://acct_northstar/charges/ch_original42", structuredClone(charge)), evidence: [source] });
const draft = () => ({ posture: "contest", summary: "The selected delivery log records buyer access to this report; respond to the non-receipt allegation with that evidence.", fields: { access_activity_log: source.content }, citations: [{ field: "access_activity_log", evidence_id: source.id, quote: source.content }], open_questions: [] });
const prepared = () => prepareResponse({ case_context: prepareContext(contextInputs()).case_context, response_draft: draft() }).prepared_response;

test("a real pre-Runx charge can be prepared without a Runx charge receipt", () => {
  const result = prepared();
  assert.equal(result.decision, "ready");
  assert.equal(result.request.amount_minor, 150);
  assert.equal(result.request.submission_count, 1);
  assert.equal(result.filing_status, "not_attempted");
  assert.equal(result.acquisition, "prior_evidence");
  assert.equal(result.sources.find((item) => item.id === "provider-charge").content.includes('"amount_refunded":0'), true);
});

test("unrelated case/payment evidence and refunds cannot be hidden", () => {
  for (const change of [
    (value) => { value.charge_evidence.data.result.charge_id = "ch_other"; },
    (value) => { value.case_evidence.data.target = "stripe://acct_other/disputes/du_case42"; },
    (value) => { value.charge_evidence.data.result.currency = "aud"; },
    (value) => { value.charge_evidence.data.result.amount_minor = 100; },
    (value) => { value.charge_evidence.data.result.amount_refunded = 201; },
  ]) {
    const input = contextInputs(); change(input);
    assert.equal(prepareContext(input).case_context.actionable, false);
  }
});

test("closed and expired cases hold before any drafting or filing", () => {
  for (const patch of [{ status: "won" }, { past_due: true }, { due_by: 1 }]) {
    const input = contextInputs(); Object.assign(input.case_evidence.data.result, patch);
    const context = prepareContext(input).case_context;
    assert.equal(context.actionable, false);
    assert.equal(prepareResponse({ case_context: context }).prepared_response.decision, "held");
  }
});

test("missing evidence, unbound citations, open questions and unsupported fields hold", () => {
  for (const change of [
    (value) => { value.fields = {}; value.citations = []; },
    (value) => { value.citations[0].evidence_id = "invented"; },
    (value) => { value.citations[0].quote = "not in source"; },
    (value) => { value.open_questions = ["Does this log belong to this customer?"]; },
    (value) => { value.fields.refund_customer = "true"; },
    (value) => { value.posture = "accept"; },
    (value) => { value.posture = "refund_already_sent"; },
  ]) {
    const answer = draft(); change(answer);
    assert.equal(prepareResponse({ case_context: prepareContext(contextInputs()).case_context, response_draft: answer }).prepared_response.decision, "held");
  }
});

test("a file field needs an actual uploaded provider file ID in its cited source", () => {
  const answer = draft();
  answer.fields = { service_documentation: "file_invented" };
  answer.citations[0].field = "service_documentation";
  assert.equal(prepareResponse({ case_context: prepareContext(contextInputs()).case_context, response_draft: answer }).prepared_response.decision, "held");
});

test("native JSON key ordering does not invalidate an unchanged prepared request", () => {
  const prior = prepared();
  prior.request = Object.fromEntries(Object.entries(prior.request).reverse());
  assert.equal(admitFiling({ prepared_response: prior, observed_at: now }).filing.request.submission_count, 1);
});

test("the whole staged evidence is included in approval and guarded against concurrent change", () => {
  const input = contextInputs();
  input.case_evidence.data.result.evidence = { uncategorized_text: "Existing case context.", customer_name: null };
  const prior = prepareResponse({ case_context: prepareContext(input).case_context, response_draft: draft() }).prepared_response;
  assert.equal(prior.decision, "ready");
  assert.equal(prior.request.evidence_form["evidence[uncategorized_text]"], "Existing case context.");
  assert.deepEqual(prior.request.expected_evidence, input.case_evidence.data.result.evidence);
  prior.request.expected_evidence = Object.fromEntries(Object.entries(prior.request.expected_evidence).reverse());
  assert.equal(admitFiling({ prepared_response: prior, observed_at: now }).filing.request.submission_count, 1);
  input.case_evidence.data.result.evidence.new_unknown_evidence = { hidden: true };
  assert.equal(prepareResponse({ case_context: prepareContext(input).case_context, response_draft: draft() }).prepared_response.decision, "held");
});

test("altered request terms and an expired prepared response are refused before the effect", () => {
  const prior = prepared(); prior.request.amount_minor += 1;
  assert.throws(() => admitFiling({ prepared_response: prior, observed_at: now }), /changed|stale/);
  assert.throws(() => admitFiling({ prepared_response: prepared(), observed_at: "2031-01-01T00:00:00Z" }), /changed|stale/);
});

test("filing needs the new count, exact evidence and confirmed independent native readback", () => {
  const filing = admitFiling({ prepared_response: prepared(), observed_at: now }).filing;
  const packet = { schema: "runx.provider.operation.v1", provider: "stripe", operation: "dispute.file", target: "stripe://acct_northstar/disputes/du_case42", status: "success", access: "mutate", finality: "confirmed", result: { ...dispute, submission_count: 1, has_evidence: true, status: "under_review", evidence: { access_activity_log: source.content } } };
  assert.equal(confirmFiling({ filing, provider_result: { data: packet } }).filing_result.state, "filed");
  for (const change of [
    (value) => { value.result.submission_count = 0; },
    (value) => { value.result.evidence.access_activity_log = "different text"; },
    (value) => { value.finality = "acknowledged"; },
    (value) => { value.target = "stripe://acct_other/disputes/du_case42"; },
  ]) {
    const altered = structuredClone(packet); change(altered);
    assert.equal(confirmFiling({ filing, provider_result: { data: altered } }).filing_result.state, "unconfirmed");
  }
});
