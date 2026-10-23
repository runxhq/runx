import { describe, expect, it } from "vitest";

import {
  CATALOG_PRESENTATION_PATH,
  RegistryPresentationError,
  assertPublicSkillCatalog,
  composedSkillId,
  readCatalogPresentation,
} from "./catalog-presentation.js";

const PRESENTATION = `---
schema: runx.catalog-presentation.v1
title: Research brief
method:
  - title: Bring the evidence
    description: Supply the question and bounded source material.
  - title: Make sense of it
    description: Separate evidence, inference and unresolved questions.
    skill: ../research
    runner: default
inputs:
  - label: The decision you need to make
    input: question
outputs:
  - label: A recommendation with citations and open questions
example:
  title: Example brief
  path: references/examples/decision-brief.md
stops:
  title: Stops when the evidence runs out.
  description: Missing evidence is reported; the result is not published.
relationships:
  - kind: evidence_source
    id: runx/sourcey
    label: Documentation handoff
---
Research that builds toward a decision.
`;

const context = {
  packagePaths: new Set([CATALOG_PRESENTATION_PATH, "references/examples/decision-brief.md"]),
  runnerNames: ["default"],
  inputNames: ["question"],
  skills: ["../research"],
};

describe("readCatalogPresentation", () => {
  it("projects a valid file into the published presentation", () => {
    expect(readCatalogPresentation(PRESENTATION, context)).toEqual({
      schema: "runx.catalog-presentation.v1",
      title: "Research brief",
      overview: "Research that builds toward a decision.",
      method: [
        { title: "Bring the evidence", description: "Supply the question and bounded source material." },
        { title: "Make sense of it", description: "Separate evidence, inference and unresolved questions.", skill: "../research", runner: "default" },
      ],
      inputs: [{ label: "The decision you need to make", input: "question" }],
      outputs: [{ label: "A recommendation with citations and open questions" }],
      example: { title: "Example brief", path: "references/examples/decision-brief.md" },
      stops: { title: "Stops when the evidence runs out.", description: "Missing evidence is reported; the result is not published." },
      relationships: [{ kind: "evidence_source", id: "runx/sourcey", label: "Documentation handoff" }],
    });
  });

  it("refuses trust and classification fields, missing files, undeclared runners, aliases and duplicate keys", () => {
    const refusal = (source: string, field: string) => {
      expect(() => readCatalogPresentation(source, context)).toThrow(RegistryPresentationError);
      expect(() => readCatalogPresentation(source, context)).toThrow(field);
    };
    refusal(PRESENTATION.replace("title: Research brief", "title: Research brief\nverified: true"), "unknown fields: verified");
    refusal(PRESENTATION.replace("references/examples/decision-brief.md", "references/examples/missing.md"), "example.path");
    refusal(PRESENTATION.replace("runner: default", "runner: elsewhere"), "method[1].runner");
    refusal(`${PRESENTATION}\n${"x".repeat(4_001)}\n`, "overview");
    // References resolve against what the package declares, not only their form.
    refusal(PRESENTATION.replace("skill: ../research", "skill: ../elsewhere"), "method[1].skill");
    // Only a sibling or publisher/name: a package-internal graph has no page to link.
    refusal(PRESENTATION.replace("skill: ../research", "skill: ./graph/plan"), "method[1].skill");
    refusal(PRESENTATION.replace("input: question", "input: topic"), "inputs[0].input");
    refusal(PRESENTATION.replace("schema: runx.catalog-presentation.v1", "schema: runx.catalog-presentation.v2"), "schema");
    refusal(PRESENTATION.replace("input: question", "input: $input.question"), "inputs[0].input");
    refusal(PRESENTATION.replace("title: Research brief", "title: &t Research brief\nstops:\n  title: *t"), "frontmatter");
    refusal(PRESENTATION.replace("title: Research brief", "title: Research brief\ntitle: Again"), "frontmatter");
    refusal(PRESENTATION.replace("path: references/examples/decision-brief.md", "path: ../outside.md"), "example.path");
  });

  it("accepts a file with only its schema: absence is valid", () => {
    expect(readCatalogPresentation("---\nschema: runx.catalog-presentation.v1\n---\n", context))
      .toEqual({ schema: "runx.catalog-presentation.v1" });
  });
});

