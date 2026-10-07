import assert from "node:assert/strict";
import test from "node:test";

import {
  blockedOperation,
  normalizeOperation,
  presentEvidence,
  prepareOperation,
} from "./nitrosend-operation.mjs";

function plan(mode, operation, args, brandSid) {
  return prepareOperation({
    mode,
    operation,
    arguments: args,
    ...(brandSid ? { brand_sid: brandSid } : {}),
  }).operation_plan;
}

// The operation packet a Nitrosend tool's answer arrives in: the tool's
// `{meta, result}` as the kernel read it.
function normalizedEvidence(prepared, result, meta = { tool: prepared.tool }) {
  return normalizeOperation({
    operation_plan: prepared,
    provider_operation: {
      status: "success",
      provider: "nitrosend",
      operation: prepared.connector_operation,
      result: { meta, result },
    },
  }).provider_evidence;
}

function assertRefused(prepared, message) {
  assert.notEqual(prepared.decision, "ready", message);
  assert.equal(prepared.connector_operation, null, message);
  assert.equal(prepared.provider_input, null, message);
  assert.equal(prepared.access, null, message);
}

test("presents step-qualified evidence without another provider layer", () => {
  const status_evidence = { decision: "ok", operation: "billing_status" };
  const plans_evidence = { decision: "ok", operation: "billing_plans" };

  assert.deepEqual(presentEvidence({ status_evidence, plans_evidence }), {
    status_evidence,
    plans_evidence,
  });
  assert.deepEqual(
    presentEvidence({ purchase_id: 701, purchase_evidence: status_evidence }),
    { purchase_evidence: status_evidence },
  );
});

test("plans a status read as the connector's status operation for the brand", () => {
  const status = plan("read", "status", {}, "br_fixture");

  assert.equal(status.decision, "ready");
  assert.equal(status.tool, "nitro_get_status");
  assert.equal(status.connector_operation, "status.read");
  assert.equal(status.access, "read");
  assert.deepEqual(status.provider_input, { brand_sid: "br_fixture" });
  assert.equal(JSON.stringify(status).includes("nskey_"), false);
});

test("plans allowlisted brand-scoped entity queries", () => {
  const query = plan("read", "query", {
    entity: "flows",
    filters: { status: "draft", search: "MailSchema" },
    page: 1,
    per: 25,
  }, "br_sourcey");

  assert.equal(query.decision, "ready");
  assert.equal(query.connector_operation, "entities.query");
  assert.deepEqual(query.provider_input, {
    brand_sid: "br_sourcey",
    arguments: { entity: "flows", filters: { status: "draft", search: "MailSchema" }, page: 1, per: 25 },
  });

  for (const arguments_ of [
    { entity: "flows", filters: { recipient: "outside@example.com" } },
    { entity: "unknown" },
    { entity: "flows", per: 51 },
  ]) {
    assertRefused(plan("read", "query", arguments_, "br_sourcey"), JSON.stringify(arguments_));
  }
  const missingBrand = plan("read", "query", { entity: "flows" });
  assert.equal(missingBrand.decision, "refused");
  assertRefused(missingBrand);
});

test("plans bounded read-only mailbox operations", () => {
  const list = plan("read", "inbox", {
    command: "list_mailbox",
    arguments: { query: "MailSchema Content Review", view: "full", page: 1, per: 25 },
  }, "br_sourcey");
  assert.equal(list.decision, "ready");
  assert.equal(list.connector_operation, "inbox.read");
  assert.deepEqual(list.provider_input, {
    brand_sid: "br_sourcey",
    arguments: { command: "list_mailbox", query: "MailSchema Content Review", view: "full", page: 1, per: 25 },
  });

  const thread = plan("read", "inbox", { command: "get_thread", arguments: { conversation_id: 77 } }, "br_sourcey");
  assert.equal(thread.decision, "ready");
  assert.deepEqual(thread.provider_input.arguments, {
    command: "get_thread",
    conversation_id: 77,
    purpose: "read",
  });
});

test("refuses mailbox writes, ambiguous targets, and widened reads", () => {
  for (const input of [
    { command: "send_reply", arguments: { conversation_id: 77 } },
    { command: "get_attachment", arguments: { conversation_id: 77, message_id: 2, attachment_id: 3 } },
    { command: "get_thread", arguments: {} },
    { command: "list_mailbox", arguments: { view: "raw" } },
    { command: "list_mailbox", arguments: { query: "x".repeat(101) } },
    { command: "get_message_body", arguments: { conversation_id: 77, message_id: 2, offset: -1 } },
  ]) {
    assertRefused(plan("read", "inbox", input, "br_sourcey"), JSON.stringify(input));
  }
  const missingBrand = plan("read", "inbox", { command: "list_mailbox", arguments: {} });
  assert.equal(missingBrand.decision, "refused");
  assertRefused(missingBrand);
});

