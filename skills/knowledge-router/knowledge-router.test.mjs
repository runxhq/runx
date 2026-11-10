import assert from "node:assert/strict";
import { test } from "node:test";
import validateRoute, { limitsRefusal } from "./knowledge-router.mjs";
import route from "./route.mjs";
const job = {
  question:
    "Which existing source and owner should I inspect before changing artifact lifetime across CLI process exits?",
  available_sources: {
    sources: [
      {
        id: "artifact-store",
        kind: "code",
        summary: "Native local artifact admission, immutable snapshots and run-scoped reads.",
        locator: "crates/runx-runtime/src/services/workspace_files/artifacts.rs",
      },
      {
        id: "author-contract",
        kind: "docs",
        summary: "Declared artifact URI lifetime and page-consumption contract.",
        locator: "docs/skill-author-runtime-contract.md",
      },
      {
        id: "registry-pages",
        kind: "docs",
        summary: "Catalog product presentation and review status.",
        locator: "docs/core-skill-review.md",
      },
    ],
    owners: [
      { id: "native-runtime", domains: ["artifact-lifetime", "local-execution"] },
      { id: "skill-authoring", domains: ["skill-authoring"] },
    ],
    skills: [
      {
        id: "prior-art",
        purpose:
          "Inspect bounded local sources and return an evidence-bound reuse or amendment recommendation.",
      },
      { id: "research", purpose: "Research admitted external sources." },
    ],
  },
  constraints: {
    allowed_sources: ["artifact-store", "author-contract"],
    allowed_skills: ["prior-art"],
  },
};
const proposal = {
  verdict: "routed",
  route: {
    domain: "artifact-lifetime",
    rationale:
      "Inspect the artifact owner and its execution contract before changing lifecycle behavior.",
    source_ids: ["artifact-store", "author-contract"],
  },
  source_matches: [
    {
      source_id: "artifact-store",
      matching_signal: "CLI process exit and snapshot lifetime",
      reason: "Contains the owning artifact admission and page service.",
    },
    {
      source_id: "author-contract",
      matching_signal: "Artifact URI lifetime",
      reason: "Defines the public execution contract that must match the owner.",
    },
  ],
  owner_recommendation: {
    owner_id: "native-runtime",
    rationale: "This catalog assigns artifact-lifetime to the native runtime owner.",
    escalation_required: false,
  },
  next_skill: {
    skill: "prior-art",
    rationale: "Inspect the two exact local sources without rediscovery or a provider.",
  },
};
const digest = "sha256:" + "a".repeat(64);
const run = (p = proposal, constraints = job.constraints) =>
  validateRoute({ ...job, constraints, route_proposal: p, catalog_digest: digest }).knowledge_route;