describe("display titles", () => {
  it("takes a declared title as plain, single-line text of at most 100 characters", () => {
    const declared = (title: string) => readCatalogPresentation(`---\nschema: runx.catalog-presentation.v1\ntitle: ${JSON.stringify(title)}\n---\n`, context).title;
    expect(declared("  Cafe\u0301 brief ")).toBe("Café brief");
    for (const bad of ["", "   ", "Line\nbreak", "**Bold**", "<b>x</b>", "# Heading", "[link](x)", "x".repeat(101), "a\u202Eb"]) {
      expect(() => declared(bad), bad).toThrow(RegistryPresentationError);
    }
  });

  it("takes a card summary as plain, single-line text of at most 160 characters", () => {
    const declared = (summary: string) => readCatalogPresentation(`---\nschema: runx.catalog-presentation.v1\nsummary: ${JSON.stringify(summary)}\n---\n`, context).summary;
    expect(declared(" Investigate a decision and bring back the evidence. ")).toBe("Investigate a decision and bring back the evidence.");
    for (const bad of ["", "Line\nbreak", "**Bold**", "x".repeat(161)]) {
      expect(() => declared(bad), bad).toThrow(RegistryPresentationError);
    }
  });
});

describe("composed skills", () => {
  it("resolves a sibling, as a bare or relative name, under the same publisher", () => {
    expect(composedSkillId("../research", "runx")).toBe("runx/research");
    expect(composedSkillId("research", "runx")).toBe("runx/research");
    expect(composedSkillId("acme/research", "runx")).toBe("acme/research");
  });
});

describe("public skill catalog", () => {
  const publicSkill = (catalog: string | undefined, metadata: Record<string, unknown> = {}) => ({
    metadata: {
      name: "research-brief",
      catalog_visibility: "public",
      category: "research",
      runner_names: ["default"],
      input_names: ["question"],
      skills: ["../research"],
      ...metadata,
    },
    packageFiles: [
      ...(catalog === undefined ? [] : [{ path: CATALOG_PRESENTATION_PATH, content: catalog }]),
      { path: "references/examples/decision-brief.md", content: "# Pilot guest access\n" },
    ],
  });

  it("admits a public skill with a category, an authored title and a summary", () => {
    const withSummary = PRESENTATION.replace("title: Research brief", "title: Research brief\nsummary: Investigate a decision.");
    expect(() => assertPublicSkillCatalog(publicSkill(withSummary))).not.toThrow();
  });

  it("refuses a public skill without a category, a catalog page, a display title or a summary", () => {
    const withSummary = PRESENTATION.replace("title: Research brief", "title: Research brief\nsummary: Investigate a decision.");
    expect(() => assertPublicSkillCatalog(publicSkill(withSummary, { category: undefined }))).toThrow("catalog.category is required");
    expect(() => assertPublicSkillCatalog(publicSkill(withSummary, { category: "not-a-category" }))).toThrow("is not a skill category");
    expect(() => assertPublicSkillCatalog(publicSkill(undefined))).toThrow("must declare a title");
    expect(() => assertPublicSkillCatalog(publicSkill(PRESENTATION))).toThrow("must declare a summary");
    expect(() => assertPublicSkillCatalog(publicSkill(withSummary.replace("title: Research brief", "title: research-brief"))))
      .toThrow("authored display name");
  });

  it("leaves an internal skill to its owner", () => {
    expect(() => assertPublicSkillCatalog(publicSkill(undefined, { catalog_visibility: "internal" }))).not.toThrow();
  });
});