test("blocks malformed arguments and non-positive provider ids", () => {
  const malformed = plan("read", "status", []);
  assert.equal(malformed.decision, "needs_input");
  assertRefused(malformed);

  for (const import_id of ["", 0, -1]) {
    const status = plan("read", "import_status", { import_id });
    assert.equal(status.decision, "needs_input");
    assertRefused(status);
  }
  const imports = plan("read", "import_status", { import_id: 9 });
  assert.equal(imports.connector_operation, "entities.query");
  assert.deepEqual(imports.provider_input.arguments, { entity: "imports", filters: { id: 9 }, page: 1, per: 1 });
});

test("requires exact flow revisions before review or delivery control", () => {
  for (const revision_id of [undefined, "", 0, -1, 1.5]) {
    const review = plan("read", "review_delivery", { target_type: "flow", target_id: 12334, revision_id });
    assert.equal(review.decision, "needs_input");
    assertRefused(review);

    for (const operation of ["approve", "reject", "live"]) {
      const control = plan("act", "control_delivery", { target_type: "flow", target_id: 12334, operation, revision_id });
      assert.equal(control.decision, "needs_input");
      assertRefused(control);
    }
  }
});

test("binds an exact flow revision and recipients for a test message", () => {
  const requested = {
    target_type: "flow",
    target_id: 12334,
    revision_id: 7,
    action_id: 99,
    channel: "email",
    to: ["operator@example.com"],
    data: { review_url: "https://example.com/reviews/12334" },
    dry_run: false,
    idempotency_key: "map-flow-12334-revision-7-test-1",
  };
  const send = plan("act", "send_test_message", requested, "br_sourcey");

  assert.equal(send.decision, "ready");
  assert.equal(send.connector_operation, "test_message.send");
  assert.equal(send.access, "write");
  // runx supplies a live send's provider key; a preview takes none.
  const { dry_run: _dryRun, idempotency_key: _key, ...fields } = requested;
  assert.deepEqual(send.provider_input, { brand_sid: "br_sourcey", ...fields });

  const preview = plan("act", "send_test_message", { ...requested, dry_run: true }, "br_sourcey");
  assert.equal(preview.connector_operation, "test_message.preview");
  assert.equal(preview.access, "read");

  const normalized = normalizedEvidence(send, {
    channel: "email",
    target: { type: "flow", id: 12334, source: "target" },
    revision_id: 7,
    revision_digest: "sha256:revision",
    recipients: [{ to: "operator@example.com", source: "explicit" }],
    recipient_count: 1,
    test_send: { sent: 1, outcome: "sent" },
  });
  assert.equal(normalized.decision, "ok");
  assert.equal(normalized.operation, "send_test_message");
  assert.equal(normalized.provider_ref, "nitrosend:send_test_message:flow:12334:revision:7");
  assert.equal(normalized.result.revision_id, 7);
});

test("refuses ambiguous, unpinned, or non-idempotent test messages", () => {
  for (const arguments_ of [
    {
      target_type: "flow", target_id: 12334, channel: "email",
      to: ["operator@example.com"], dry_run: false, idempotency_key: "test-1",
    },
    {
      target_type: "campaign", target_id: 42, revision_id: 7, channel: "email",
      to: ["operator@example.com"], dry_run: false, idempotency_key: "test-2",
    },
    {
      target_type: "campaign", target_id: 42, channel: "email",
      to: [], dry_run: false, idempotency_key: "test-3",
    },
    {
      target_type: "campaign", target_id: 42, channel: "email",
      to: ["operator@example.com"], dry_run: false,
    },
  ]) {
    assertRefused(plan("act", "send_test_message", arguments_, "br_sourcey"), JSON.stringify(arguments_));
  }
  const missingBrand = plan("act", "send_test_message", {
    target_type: "flow", target_id: 12334, revision_id: 7, channel: "email",
    to: ["operator@example.com"], dry_run: false, idempotency_key: "test-4",
  });
  assert.equal(missingBrand.decision, "refused");
  assertRefused(missingBrand);
});

