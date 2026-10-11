function authority(scopes) {
  return {
    requested: scopes,
    provided: "fixture_only",
  };
}

function gate(approvalRequired, approvalGate, stopReason) {
  return {
    approval_required: approvalRequired,
    approval_gate: approvalGate,
    stop_reason: stopReason,
  };
}

function handoff({ interfaceName, laneRef, runnerRef = null, commandHint = null }) {
  return {
    interface: interfaceName,
    lane_ref: laneRef,
    runner_ref: runnerRef,
    command_hint: commandHint,
  };
}

function evidence(inputsRequired, readbacks, receiptRefs = []) {
  return {
    inputs_required: inputsRequired,
    readbacks,
    receipt_refs: receiptRefs,
  };
}

export default function routeLane(inputs) {
const lane = String(inputs.lane || "classify");
const signal = String(inputs.signal || "").trim();
const operatorContext = String(inputs.operator_context || "").trim();

const laneDetails = {
  classify: {
    status: "ready",
    decision: "route",
    kind: "router",
    consequence: "read_only",
    summary: "Classify the signal before any lane receives authority.",
    why: "A business signal can touch docs, release, work, outreach, money, and proof. Classification keeps that fanout explicit instead of letting the agent improvise.",
    authority: authority([]),
    gate: gate(false, null, null),
    handoff: handoff({
      interfaceName: "graph",
      laneRef: "business-ops",
      runnerRef: "main",
      commandHint: "runx skill business-ops -i signal=\"...\"",
    }),
    evidence: evidence(
      ["signal", "operator_context_if_available"],
      ["lane packets for docs, release, issue, outreach, spend, and proof"],
    ),
    risks: ["over-routing a vague signal", "mistaking fixture packets for provider effects"],
    next: ["sourcey", "release.prepare", "issue-to-pr", "outreach.plan", "spend.quote", "receipt-audit"],
  },
  sourcey: {
    status: "ready",
    decision: "prepare",
    kind: "docs",
    consequence: "draft",
    summary: "Prepare documentation or launch-note work from repo evidence before publishing claims.",
    why: "Docs are usually safe to draft, but public claims should be grounded in source files, release state, and receipts.",
    authority: authority(["repo.read", "docs.draft"]),
    gate: gate(false, null, "publish waits for a docs or release approval lane"),
    handoff: handoff({
      interfaceName: "skill",
      laneRef: "sourcey",
      runnerRef: "sourcey",
      commandHint: "runx skill sourcey ...",
    }),
    evidence: evidence(
      ["repo evidence", "current docs", "claim being made"],
      ["diff or rendered docs preview", "source references", "public URL after publish"],
    ),
    risks: ["ungrounded marketing claims", "stale docs", "public proof missing after publish"],
    next: ["approval.docs_publish", "receipt-audit"],
  },
  "release.prepare": {
    status: "awaiting_approval",
    decision: "prepare",
    kind: "release",
    consequence: "live_mutation",
    summary: "Build a release packet with checks, changelog, risk notes, and unresolved gates.",
    why: "Release prep is useful without authority. Publishing tags, packages, or deploys is a separate consequential act.",
    authority: authority(["repo.read", "release.prepare"]),
    gate: gate(true, "approval.release_publish", "tag, package, deploy, and announcement steps are not authorized by this fixture"),
    handoff: handoff({
      interfaceName: "skill",
      laneRef: "release",
      runnerRef: "prepare",
      commandHint: "runx skill release -i objective=\"...\"",
    }),
    evidence: evidence(
      ["version", "changelog", "checks", "release target"],
      ["green CI", "package dry-run", "tag or registry readback after approval"],
    ),
    risks: ["version drift", "publishing before checks finish", "site or changelog stale after release"],
    next: ["approval.release_publish", "receipt-audit"],
  },
  "issue-to-pr": {
    status: "awaiting_approval",
    decision: "prepare",
    kind: "work",
    consequence: "live_mutation",
    summary: "Turn the signal into a scoped issue or PR handoff, with review and merge held separately.",
    why: "Implementation work can be proposed and reviewed, but repo mutation and merge authority need explicit project gates.",
    authority: authority(["repo.read", "work.plan"]),
    gate: gate(true, "approval.pr_create_or_merge", "opening PRs, pushing branches, and merging are project mutations"),
    handoff: handoff({
      interfaceName: "skill",
      laneRef: "issue-to-pr",
      runnerRef: "issue-to-pr",
      commandHint: "runx skill issue-to-pr ...",
    }),
    evidence: evidence(
      ["issue or objective", "repo context", "acceptance criteria"],
      ["branch or PR URL", "review result", "merge receipt after approval"],
    ),
    risks: ["scope creep", "unreviewed merge", "claiming completion without tests or review"],
    next: ["review", "merge_gate", "receipt-audit"],
  },
  "outreach.plan": {
    status: "awaiting_approval",
    decision: "draft",
    kind: "outreach",
    consequence: "public_send",
    summary: "Plan outbound, customer, or operator communication, then stop before live delivery.",
    why: "Real outreach needs principal, audience, content digest, consent, provider readiness, and human approval. The core graph stays provider-neutral; vendor details belong in the selected provider skill and its connector.",
    authority: authority(["comms.draft"]),
    gate: gate(true, "approval.send", "live send, campaign schedule, broad audience, or public post requires a send gate"),
    handoff: handoff({
      interfaceName: "skill",
      laneRef: "send-as -> provider.send",
      runnerRef: "send-as plan, then the selected vendor-specific send runner",
      commandHint: "runx skill send-as ...; runx skill <provider-send-skill> <send-runner> ...",
    }),
    evidence: evidence(
      ["principal", "audience", "content digest", "consent basis", "provider status"],
      ["provider preflight", "test send result", "delivery receipt after approval"],
    ),
    risks: ["wrong audience", "missing consent", "mutable content", "provider send treated as a draft"],
    next: ["send-as", "provider.send", "approval.send", "receipt-audit"],
  },
  "spend.quote": {
    status: "awaiting_approval",
    decision: "quote",
    kind: "spend",
    consequence: "money_movement",
    summary: "Quote spend intent, amount, cap, recipient, and rail, then stop before settlement.",
    why: "Money movement is never implied by a business plan. Quote, cap, approval, settlement, and readback stay distinct.",
    authority: authority(["spend.quote"]),
    gate: gate(true, "approval.spend", "settlement requires amount, recipient, rail, cap, and approval"),
    handoff: handoff({
      interfaceName: "skill",
      laneRef: "spend | charge | payout | refund",
      runnerRef: "spend.mock or rail-specific payment runner",
      commandHint: "runx skill spend ...",
    }),
    evidence: evidence(
      ["amount", "cap", "recipient", "rail", "purpose"],
      ["quote", "approval ref", "settlement transaction or provider readback after approval"],
    ),
    risks: ["network mismatch", "recipient ambiguity", "uncapped spend", "settlement marked complete from local state only"],
    next: ["approval.spend", "settlement_lane", "receipt-audit"],
  },
  "receipt-audit": {
    status: "ready",
    decision: "verify",
    kind: "proof",
    consequence: "proof",
    summary: "State which receipts and provider readbacks would prove the lane chain after execution.",
    why: "The graph receipt proves routing. External effects need child receipts and provider readbacks so another agent can replay what happened.",
    authority: authority(["receipt.read"]),
    gate: gate(false, null, null),
    handoff: handoff({
      interfaceName: "skill",
      laneRef: "receipt-audit | run-history | receipt-query",
      runnerRef: "verify",
      commandHint: "runx skill receipt-audit ...",
    }),
    evidence: evidence(
      ["graph receipt", "child receipt refs", "provider readbacks"],
      ["receipt chain", "effect packets", "public evidence URL where applicable"],
    ),
    risks: ["confusing route proof with effect proof", "missing provider readback", "unpublished receipt"],
    next: ["history", "verify", "publish evidence if intended"],
  },
};

function missingSignalPacket() {
  return {
    schema: "runx.business_ops_lane.v1",
    lane,
    signal,
    operator_context: operatorContext || null,
    status: "needs_input",
    decision: "stop",
    kind: "router",
    consequence: "read_only",
    summary: "No business signal was provided.",
    why: "The graph cannot choose safe lanes without a concrete objective.",
    authority: authority([]),
    gate: gate(false, null, null),
    handoff: handoff({
      interfaceName: "graph",
      laneRef: "business-ops",
      runnerRef: "main",
      commandHint: "runx skill business-ops -i signal=\"...\"",
    }),
    evidence: evidence(["signal"], []),
    risks: ["routing vague or empty work into consequential lanes"],
    next: [],
  };
}

const detail = laneDetails[lane];
const lanePacket = signal
  ? {
      schema: "runx.business_ops_lane.v1",
      lane,
      signal,
      operator_context: operatorContext || null,
      ...(detail ?? {
        status: "needs_input",
        decision: "stop",
        kind: "router",
        consequence: "read_only",
        summary: `Unknown lane: ${lane}`,
        why: "The requested lane is not part of the business-ops graph.",
        authority: authority([]),
        gate: gate(false, null, null),
        handoff: handoff({ interfaceName: "graph", laneRef: "business-ops" }),
        evidence: evidence(["known lane"], []),
        risks: ["private or misspelled lane invoked without a contract"],
        next: [],
      }),
    }
  : missingSignalPacket();

  return { lane_packet: lanePacket };
}