test("routes the exact catalog and preserves the declared owner and follow-up", () => {
  const result = run();
  assert.equal(result.verdict, "routed");
  assert.deepEqual(result.route.source_ids, proposal.route.source_ids);
  assert.equal(result.owner_recommendation.owner_id, "native-runtime");
  assert.equal(result.next_skill.skill, "prior-art");
});
test("selection limits are enforced, including an explicitly empty allowlist", () => {
  for (const constraints of [
    { allowed_sources: [] },
    { allowed_sources: ["artifact-store"] },
    { excluded_sources: ["artifact-store"] },
    { allowed_owners: [] },
    { allowed_skills: [] },
    { allowed_sources: ["not-in-catalog"] },
  ]) {
    const result = run(proposal, constraints);
    assert.equal(result.verdict, "refused");
    assert.equal(result.validation.valid, false);
  }
  assert.equal(run(proposal, {}).verdict, "routed");
});
test("source matches and owner relationships cannot be invented with valid ids", () => {
  for (const change of [
    (p) => p.source_matches.pop(),
    (p) => p.source_matches.push({ ...p.source_matches[0] }),
    (p) =>
      (p.owner_recommendation = {
        owner_id: "skill-authoring",
        rationale: "Wrong domain",
        escalation_required: false,
      }),
    (p) => {
      p.verdict = "needs_more_context";
      p.route.source_ids = [];
      p.owner_recommendation = null;
      p.next_skill = null;
    },
  ]) {
    const p = structuredClone(proposal);
    change(p);
    assert.equal(run(p, {}).verdict, "refused");
  }
});
test("honest missing context selects nothing", () => {
  const p = {
    verdict: "needs_more_context",
    route: {
      domain: null,
      rationale: "The selected index has no source for this question.",
      source_ids: [],
    },
    source_matches: [],
    owner_recommendation: null,
    next_skill: null,
  };
  assert.equal(run(p, {}).verdict, "needs_more_context");
});
test("limits naming an entry outside the catalog refuse whatever is proposed", () => {
  for (const constraints of [
    { allowed_sources: ["not-in-catalog"] },
    { excluded_sources: ["not-in-catalog"] },
    { allowed_owners: ["not-in-catalog"] },
    { allowed_skills: ["not-in-catalog"] },
  ]) {
    const refusal = limitsRefusal({ ...job, constraints }).knowledge_route;
    assert.equal(refusal.verdict, "refused");
    assert.equal(refusal.route.rationale, "selection limits must reference entries in the supplied catalog");
    const invented = structuredClone(proposal);
    invented.route.source_ids.push("invented-docs");
    invented.source_matches.push({ source_id: "invented-docs", matching_signal: "lifetime", reason: "Invented." });
    assert.deepEqual(run(invented, constraints), refusal);
  }
  assert.equal(limitsRefusal(job), null);
  assert.equal(limitsRefusal({ ...job, constraints: undefined }), null);
});
// A script's ctx whose proposals arrive only as references, as a field past
// the inline bound does: each resolves through ctx.read alone.
const scripted = (proposals) => {
  const calls = [];
  const held = new Map();
  return {
    calls,
    ctx: {
      skill: async (ref, inputs) => {
        calls.push({ ref, inputs: structuredClone(inputs) });
        const reference = { reference: calls.length };
        held.set(reference, proposals[calls.length - 1]);
        return { route_proposal: reference };
      },
      read: async (value) => held.get(value),
    },
  };
};
const routeInputs = (changes = {}) => ({ question: job.question, available_sources: job.available_sources,
  constraints: job.constraints, catalog_digest: digest, ...changes });
test("a refused proposal goes back as feedback, and its revision routes", async () => {
  const invented = structuredClone(proposal);
  invented.route.source_ids = ["artifact-store", "invented-docs"];
  invented.source_matches.push({ source_id: "invented-docs", matching_signal: "lifetime", reason: "Invented." });
  const { ctx, calls } = scripted([invented, proposal]);
  const { knowledge_route } = await route(ctx, routeInputs());
  assert.equal(knowledge_route.verdict, "routed");
  assert.deepEqual(calls.map((call) => call.ref), [".#propose", ".#propose"]);
  assert.equal("feedback" in calls[0].inputs, false);
  assert.deepEqual(calls[1].inputs.feedback, {
    proposal: invented,
    reason: "route contains references outside the supplied catalog",
    unknown_entries: [{ kind: "source", id: "invented-docs" }],
  });
});
test("two retries, and the third refusal is the result", async () => {
  const { ctx, calls } = scripted([0, 1, 2].map((n) => {
    const p = structuredClone(proposal);
    p.route.source_ids = ["registry-pages"];
    p.source_matches = [{ source_id: "registry-pages", matching_signal: "catalog", reason: "Attempt " + n }];
    return p;
  }));
  const { knowledge_route } = await route(ctx, routeInputs());
  assert.equal(calls.length, 3);
  assert.equal(knowledge_route.verdict, "refused");
  assert.equal(knowledge_route.route.rationale, "route violates allowed_sources");
  assert.equal(calls[2].inputs.feedback.proposal.source_matches[0].reason, "Attempt 1");
});
test("limits outside the catalog refuse before any proposal", async () => {
  const { ctx, calls } = scripted([]);
  const { knowledge_route } = await route(ctx, routeInputs({ constraints: { allowed_owners: ["not-in-catalog"] } }));
  assert.equal(calls.length, 0);
  assert.equal(knowledge_route.verdict, "refused");
  assert.equal(knowledge_route.route.rationale, "selection limits must reference entries in the supplied catalog");
});