test("threads exact flow revisions and maps each lifecycle control to its operation", () => {
  const review = plan("read", "review_delivery", { target_type: "flow", target_id: 12334, revision_id: 7 });
  assert.equal(review.connector_operation, "delivery.review");
  assert.deepEqual(review.provider_input.arguments, { target_type: "flow", target_id: 12334, revision_id: 7 });

  const activate = plan("act", "control_delivery", { target_type: "flow", target_id: 12334, operation: "live", revision_id: 7 });
  assert.equal(activate.connector_operation, "flow.activate");
  assert.deepEqual(activate.provider_input, { target_id: 12334, revision_id: 7 });

  for (const [args, connectorOperation] of [
    [{ target_type: "campaign", target_id: 42, operation: "approve" }, "campaign.approve"],
    [{ target_type: "flow", target_id: 12334, operation: "approve", revision_id: 7 }, "flow.approve"],
    [{ target_type: "campaign", target_id: 42, operation: "live", idempotency_key: "c-42" }, "campaign.send"],
    [{ target_type: "campaign", target_id: 42, operation: "schedule", scheduled_at: "2026-11-01T00:00:00Z", idempotency_key: "c-42" }, "campaign.schedule"],
  ]) {
    const control = plan("act", "control_delivery", args);
    assert.equal(control.decision, "ready", JSON.stringify(args));
    assert.equal(control.connector_operation, connectorOperation);
    assert.equal(control.provider_input.idempotency_key, undefined);
  }
  // A lifecycle the connector declares no operation for is refused.
  const archive = plan("act", "control_delivery", { target_type: "campaign", target_id: 42, operation: "archive" });
  assert.equal(archive.decision, "refused");
  assertRefused(archive);
});

test("requires exact brand-scoped sender settings", () => {
  for (const [mode, operation, args] of [
    ["read", "sender_settings", {}],
    ["act", "configure_sender", {
      from_name: "Sourcey",
      from_email: "hello@sourcey.com",
      reply_to: "hello@sourcey.com",
      test_email_recipients: [],
    }],
  ]) {
    const prepared = plan(mode, operation, args);
    assert.equal(prepared.decision, "refused");
    assertRefused(prepared);
  }

  const invalid = plan("act", "configure_sender", {
    from_name: "Sourcey",
    from_email: "hello@sourcey.com",
    reply_to: "not-an-email",
    test_email_recipients: [],
  }, "br_sourcey");
  assert.equal(invalid.decision, "needs_input");
  assertRefused(invalid);
});

test("plans and normalizes exact brand-scoped sender configuration", () => {
  const requested = {
    from_name: "Sourcey",
    from_email: "hello@sourcey.com",
    reply_to: "hello@sourcey.com",
    test_email_recipients: ["kam@sourcey.com"],
  };
  const configure = plan("act", "configure_sender", requested, "br_sourcey");

  assert.equal(configure.decision, "ready");
  assert.equal(configure.connector_operation, "sender.configure");
  assert.deepEqual(configure.provider_input, { brand_sid: "br_sourcey", arguments: requested });

  const evidence = normalizedEvidence(
    configure,
    {
      sender: { from_name: requested.from_name, from_email: requested.from_email, reply_to: requested.reply_to },
      test_email_recipients: requested.test_email_recipients,
    },
    { tool: "nitro_configure_account", current_brand: { sid: "br_sourcey", name: "Sourcey" } },
  );
  assert.equal(evidence.decision, "ok");
  assert.equal(evidence.result.current_brand.sid, "br_sourcey");
  assert.deepEqual(evidence.result.sender_settings, {
    brand_sid: "br_sourcey",
    from_name: "Sourcey",
    from_email: "hello@sourcey.com",
    reply_to: "hello@sourcey.com",
    test_email_recipients: ["kam@sourcey.com"],
  });
});

test("maps consented inline imports without forwarding audit-only fields", () => {
  const preview = plan("act", "import_contacts", {
    source_id: "product-signup",
    consent_basis: "First-party signup opt-in",
    records: [{ email: "fixture@example.com" }],
    dry_run: true,
    idempotency_key: "fixture-import",
  });

  assert.equal(preview.connector_operation, "contacts.records.preview");
  assert.equal(preview.access, "read");
  const input = preview.provider_input;
  for (const field of ["source_id", "consent_basis", "dry_run", "idempotency_key"]) {
    assert.equal(input[field], undefined, field);
  }
  assert.equal(input.records[0].source, "product-signup");
});

