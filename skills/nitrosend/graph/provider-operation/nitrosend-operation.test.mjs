import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import Ajv2020 from "ajv/dist/2020.js";

import {
  blockedOperation,
  normalizeOperation,
  presentEvidence,
  prepareOperation,
} from "./nitrosend-operation.mjs";

const billingSchema = JSON.parse(readFileSync(
  new URL("./nitro-manage-billing.input-schema.json", import.meta.url),
  "utf8",
));
const validateBillingArguments = new Ajv2020({ strict: true }).compile(billingSchema);

function mcpPayload(result, meta = { tool: "fixture" }) {
  return {
    jsonrpc: "2.0",
    id: "fixture",
    result: {
      content: [{
        type: "text",
        text: JSON.stringify({ meta, result }),
      }],
    },
  };
}

function normalizedEvidence(plan, result) {
  return normalizeOperation({
    operation_plan: plan,
    http_execution: {
      responses: [{
        id: `nitrosend-${plan.operation}`,
        status: 200,
        ok: true,
        body_digest: "sha256:fixture",
        json: mcpPayload(result, { tool: plan.tool }),
      }],
    },
  }).provider_evidence;
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

test("prepares a bounded read through native authenticated HTTP", () => {
  const { operation_plan: plan } = prepareOperation({
    mode: "read",
    operation: "status",
    arguments: {},
    brand_sid: "br_fixture",
  });

  assert.equal(plan.decision, "ready");
  assert.equal(plan.tool, "nitro_get_status");
  assert.deepEqual(plan.allowed_hosts, ["api.nitrosend.com"]);
  assert.deepEqual(plan.auth, { type: "bearer", secret_env: "NITROSEND_API_KEY" });
  assert.equal(plan.requests.length, 1);
  assert.match(plan.requests[0].id, /^[A-Za-z0-9_-]+$/u);
  assert.equal(plan.requests[0].body.id, plan.requests[0].id);
  assert.equal(plan.requests[0].body.params.name, "nitro_get_status");
  assert.equal(plan.requests[0].headers["x-brand-sid"], "br_fixture");
  assert.equal(JSON.stringify(plan).includes("nskey_"), false);
});

test("prepares allowlisted brand-scoped entity queries", () => {
  const { operation_plan: plan } = prepareOperation({
    mode: "read",
    operation: "query",
    arguments: {
      entity: "flows",
      filters: { status: "draft", search: "MailSchema" },
      page: 1,
      per: 25,
    },
    brand_sid: "br_sourcey",
  });

  assert.equal(plan.decision, "ready");
  assert.equal(plan.tool, "nitro_query");
  assert.equal(plan.requests[0].headers["x-brand-sid"], "br_sourcey");
  assert.deepEqual(plan.requests[0].body.params.arguments, {
    entity: "flows",
    filters: { status: "draft", search: "MailSchema" },
    page: 1,
    per: 25,
  });

  for (const arguments_ of [
    { entity: "flows", filters: { recipient: "outside@example.com" } },
    { entity: "unknown" },
    { entity: "flows", per: 51 },
  ]) {
    const refused = prepareOperation({
      mode: "read",
      operation: "query",
      arguments: arguments_,
      brand_sid: "br_sourcey",
    }).operation_plan;
    assert.notEqual(refused.decision, "ready");
    assert.deepEqual(refused.requests, []);
  }

  const missingBrand = prepareOperation({
    mode: "read",
    operation: "query",
    arguments: { entity: "flows" },
  }).operation_plan;
  assert.equal(missingBrand.decision, "refused");
  assert.deepEqual(missingBrand.requests, []);
});

test("prepares bounded read-only mailbox operations", () => {
  const list = prepareOperation({
    mode: "read",
    operation: "inbox",
    arguments: {
      command: "list_mailbox",
      arguments: { query: "MailSchema Content Review", view: "full", page: 1, per: 25 },
    },
    brand_sid: "br_sourcey",
  }).operation_plan;
  assert.equal(list.decision, "ready");
  assert.equal(list.tool, "nitro_inbox");
  assert.equal(list.requests[0].headers["x-brand-sid"], "br_sourcey");
  assert.deepEqual(list.requests[0].body.params.arguments, {
    command: "list_mailbox",
    query: "MailSchema Content Review",
    view: "full",
    page: 1,
    per: 25,
  });

  const thread = prepareOperation({
    mode: "read",
    operation: "inbox",
    arguments: { command: "get_thread", arguments: { conversation_id: 77 } },
    brand_sid: "br_sourcey",
  }).operation_plan;
  assert.equal(thread.decision, "ready");
  assert.deepEqual(thread.requests[0].body.params.arguments, {
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
    const refused = prepareOperation({
      mode: "read",
      operation: "inbox",
      arguments: input,
      brand_sid: "br_sourcey",
    }).operation_plan;
    assert.notEqual(refused.decision, "ready");
    assert.deepEqual(refused.requests, []);
  }

  const missingBrand = prepareOperation({
    mode: "read",
    operation: "inbox",
    arguments: { command: "list_mailbox", arguments: {} },
  }).operation_plan;
  assert.equal(missingBrand.decision, "refused");
  assert.deepEqual(missingBrand.requests, []);
});

test("blocks malformed arguments and non-positive provider ids before HTTP", () => {
  const malformed = prepareOperation({ mode: "read", operation: "status", arguments: [] }).operation_plan;
  assert.equal(malformed.decision, "needs_input");
  assert.deepEqual(malformed.requests, []);

  for (const import_id of ["", 0, -1]) {
    const plan = prepareOperation({
      mode: "read",
      operation: "import_status",
      arguments: { import_id },
    }).operation_plan;
    assert.equal(plan.decision, "needs_input");
    assert.deepEqual(plan.requests, []);
  }
});

test("requires exact flow revisions before review or publish transport", () => {
  for (const revision_id of [undefined, "", 0, -1, 1.5]) {
    const review = prepareOperation({
      mode: "read",
      operation: "review_delivery",
      arguments: { target_type: "flow", target_id: 12334, revision_id },
    }).operation_plan;
    assert.equal(review.decision, "needs_input");
    assert.deepEqual(review.requests, []);

    for (const operation of ["approve", "reject", "live"]) {
      const control = prepareOperation({
        mode: "act",
        operation: "control_delivery",
        arguments: { target_type: "flow", target_id: 12334, operation, revision_id },
      }).operation_plan;
      assert.equal(control.decision, "needs_input");
      assert.deepEqual(control.requests, []);
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
  const { operation_plan: plan } = prepareOperation({
    mode: "act",
    operation: "send_test_message",
    arguments: requested,
    brand_sid: "br_sourcey",
  });

  assert.equal(plan.decision, "ready");
  assert.equal(plan.tool, "nitro_send_test_message");
  assert.equal(plan.requests[0].headers["x-brand-sid"], "br_sourcey");
  assert.deepEqual(plan.requests[0].body.params.arguments, requested);

  const normalized = normalizedEvidence(plan, {
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
    const refused = prepareOperation({
      mode: "act",
      operation: "send_test_message",
      arguments: arguments_,
      brand_sid: "br_sourcey",
    }).operation_plan;
    assert.notEqual(refused.decision, "ready");
    assert.deepEqual(refused.requests, []);
  }

  const missingBrand = prepareOperation({
    mode: "act",
    operation: "send_test_message",
    arguments: {
      target_type: "flow", target_id: 12334, revision_id: 7, channel: "email",
      to: ["operator@example.com"], dry_run: false, idempotency_key: "test-4",
    },
  }).operation_plan;
  assert.equal(missingBrand.decision, "refused");
  assert.deepEqual(missingBrand.requests, []);
});

test("threads exact flow revisions and preserves campaign lifecycle behavior", () => {
  const review = prepareOperation({
    mode: "read",
    operation: "review_delivery",
    arguments: { target_type: "flow", target_id: 12334, revision_id: 7 },
  }).operation_plan;
  assert.equal(review.decision, "ready");
  assert.deepEqual(review.requests[0].body.params.arguments, {
    target_type: "flow",
    target_id: 12334,
    revision_id: 7,
  });

  const publish = prepareOperation({
    mode: "act",
    operation: "control_delivery",
    arguments: { target_type: "flow", target_id: 12334, operation: "live", revision_id: 7 },
  }).operation_plan;
  assert.equal(publish.decision, "ready");
  assert.deepEqual(publish.requests[0].body.params.arguments, {
    target_type: "flow",
    target_id: 12334,
    operation: "live",
    revision_id: 7,
  });

  const campaignReview = prepareOperation({
    mode: "read",
    operation: "review_delivery",
    arguments: { target_type: "campaign", target_id: 42 },
  }).operation_plan;
  assert.equal(campaignReview.decision, "ready");

  const campaignApprove = prepareOperation({
    mode: "act",
    operation: "control_delivery",
    arguments: { target_type: "campaign", target_id: 42, operation: "approve" },
  }).operation_plan;
  assert.equal(campaignApprove.decision, "ready");
});

test("prepares a bounded self-contained content review without private entity ids", () => {
  const plan = prepareOperation({
    mode: "read",
    operation: "review_content",
    arguments: { subject: "A useful update", html: "<h1>Update</h1>" },
  }).operation_plan;

  assert.equal(plan.decision, "ready");
  assert.equal(plan.tool, "nitro_review_delivery");
  assert.deepEqual(plan.requests[0].body.params.arguments, {
    subject: "A useful update",
    html: "<h1>Update</h1>",
  });
  assert.equal(plan.requests[0].headers["x-brand-sid"], undefined);

  for (const arguments_ of [
    { subject: "ok", html: "" },
    { subject: "ok", html: "<p>ok</p>", target_id: 42 },
    { subject: "x".repeat(999), html: "<p>ok</p>" },
  ]) {
    const refused = prepareOperation({
      mode: "read",
      operation: "review_content",
      arguments: arguments_,
    }).operation_plan;
    assert.equal(refused.decision, "needs_input");
    assert.deepEqual(refused.requests, []);
  }
});

test("requires exact brand-scoped sender settings before HTTP", () => {
  for (const mode of ["read", "act"]) {
    const operation = mode === "read" ? "sender_settings" : "configure_sender";
    const plan = prepareOperation({
      mode,
      operation,
      arguments:
        mode === "read"
          ? {}
          : {
              from_name: "Sourcey",
              from_email: "hello@sourcey.com",
              reply_to: "hello@sourcey.com",
              test_email_recipients: [],
            },
    }).operation_plan;
    assert.equal(plan.decision, "refused");
    assert.deepEqual(plan.requests, []);
  }

  const invalid = prepareOperation({
    mode: "act",
    operation: "configure_sender",
    brand_sid: "br_sourcey",
    arguments: {
      from_name: "Sourcey",
      from_email: "hello@sourcey.com",
      reply_to: "not-an-email",
      test_email_recipients: [],
    },
  }).operation_plan;
  assert.equal(invalid.decision, "needs_input");
  assert.deepEqual(invalid.requests, []);
});

test("prepares and normalizes exact brand-scoped sender configuration", () => {
  const requested = {
    from_name: "Sourcey",
    from_email: "hello@sourcey.com",
    reply_to: "hello@sourcey.com",
    test_email_recipients: ["kam@sourcey.com"],
  };
  const plan = prepareOperation({
    mode: "act",
    operation: "configure_sender",
    arguments: requested,
    brand_sid: "br_sourcey",
  }).operation_plan;

  assert.equal(plan.decision, "ready");
  assert.equal(plan.tool, "nitro_configure_account");
  assert.equal(plan.brand_sid, "br_sourcey");
  assert.equal(plan.requests[0].headers["x-brand-sid"], "br_sourcey");
  assert.deepEqual(plan.requests[0].body.params.arguments, requested);

  const { provider_evidence: evidence } = normalizeOperation({
    operation_plan: plan,
    http_execution: {
      responses: [
        {
          id: "nitrosend-configure_sender",
          status: 200,
          ok: true,
          body_digest: "sha256:sender",
          json: mcpPayload(
            {
              sender: {
                from_name: requested.from_name,
                from_email: requested.from_email,
                reply_to: requested.reply_to,
              },
              test_email_recipients: requested.test_email_recipients,
            },
            {
              tool: "nitro_configure_account",
              current_brand: { sid: "br_sourcey", name: "Sourcey" },
            },
          ),
        },
      ],
    },
  });

  assert.equal(evidence.decision, "ok");
  assert.equal(evidence.result.current_brand.sid, "br_sourcey");
  assert.deepEqual(evidence.result.sender, {
    from_name: "Sourcey",
    from_email: "hello@sourcey.com",
    reply_to: "hello@sourcey.com",
  });
  assert.deepEqual(evidence.result.sender_settings, {
    brand_sid: "br_sourcey",
    from_name: "Sourcey",
    from_email: "hello@sourcey.com",
    reply_to: "hello@sourcey.com",
    test_email_recipients: ["kam@sourcey.com"],
  });
});

test("maps consented inline imports without forwarding audit-only fields", () => {
  const { operation_plan: plan } = prepareOperation({
    mode: "act",
    operation: "import_contacts",
    arguments: {
      source_id: "product-signup",
      consent_basis: "First-party signup opt-in",
      records: [{ email: "fixture@example.com" }],
      dry_run: true,
      idempotency_key: "fixture-import",
    },
  });

  const args = plan.requests[0].body.params.arguments;
  assert.equal(args.source_id, undefined);
  assert.equal(args.consent_basis, undefined);
  assert.equal(args.records[0].source, "product-signup");
});

test("maps one reviewed remote image ingest and strips storage capability material", () => {
  const requested = {
    image_url: "https://vendor.example/hero.png",
    description: "Product documentation preview on a blue background",
    filename: "hero.png",
  };
  const plan = prepareOperation({
    mode: "act",
    operation: "ingest_image",
    arguments: requested,
    brand_sid: "br_sourcey",
  }).operation_plan;

  assert.equal(plan.decision, "ready");
  assert.equal(plan.tool, "nitro_ingest");
  assert.deepEqual(plan.requests[0].body.params.arguments, {
    kind: "image",
    ...requested,
  });

  const normalized = normalizeOperation({
    operation_plan: plan,
    http_execution: {
      responses: [{
        id: "nitrosend-ingest_image",
        status: 200,
        ok: true,
        body_digest: "sha256:image",
        json: mcpPayload({
          image_url: "https://api.nitrosend.com/cdn/images/safe/large/hero.png",
          media_url: "https://api.nitrosend.com/cdn/images/safe/large/hero.png",
          description: requested.description,
          signed_id: "opaque-storage-capability",
        }),
      }],
    },
  }).provider_evidence;

  assert.equal(normalized.decision, "ok");
  assert.equal(normalized.result.image_url, "https://api.nitrosend.com/cdn/images/safe/large/hero.png");
  assert.equal(normalized.result.signed_id, undefined);
});

test("rejects incomplete or widened remote image ingest arguments before HTTP", () => {
  for (const arguments_ of [
    { image_url: "https://vendor.example/hero.png" },
    { image_url: "file:///tmp/hero.png", description: "Local file" },
    { image_url: "https://vendor.example/hero.png", description: "Hero", image_data: "bytes" },
  ]) {
    const plan = prepareOperation({
      mode: "act",
      operation: "ingest_image",
      arguments: arguments_,
    }).operation_plan;
    assert.equal(plan.decision, "needs_input");
    assert.deepEqual(plan.requests, []);
  }
});

test("normalizes provider readback and redacts provider-returned secrets", () => {
  const returnedSecret = ["nskey", "live", "secret"].join("_");
  const plan = prepareOperation({ mode: "read", operation: "status", arguments: {} }).operation_plan;
  const { provider_evidence: evidence } = normalizeOperation({
    operation_plan: plan,
    http_execution: {
      responses: [{
        id: "nitrosend:status",
        status: 200,
        ok: true,
        body_digest: "sha256:fixture",
        json: mcpPayload({ data: { id: 42, api_token: returnedSecret } }),
      }],
    },
  });

  assert.equal(evidence.decision, "ok");
  assert.equal(evidence.provider_ref, "nitrosend:status:42");
  assert.equal(evidence.result.data.api_token, "[REDACTED]");
  assert.equal(evidence.evidence.body_digest, "sha256:fixture");
  assert.equal(JSON.stringify(evidence).includes(returnedSecret), false);
});

test("projects credential rejection and local validation as bounded evidence", () => {
  const plan = prepareOperation({ mode: "read", operation: "status", arguments: {} }).operation_plan;
  const rejected = normalizeOperation({
    operation_plan: plan,
    http_execution: {
      responses: [{ id: "nitrosend:status", status: 401, ok: false, body_digest: "sha256:401" }],
    },
  }).provider_evidence;
  assert.equal(rejected.decision, "needs_input");
  assert.match(rejected.blockers[0], /credential/u);

  const blockedPlan = prepareOperation({ mode: "act", operation: "unknown", arguments: {} }).operation_plan;
  const blocked = blockedOperation({ operation_plan: blockedPlan }).provider_evidence;
  assert.equal(blocked.decision, "needs_input");
  assert.equal(blocked.evidence, null);
});

test("preserves redacted MCP error detail as provider evidence", () => {
  const plan = prepareOperation({ mode: "act", operation: "compose_flow", arguments: {} }).operation_plan;
  const returnedSecret = ["nskey", "live", "secret"].join("_");
  const failed = normalizeOperation({
    operation_plan: plan,
    http_execution: {
      responses: [{
        id: "nitrosend-compose_flow",
        status: 200,
        ok: true,
        body_digest: "sha256:error",
        json: {
          jsonrpc: "2.0",
          id: "nitrosend-compose_flow",
          error: {
            code: -32603,
            message: "Internal error",
            data: `Lock wait timeout; token=${returnedSecret}`,
          },
        },
      }],
    },
  }).provider_evidence;

  assert.equal(failed.decision, "provider_error");
  assert.equal(failed.result, null);
  assert.equal(failed.blockers[0], "Internal error: Lock wait timeout; token=[REDACTED]");
  assert.equal(JSON.stringify(failed).includes(returnedSecret), false);
});

test("projects an HTTP 200 MCP tool error as provider failure", () => {
  const plan = prepareOperation({
    mode: "act",
    operation: "plan_checkout",
    arguments: { plan_id: 202, confirm: true, idempotency_key: "account-1-plan-202-v1" },
  }).operation_plan;
  const failed = normalizeOperation({
    operation_plan: plan,
    http_execution: {
      responses: [{
        id: "nitrosend-plan_checkout",
        status: 200,
        ok: true,
        body_digest: "sha256:tool-error",
        json: {
          jsonrpc: "2.0",
          id: "nitrosend-plan_checkout",
          result: {
            isError: true,
            content: [{ type: "text", text: "plan unavailable" }],
          },
        },
      }],
    },
  }).provider_evidence;

  assert.equal(failed.decision, "provider_error");
  assert.deepEqual(failed.blockers, ["plan unavailable"]);
  assert.equal(failed.result.message, "plan unavailable");
});

test("maps all composition surfaces to non-persisting reads", () => {
  for (const [target_type, tool] of [
    ["campaign", "nitro_compose_campaign"],
    ["flow", "nitro_compose_flow"],
    ["template", "nitro_manage_template"],
  ]) {
    const intent = prepareOperation({ mode: "read", operation: "compose_email", arguments: {
      target_type, composition_mode: "intent",
      arguments: { composition_mode: "draft", goal: `Write one ${target_type} email`, idempotency_key: "retry", dry_run: true },
    } }).operation_plan;
    assert.equal(intent.decision, "ready");
    assert.equal(intent.tool, tool);
    assert.deepEqual(intent.requests[0].body.params.arguments, {
      goal: `Write one ${target_type} email`, composition_mode: "intent",
    });

    const validation = prepareOperation({ mode: "read", operation: "compose_email", arguments: {
      target_type, composition_mode: "validate",
      arguments: { composition_mode: "draft", contract_id: `ecc_${target_type}`, idempotency_key: "retry", body: "Candidate" },
    } }).operation_plan;
    assert.equal(validation.decision, "ready");
    assert.equal(validation.tool, tool);
    assert.deepEqual(validation.requests[0].body.params.arguments, {
      contract_id: `ecc_${target_type}`, body: "Candidate", composition_mode: "validate", validate_only: true,
    });
  }
});

test("refuses malformed composition reads before transport", () => {
  for (const arguments_ of [
    { target_type: "message", composition_mode: "intent", arguments: { goal: "No" } },
    { target_type: "campaign", composition_mode: "draft", arguments: { goal: "No" } },
    { target_type: "campaign", composition_mode: "validate", arguments: { body: "Missing contract" } },
    { target_type: "flow", composition_mode: "intent", arguments: { contract_id: "ecc_wrong" } },
    { target_type: "template", composition_mode: "intent", arguments: [] },
  ]) {
    const plan = prepareOperation({ mode: "read", operation: "compose_email", arguments: arguments_ }).operation_plan;
    assert.notEqual(plan.decision, "ready");
    assert.deepEqual(plan.requests, []);
  }
});

test("admits only persistence-ready creative draft mutations", () => {
  for (const [operation, tool] of [
    ["compose_campaign", "nitro_compose_campaign"],
    ["compose_flow", "nitro_compose_flow"],
    ["manage_template", "nitro_manage_template"],
  ]) {
    const valid = { composition_mode: "draft", contract_id: `ecc_${operation}`, idempotency_key: `ecr_${operation}` };
    const plan = prepareOperation({ mode: "act", operation, arguments: valid }).operation_plan;
    assert.equal(plan.decision, "ready");
    assert.equal(plan.tool, tool);
    for (const invalid of [
      { ...valid, composition_mode: "intent" },
      { ...valid, composition_mode: "validate" },
      { ...valid, composition_mode: "generate" },
      { ...valid, contract_id: "" },
      { ...valid, idempotency_key: "" },
    ]) {
      const refused = prepareOperation({ mode: "act", operation, arguments: invalid }).operation_plan;
      assert.equal(refused.decision, "refused");
      assert.deepEqual(refused.requests, []);
    }
  }
});

test("pins plan billing MCP sub-actions and caller retry identity", () => {
  const status = prepareOperation({
    mode: "read",
    operation: "billing_status",
    arguments: {},
  }).operation_plan;
  assert.equal(status.decision, "ready");
  assert.equal(status.tool, "nitro_manage_billing");
  assert.deepEqual(status.requests[0].body.params.arguments, { operation: "status" });

  const plans = prepareOperation({
    mode: "read",
    operation: "billing_plans",
    arguments: {},
  }).operation_plan;
  assert.deepEqual(plans.requests[0].body.params.arguments, { operation: "plans" });

  const checkout = prepareOperation({
    mode: "act",
    operation: "plan_checkout",
    arguments: {
      plan_id: 202,
      confirm: true,
      idempotency_key: "account-1-plan-202-v1",
    },
  }).operation_plan;
  assert.equal(checkout.decision, "ready");
  assert.equal(checkout.tool, "nitro_manage_billing");
  assert.equal(checkout.requests[0].idempotency_key, undefined);
  assert.deepEqual(checkout.requests[0].body.params.arguments, {
    operation: "checkout",
    params: { plan_id: 202, confirm: true },
    idempotency_key: "account-1-plan-202-v1",
  });

  const readback = prepareOperation({
    mode: "read",
    operation: "plan_checkout_status",
    arguments: { purchase_id: 701 },
  }).operation_plan;
  assert.deepEqual(readback.requests[0].body.params.arguments, {
    operation: "checkout_status",
    params: { purchase_id: 701 },
  });

  for (const plan of [status, plans, checkout, readback]) {
    const argumentsValue = plan.requests[0].body.params.arguments;
    assert.equal(
      validateBillingArguments(argumentsValue),
      true,
      `${plan.operation} diverged from deployed nitro_manage_billing schema: ${JSON.stringify(validateBillingArguments.errors)}`,
    );
  }
});

test("binds billing readback to the requested operation and purchase", () => {
  const purchase701 = prepareOperation({
    mode: "read",
    operation: "plan_checkout_status",
    arguments: { purchase_id: 701 },
  }).operation_plan;
  const purchase702 = prepareOperation({
    mode: "read",
    operation: "plan_checkout_status",
    arguments: { purchase_id: 702 },
  }).operation_plan;

  const evidence701 = normalizedEvidence(purchase701, {
    operation: "checkout_status",
    purchase_id: 701,
    status: "awaiting_provider_approval",
  });
  const evidence702 = normalizedEvidence(purchase702, {
    operation: "checkout_status",
    purchase_id: 702,
    status: "active",
  });
  assert.equal(evidence701.decision, "ok");
  assert.equal(evidence701.provider_ref, "nitrosend:plan_purchase:701");
  assert.equal(evidence702.decision, "ok");
  assert.equal(evidence702.provider_ref, "nitrosend:plan_purchase:702");

  const mismatch = normalizedEvidence(purchase701, {
    operation: "checkout_status",
    purchase_id: 702,
    status: "active",
  });
  assert.equal(mismatch.decision, "provider_error");
  assert.equal(mismatch.provider_ref, null);
  assert.deepEqual(mismatch.blockers, ["Nitrosend returned purchase_id 702; expected 701"]);

  const status = prepareOperation({
    mode: "read",
    operation: "billing_status",
    arguments: {},
  }).operation_plan;
  const wrongOperation = normalizedEvidence(status, {
    operation: "checkout",
    purchase_id: 701,
  });
  assert.equal(wrongOperation.decision, "provider_error");
  assert.equal(wrongOperation.provider_ref, null);
  assert.deepEqual(wrongOperation.blockers, ["Nitrosend returned operation checkout; expected status"]);
});

test("refuses widened or prepaid arguments on every plan billing lane", () => {
  const cases = [
    ["read", "billing_status", { operation: "checkout" }],
    ["read", "billing_plans", { amount_cents: 5000 }],
    ["read", "plan_checkout_status", { purchase_id: 701, instrument: "card" }],
    ["act", "plan_checkout", {
      plan_id: 202,
      confirm: true,
      idempotency_key: "account-1-plan-202-v1",
      currency: "USD",
    }],
    ["act", "plan_checkout", {
      plan_id: 202,
      confirm: false,
      idempotency_key: "account-1-plan-202-v1",
    }],
    ["act", "plan_checkout", {
      plan_id: 202,
      confirm: true,
      idempotency_key: "x".repeat(129),
    }],
  ];

  for (const [mode, operation, args] of cases) {
    const plan = prepareOperation({ mode, operation, arguments: args }).operation_plan;
    assert.notEqual(plan.decision, "ready", `${operation} admitted ${JSON.stringify(args)}`);
    assert.deepEqual(plan.requests, []);
  }
});
