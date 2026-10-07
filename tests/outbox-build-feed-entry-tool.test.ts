import { spawnSync } from "node:child_process";
import path from "node:path";

import { describe, expect, it } from "vitest";

const toolPath = path.resolve("tools/outbox/build_feed_entry/run.mjs");
const fakeGithubToken = ["ghp", "123456789012345678901234567890123456"].join("_");

describe("outbox.build_feed_entry tool", () => {
  it("packages a durable feed entry message with PR and merge-gate context", () => {
    const result = runTool({
      task_id: "fixture-task",
      thread_title: "Fix fixture behavior",
      thread_locator: "github://example/repo/issues/123",
      target_repo: "example/repo",
      harness_context: {
        harness: {
          schema: "runx.harness.v1",
          harness_id: "harness_fixture_123",
          state: "running",
        },
        signal: {
          schema: "runx.signal.v1",
          signal_id: "sig_fixture_123",
          title: "Fix fixture behavior",
          source_ref: {
            type: "thread",
            uri: "github://example/repo/issues/123",
          },
          thread_ref: {
            type: "thread",
            uri: "github://example/repo/issues/123",
          },
          fingerprint: {
            value: "sha256:fixture-123",
          },
        },
        decision: {
          schema: "runx.decision.v1",
          decision_id: "dec_fixture_123",
          choice: "open",
          justification: {
            summary: "The request is bounded and reproducible.",
          },
        },
      },
      build_result: {
        passed: 3,
        failed: 0,
      },
      review_result: {
        verdict: "pass",
        findings: [
          {
            id: "non-blocking-fixture",
            severity: "low",
            blocks_completion: false,
          },
        ],
      },
      completion_result: {
        status: "completed",
        title: "Fix fixture behavior",
      },
      status_snapshot: {
        status: "completed",
      },
      pull_request_outbox_entry: {
        kind: "pull_request",
        locator: "https://github.com/example/repo/pull/77",
        metadata: {
          repo: "example/repo",
          branch: "fixture-task",
          base: "main",
        },
      },
      push_result: {
        pull_request: {
          url: "https://github.com/example/repo/pull/77",
        },
      },
    });

    expect(result.feed_entry.data).toMatchObject({
      thread_locator: "github://example/repo/issues/123",
      title: "Fix fixture behavior",
    });
    expect(result.feed_entry.data.milestones).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ kind: "accepted" }),
        expect.objectContaining({ kind: "triaged" }),
        expect.objectContaining({ kind: "spec_ready" }),
        expect.objectContaining({ kind: "build_started", status: "passed" }),
        expect.objectContaining({ kind: "review_requested", status: "passed" }),
        expect.objectContaining({ kind: "change_request_created", status: "ready" }),
        expect.objectContaining({ kind: "human_gate", status: "ready" }),
        expect.objectContaining({ kind: "final_outcome", status: "pending" }),
      ]),
    );
    expect(result.outbox_entry).toMatchObject({
      entry_id: "message:fixture-task:human_gate",
      kind: "message",
      status: "proposed",
      thread_locator: "github://example/repo/issues/123",
      metadata: {
        schema_version: "runx.outbox-entry.feed-entry.v1",
        workflow: "issue-to-pr",
        milestone_kind: "human_gate",
        outbox_receipt_id: expect.stringMatching(/^feed:issue-to-pr:fixture-task:human_gate:[a-f0-9]{20}$/),
        source_thread: {
          required: true,
          publish_mode: "reply",
          missing_behavior: "fail_closed",
          thread_locator: "github://example/repo/issues/123",
        },
        body_markdown: expect.stringContaining("PR: https://github.com/example/repo/pull/77"),
      },
    });
    expect(result.outbox_entry.metadata.body_markdown).toContain("Human merge gate");
    expect(result.outbox_entry.metadata.body_markdown).toContain("Harness: harness_fixture_123");
    expect(result.outbox_entry.metadata.body_markdown).toContain("Fingerprint: sha256:fixture-123");
    expect(result.outbox_entry.metadata.body_markdown).toContain("Blocking findings: 0");
    expect(result.outbox_entry.metadata.body_markdown).toContain("No final provider outcome has been observed yet");
  });

  it("fails closed when no source thread locator is available", () => {
    const result = spawnSync("node", [toolPath], {
      cwd: path.resolve("."),
      encoding: "utf8",
      env: {
        ...process.env,
        RUNX_INPUTS_JSON: JSON.stringify({
          task_id: "fixture-task",
          build_result: {
            passed: 1,
            failed: 0,
          },
          review_result: {
            verdict: "pass",
          },
          completion_result: {
            status: "completed",
            title: "Fix fixture behavior",
          },
        }),
      },
    });

    expect(result.status).toBe(1);
    expect(result.stderr).toContain("source thread locator is required");
  });

  it("packages observed merged provider outcomes as a final source-thread update", () => {
    const result = runTool({
      task_id: "fixture-task",
      thread_title: "Fix fixture behavior",
      thread_locator: "github://example/repo/issues/123",
      build_result: {
        passed: 3,
        failed: 0,
      },
      review_result: {
        verdict: "pass",
      },
      completion_result: {
        status: "completed",
        title: "Fix fixture behavior",
      },
      pull_request_outbox_entry: {
        kind: "pull_request",
        locator: "https://github.com/example/repo/pull/77",
        status: "closed",
        metadata: {
          provider_outcome: "merged",
          merged_at: "2026-05-14T12:00:00Z",
          branch: "fixture-task",
          base: "main",
        },
      },
    });

    expect(result.feed_entry.data.milestones).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: "final_outcome",
          status: "completed",
          summary: "Provider outcome observed: merged.",
        }),
      ]),
    );
    expect(result.outbox_entry).toMatchObject({
      entry_id: "message:fixture-task:final_outcome",
      kind: "message",
      title: "Issue-to-PR outcome",
      metadata: {
        milestone_kind: "final_outcome",
        body_markdown: expect.stringContaining("Provider outcome observed: merged."),
      },
    });
    expect(result.outbox_entry.metadata.body_markdown).toContain("Merged at: 2026-05-14T12:00:00Z");
  });

  it("packages observed closed provider outcomes from refreshed PR state", () => {
    const result = runTool({
      task_id: "fixture-task",
      thread_title: `Fix fixture behavior ${fakeGithubToken}`,
      thread_locator: "github://example/repo/issues/123",
      build_result: {
        passed: 3,
        failed: 0,
      },
      review_result: {
        verdict: "pass",
      },
      completion_result: {
        status: "completed",
        title: "Fix fixture behavior",
      },
      pull_request_outbox_entry: {
        kind: "pull_request",
        locator: "https://github.com/example/repo/pull/77",
        metadata: {
          branch: "fixture-task",
          base: "main",
        },
      },
      push_result: {
        pull_request: {
          url: "https://github.com/example/repo/pull/77",
          state: "CLOSED",
        },
      },
    });

    expect(result.feed_entry.data.milestones).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: "final_outcome",
          status: "completed",
          summary: "Provider outcome observed: closed.",
        }),
      ]),
    );
    expect(result.outbox_entry).toMatchObject({
      entry_id: "message:fixture-task:final_outcome",
      metadata: {
        milestone_kind: "final_outcome",
        body_markdown: expect.stringContaining("Provider state: CLOSED"),
      },
    });
  });

  it("redacts local paths and token-shaped values from source-thread story output", () => {
    const result = runTool({
      task_id: "fixture-task",
      thread_title: "Fix fixture behavior",
      thread_locator: "/Users/kam/dev/runx/thread.json",
      build_result: {
        passed: 1,
        failed: 0,
      },
      review_result: {
        verdict: "pass",
      },
      completion_result: {
        status: "completed",
        title: "Fix fixture behavior",
      },
      pull_request_outbox_entry: {
        kind: "pull_request",
        locator: "https://github.com/example/repo/pull/77",
        metadata: {
          branch: "fixture-task",
          base: "main",
        },
      },
      push_result: {
        status: "pushed",
        pull_request: {
          url: "https://github.com/example/repo/pull/77",
        },
      },
    });

    expect(result.outbox_entry.metadata.body_markdown).not.toContain("/Users/kam");
    expect(result.outbox_entry.metadata.body_markdown).toContain("[local-path]");
    expect(result.outbox_entry.metadata.body_markdown).not.toContain(fakeGithubToken);
  });

  it("carries trusted existing provider state for story refreshes", () => {
    const result = runTool({
      task_id: "fixture-task",
      thread_title: "Fix fixture behavior",
      thread_locator: "github://example/repo/issues/123",
      thread: {
        kind: "runx.thread.v1",
        adapter: {
          type: "github",
          adapter_ref: "example/repo#issue/123",
        },
        thread_kind: "signal",
        thread_locator: "github://example/repo/issues/123",
        entries: [],
        decisions: [],
        outbox: [
          {
            entry_id: "message:fixture-task:merge_gate",
            kind: "message",
            locator: "https://github.com/example/repo/issues/123#issuecomment-1000",
            status: "published",
            thread_locator: "github://example/repo/issues/123",
            metadata: {
              schema_version: "runx.outbox-entry.feed-entry.v1",
              milestone_kind: "merge_gate",
              channel: "github_issue_comment",
              comment_id: "1000",
              outbox_receipt_id: "receipt-fixture-story",
              body_markdown: "Old story body.",
            },
          },
        ],
        source_refs: [],
      },
      build_result: {
        passed: 1,
        failed: 0,
      },
      review_result: {
        verdict: "pass",
      },
      completion_result: {
        status: "completed",
        title: "Fix fixture behavior",
      },
      pull_request_outbox_entry: {
        kind: "pull_request",
        locator: "https://github.com/example/repo/pull/77",
      },
    });

    expect(result.outbox_entry).toMatchObject({
      entry_id: "message:fixture-task:human_gate",
      locator: "https://github.com/example/repo/issues/123#issuecomment-1000",
      metadata: {
        milestone_kind: "human_gate",
        comment_id: "1000",
        outbox_receipt_id: "receipt-fixture-story",
        body_markdown: expect.stringContaining("PR: https://github.com/example/repo/pull/77"),
      },
    });
    expect(result.outbox_entry.metadata.body_markdown).not.toContain("Old story body.");
  });

  it("legacy_published_refresh preserves_comment_id preserves_locator preserves_receipt_ref writes_canonical_milestone_id no_duplicate_comment", () => {
    const result = runTool({
      task_id: "fixture-task",
      thread_title: "Fix fixture behavior",
      thread_locator: "github://example/repo/issues/123",
      thread: {
        kind: "runx.thread.v1",
        adapter: {
          type: "github",
          adapter_ref: "example/repo#issue/123",
        },
        thread_kind: "signal",
        thread_locator: "github://example/repo/issues/123",
        entries: [],
        decisions: [],
        outbox: [
          {
            entry_id: "message:fixture-task:merge_gate",
            kind: "message",
            locator: "https://github.com/example/repo/issues/123#issuecomment-1000",
            status: "published",
            thread_locator: "github://example/repo/issues/123",
            metadata: {
              schema_version: "runx.outbox-entry.feed-entry.v1",
              milestone_kind: "merge_gate",
              channel: "github_issue_comment",
              comment_id: "1000",
              outbox_receipt_id: "receipt-fixture-story",
            },
          },
        ],
        source_refs: [],
      },
      build_result: {
        passed: 1,
        failed: 0,
      },
      review_result: {
        verdict: "pass",
      },
      completion_result: {
        status: "completed",
        title: "Fix fixture behavior",
      },
      pull_request_outbox_entry: {
        kind: "pull_request",
        locator: "https://github.com/example/repo/pull/77",
        metadata: {
          provider_outcome: "merged",
          merged_at: "2026-05-14T12:00:00Z",
        },
      },
    });

    expect(result.outbox_entry).toMatchObject({
      entry_id: "message:fixture-task:final_outcome",
      locator: "https://github.com/example/repo/issues/123#issuecomment-1000",
      metadata: {
        milestone_kind: "final_outcome",
        comment_id: "1000",
        outbox_receipt_id: "receipt-fixture-story",
        body_markdown: expect.stringContaining("Provider outcome observed: merged."),
      },
    });
  });

  it("refreshes a file-backed merge-gate story outcome without receipt metadata", () => {
    const result = runTool({
      task_id: "fixture-task",
      thread_title: "Fix fixture behavior",
      thread_locator: "local://provider/issues/123",
      thread: {
        kind: "runx.thread.v1",
        adapter: {
          type: "file",
          adapter_ref: "/tmp/thread.json",
        },
        thread_kind: "signal",
        thread_locator: "local://provider/issues/123",
        entries: [],
        decisions: [],
        outbox: [
          {
            entry_id: "message:fixture-task:merge_gate",
            kind: "message",
            locator: "file://fixture-thread.json#outbox/message%3Afixture-task%3Amerge_gate",
            status: "published",
            thread_locator: "local://provider/issues/123",
            metadata: {
              schema_version: "runx.outbox-entry.feed-entry.v1",
              milestone_kind: "merge_gate",
              body_markdown: "Old story body.",
            },
          },
        ],
        source_refs: [],
      },
      build_result: {
        passed: 1,
        failed: 0,
      },
      review_result: {
        verdict: "pass",
      },
      completion_result: {
        status: "completed",
        title: "Fix fixture behavior",
      },
      pull_request_outbox_entry: {
        kind: "pull_request",
        locator: "https://github.com/example/repo/pull/77",
        metadata: {
          provider_outcome: "closed",
          state: "CLOSED",
        },
      },
    });

    expect(result.outbox_entry).toMatchObject({
      entry_id: "message:fixture-task:final_outcome",
      locator: "file://fixture-thread.json#outbox/message%3Afixture-task%3Amerge_gate",
      metadata: {
        milestone_kind: "final_outcome",
        body_markdown: expect.stringContaining("Provider outcome observed: closed."),
      },
    });
  });
});

function runTool(inputs: Readonly<Record<string, unknown>>) {
  const result = spawnSync("node", [toolPath], {
    cwd: path.resolve("."),
    encoding: "utf8",
    env: {
      ...process.env,
      RUNX_INPUTS_JSON: JSON.stringify(inputs),
    },
  });
  expect(result.status).toBe(0);
  if (result.status !== 0) {
    throw new Error(result.stderr || result.stdout || "tool failed");
  }
  return JSON.parse(result.stdout);
}