test("maps one reviewed remote image ingest and strips storage capability material", () => {
  const requested = {
    image_url: "https://vendor.example/hero.png",
    description: "Product documentation preview on a blue background",
    filename: "hero.png",
  };
  const ingest = plan("act", "ingest_image", requested, "br_sourcey");

  assert.equal(ingest.decision, "ready");
  assert.equal(ingest.connector_operation, "image.ingest");
  assert.deepEqual(ingest.provider_input, { brand_sid: "br_sourcey", arguments: { kind: "image", ...requested } });

  const normalized = normalizedEvidence(ingest, {
    image_url: "https://api.nitrosend.com/cdn/images/safe/large/hero.png",
    media_url: "https://api.nitrosend.com/cdn/images/safe/large/hero.png",
    description: requested.description,
    signed_id: "opaque-storage-capability",
  });
  assert.equal(normalized.decision, "ok");
  assert.equal(normalized.result.image_url, "https://api.nitrosend.com/cdn/images/safe/large/hero.png");
  assert.equal(normalized.result.signed_id, undefined);
});

test("rejects incomplete or widened remote image ingest arguments", () => {
  for (const arguments_ of [
    { image_url: "https://vendor.example/hero.png" },
    { image_url: "file:///tmp/hero.png", description: "Local file" },
    { image_url: "https://vendor.example/hero.png", description: "Hero", image_data: "bytes" },
  ]) {
    const ingest = plan("act", "ingest_image", arguments_);
    assert.equal(ingest.decision, "needs_input");
    assertRefused(ingest);
  }
});

test("normalizes provider results and redacts provider-returned secrets", () => {
  const returnedSecret = ["nskey", "live", "secret"].join("_");
  const status = plan("read", "status", {});
  const evidence = normalizedEvidence(status, { data: { id: 42, api_token: returnedSecret } });

  assert.equal(evidence.decision, "ok");
  assert.equal(evidence.provider_ref, "nitrosend:status:42");
  assert.equal(evidence.result.data.api_token, "[REDACTED]");
  assert.equal(JSON.stringify(evidence).includes(returnedSecret), false);
});

test("projects provider-reported errors and local validation as bounded evidence", () => {
  const checkout = plan("act", "plan_checkout", { plan_id: 202, confirm: true, idempotency_key: "account-1-plan-202-v1" });
  const failed = normalizedEvidence(checkout, { error: true, message: "plan unavailable" });
  assert.equal(failed.decision, "provider_error");
  assert.deepEqual(failed.blockers, ["plan unavailable"]);

  const blockedPlan = plan("act", "unknown", {});
  const blocked = blockedOperation({ operation_plan: blockedPlan }).provider_evidence;
  assert.equal(blocked.decision, "needs_input");
  assert.equal(blocked.result, null);
});

test("refuses a result for another operation than the one admitted", () => {
  const status = plan("read", "status", {});
  assert.throws(() => normalizeOperation({
    operation_plan: status,
    provider_operation: { status: "success", provider: "nitrosend", operation: "billing.status", result: {} },
  }));
});

test("maps all composition surfaces to non-persisting reads", () => {
  for (const [target_type, tool] of [
    ["campaign", "nitro_compose_campaign"],
    ["flow", "nitro_compose_flow"],
    ["template", "nitro_manage_template"],
  ]) {
    const intent = plan("read", "compose_email", {
      target_type, composition_mode: "intent",
      arguments: { composition_mode: "draft", goal: `Write one ${target_type} email`, idempotency_key: "retry", dry_run: true },
    });
    assert.equal(intent.decision, "ready");
    assert.equal(intent.tool, tool);
    assert.equal(intent.connector_operation, `${target_type}.composition.plan`);
    assert.deepEqual(intent.provider_input.arguments, {
      goal: `Write one ${target_type} email`, composition_mode: "intent",
    });

    const validation = plan("read", "compose_email", {
      target_type, composition_mode: "validate",
      arguments: { composition_mode: "draft", contract_id: `ecc_${target_type}`, idempotency_key: "retry", body: "Candidate" },
    });
    assert.equal(validation.connector_operation, `${target_type}.composition.validate`);
    assert.deepEqual(validation.provider_input.arguments, {
      contract_id: `ecc_${target_type}`, body: "Candidate", composition_mode: "validate", validate_only: true,
    });
  }
});

