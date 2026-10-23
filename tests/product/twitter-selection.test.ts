import { describe, expect, it } from "vitest";

import { selectArchivePage } from "../../skills/twitter/twitter-selection.mjs";

describe("twitter archive selection", () => {
  it("binds plans to stable archive content rather than ephemeral artifact handles", () => {
    const first = select("runx:local-artifact:first");
    const second = select("runx:local-artifact:second");

    expect(first).toEqual(second);
    expect(first.evidence_refs).toEqual(["sha256:archive-content"]);
  });
});

function select(artifactUri: string) {
  const selected = selectArchivePage({
    objective: "Delete old reposts",
    principal: "account:@fixture",
    max_acts: 10,
    selection_plan: {
      target: "posts",
      predicate: {
        is_retweet: true,
        before_year: 2026,
      },
      blockers: [],
    },
    runx_page: {
      artifact_uri: artifactUri,
      whole_digest: "sha256:archive-content",
      records: [
        JSON.stringify({
          tweet: {
            id_str: "123",
            full_text: "RT @example: archived",
            created_at: "Wed Jan 01 00:00:00 +0000 2025",
          },
        }),
      ],
      state: null,
      eof: true,
    },
  });
  if (!("twitter_selection_draft" in selected)) {
    throw new Error("fixture archive page did not produce a selection draft");
  }
  return selected.twitter_selection_draft.twitter_plan;
}