test("refuses malformed composition reads", () => {
  for (const arguments_ of [
    { target_type: "message", composition_mode: "intent", arguments: { goal: "No" } },
    { target_type: "campaign", composition_mode: "draft", arguments: { goal: "No" } },
    { target_type: "campaign", composition_mode: "validate", arguments: { body: "Missing contract" } },
    { target_type: "flow", composition_mode: "intent", arguments: { contract_id: "ecc_wrong" } },
    { target_type: "template", composition_mode: "intent", arguments: [] },
  ]) {
    assertRefused(plan("read", "compose_email", arguments_), JSON.stringify(arguments_));
  }
});

test("admits only persistence-ready creative draft mutations", () => {
  for (const [operation, connectorOperation] of [
    ["compose_campaign", "campaign.draft"],
    ["compose_flow", "flow.draft"],
    ["manage_template", "template.draft"],
  ]) {
    const valid = { composition_mode: "draft", contract_id: `ecc_${operation}`, idempotency_key: `ecr_${operation}` };
    const draft = plan("act", operation, valid);
    assert.equal(draft.decision, "ready");
    assert.equal(draft.connector_operation, connectorOperation);
    assert.equal(draft.access, "write");
    for (const invalid of [
      { ...valid, composition_mode: "intent" },
      { ...valid, composition_mode: "validate" },
      { ...valid, composition_mode: "generate" },
      { ...valid, contract_id: "" },
      { ...valid, idempotency_key: "" },
    ]) {
      const refused = plan("act", operation, invalid);
      assert.equal(refused.decision, "refused");
      assertRefused(refused);
    }
  }
});

test("maps plan billing lanes to their connector operations", () => {
  assert.equal(plan("read", "billing_status", {}).connector_operation, "billing.status");
  assert.equal(plan("read", "billing_plans", {}).connector_operation, "billing.plans");

  const checkout = plan("act", "plan_checkout", { plan_id: 202, confirm: true, idempotency_key: "account-1-plan-202-v1" });
  assert.equal(checkout.connector_operation, "plan.checkout");
  assert.equal(checkout.access, "write");
  assert.deepEqual(checkout.provider_input, { plan_id: 202 });

  const purchase = plan("read", "plan_checkout_status", { purchase_id: 701 });
  assert.equal(purchase.connector_operation, "checkout.status");
  assert.deepEqual(purchase.provider_input, { purchase_id: 701 });
});

test("binds billing results to the requested operation and purchase", () => {
  const purchase701 = plan("read", "plan_checkout_status", { purchase_id: 701 });
  const purchase702 = plan("read", "plan_checkout_status", { purchase_id: 702 });

  const evidence701 = normalizedEvidence(purchase701, { operation: "checkout_status", purchase_id: 701, status: "awaiting_provider_approval" });
  const evidence702 = normalizedEvidence(purchase702, { operation: "checkout_status", purchase_id: 702, status: "active" });
  assert.equal(evidence701.provider_ref, "nitrosend:plan_purchase:701");
  assert.equal(evidence702.provider_ref, "nitrosend:plan_purchase:702");

  const mismatch = normalizedEvidence(purchase701, { operation: "checkout_status", purchase_id: 702, status: "active" });
  assert.equal(mismatch.decision, "provider_error");
  assert.equal(mismatch.provider_ref, null);
  assert.deepEqual(mismatch.blockers, ["Nitrosend returned purchase_id 702; expected 701"]);

  const status = plan("read", "billing_status", {});
  const wrongOperation = normalizedEvidence(status, { operation: "checkout", purchase_id: 701 });
  assert.equal(wrongOperation.decision, "provider_error");
  assert.deepEqual(wrongOperation.blockers, ["Nitrosend returned operation checkout; expected status"]);
});

test("refuses widened or prepaid arguments on every plan billing lane", () => {
  for (const [mode, operation, args] of [
    ["read", "billing_status", { operation: "checkout" }],
    ["read", "billing_plans", { amount_cents: 5000 }],
    ["read", "plan_checkout_status", { purchase_id: 701, instrument: "card" }],
    ["act", "plan_checkout", { plan_id: 202, confirm: true, idempotency_key: "account-1-plan-202-v1", currency: "USD" }],
    ["act", "plan_checkout", { plan_id: 202, confirm: false, idempotency_key: "account-1-plan-202-v1" }],
    ["act", "plan_checkout", { plan_id: 202, confirm: true, idempotency_key: "x".repeat(129) }],
  ]) {
    assertRefused(plan(mode, operation, args), `${operation} admitted ${JSON.stringify(args)}`);
  }
});
