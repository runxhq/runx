import { existsSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { createHash } from "node:crypto";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

import {
  finalize,
  prepare,
  prepareSubjects,
} from "../skills/operator-inbox/graph/plan/plan.mjs";

const SKILL_PATH = path.resolve("skills/operator-inbox");
const QUERY_DIGEST = `sha256:${"0".repeat(64)}`;
const runtimeGlobal = globalThis as typeof globalThis & {
  Runx?: { readonly parseUrl: (value: string) => URL };
};
const previousRunx = runtimeGlobal.Runx;

beforeAll(() => {
  runtimeGlobal.Runx = Object.freeze({ parseUrl: (value: string) => new URL(value) });
});

afterAll(() => {
  if (previousRunx === undefined) {
    delete runtimeGlobal.Runx;
  } else {
    runtimeGlobal.Runx = previousRunx;
  }
});

describe("operator-inbox skill", () => {
  it("preserves a disposition for old history and reopens only for newer external work", () => {
    const initialTransition = planAction(undefined, 0, message({
      messageLocator: "slack://workspace/analytics/100.1",
      occurredAt: "2026-07-14T09:00:00.000Z",
      authorId: "george",
      preview: "Can you check the analytics claim?",
    }));
    const initial = actionFrom(initialTransition);
    expect(initial).toMatchObject({
      schema: "runx.operator_inbox.action.v1",
      status: "open",
      requester: { external_id: "george", display_name: "George" },
      conversation: { external_id: "analytics", display_name: "analytics", type: "channel" },
      triage: { kind: "direct_mention" },
    });

    const resolved = actionFrom(plan({
      operation: "disposition",
      expected_version: 1,
      current_action: initial,
      observed_at: "2026-07-14T10:00:00.000Z",
      disposition: {
        status: "resolved",
        actor: "Kam",
        reason: "Addressed in the sending-at-scale article",
        evidence_url: "https://example.com/sending-at-scale",
      },
    }));
    expect(resolved).toMatchObject({
      status: "resolved",
      disposition: {
        actor: "Kam",
        covered_occurrence_at: "2026-07-14T09:00:00.000Z",
      },
    });

    const oldHistoryTransition = planAction(resolved, 2, message({
      messageLocator: "slack://workspace/analytics/99.9",
      occurredAt: "2026-07-14T08:30:00.000Z",
      authorId: "george",
      preview: "An older reminder",
    }));
    const oldHistory = actionFrom(oldHistoryTransition);
    expect(oldHistoryTransition.idempotency_key).not.toBe(initialTransition.idempotency_key);
    expect(oldHistory).toMatchObject({
      status: "resolved",
      latest_message: { message_locator: "slack://workspace/analytics/100.1" },
      disposition: { reason: "Addressed in the sending-at-scale article" },
    });

    const newer = actionFrom(planAction(oldHistory, 3, message({
      messageLocator: "slack://workspace/analytics/102.1",
      occurredAt: "2026-07-14T09:30:00.000Z",
      authorId: "nick",
      authorName: "Nick",
      preview: "A newer unseen follow-up",
    })));
    expect(newer).toMatchObject({
      status: "open",
      requester: { external_id: "george", display_name: "George" },
      latest_message: { preview: "A newer unseen follow-up" },
    });
    expect(newer).not.toHaveProperty("disposition");
  });

  it("records resumable bounded scan checkpoints", () => {
    const transition = plan({
      operation: "scan_page",
      expected_version: 3,
      observed_at: "2026-07-14T09:05:00.000Z",
      scan: {
        scan_id: "scan-1",
        provider: "slack",
        query_digest: QUERY_DIGEST,
        page_index: 4,
        status: "truncated",
        next_cursor: "next-page",
        started_at: "2026-07-14T09:00:00.000Z",
      },
      messages: [message({
        messageLocator: "slack://workspace/analytics/100.1",
        occurredAt: "2026-07-14T09:00:00.000Z",
        authorId: "george",
        preview: "Bounded",
      })],
    });
    expect(transition).toMatchObject({
      expected_version: 3,
      event: {
        type: "operator_inbox.scan.truncated",
        payload: {
          scan: { page_index: 4, next_cursor: "next-page", status: "truncated" },
          messages: [expect.objectContaining({ preview: "Bounded" })],
        },
      },
    });

    expect(() => plan({
      operation: "scan_page",
      expected_version: 0,
      observed_at: "2026-07-14T09:05:00.000Z",
      scan: {
        scan_id: "scan-too-many",
        provider: "slack",
        query_digest: QUERY_DIGEST,
        page_index: 1,
        status: "complete",
      },
      messages: Array.from({ length: 21 }, (_, index) => message({
        messageLocator: `slack://workspace/analytics/${index}`,
        occurredAt: "2026-07-14T09:00:00.000Z",
        authorId: "george",
        preview: "Bounded",
      })),
    })).toThrow("at most 20");
  });

  it("rejects operator-authored action observations", () => {
    expect(() => plan({
      operation: "action_observation",
      expected_version: 0,
      observed_at: "2026-07-14T09:05:00.000Z",
      triage: triage(),
      message: message({
        messageLocator: "slack://workspace/analytics/100.1",
        occurredAt: "2026-07-14T09:00:00.000Z",
        authorId: "operator-1",
        preview: "I handled this",
      }),
    })).toThrow("operator-authored messages");
  });

  it("composes graph writes through the case's own local SQLite data source", () => {
    const workspace = mkdtempSync(path.join(os.tmpdir(), "runx-operator-inbox-"));
    try {
      const journey = path.join(SKILL_PATH, "fixtures", "action-lifecycle.yaml");
      const result = spawnSync(nativeRunxBinaryForTest(), ["harness", journey, "--json"], {
        cwd: workspace,
        encoding: "utf8",
        env: {
          ...process.env,
          RUNX_CWD: workspace,
        },
      });
      expect(result.status, result.stderr).toBe(0);
      expect(JSON.parse(result.stdout)).toMatchObject({
        schema: "runx.receipt.v1",
        seal: { disposition: "closed" },
      });
      // The case writes to a store of its own; the operator's workspace store
      // is never touched.
      expect(readdirSync(path.join(workspace, ".runx"))).not.toContain("store.sqlite");
    } finally {
      rmSync(workspace, { recursive: true, force: true });
    }
  });
});

function planAction(currentAction: Record<string, unknown> | undefined, expectedVersion: number, observedMessage: Message) {
  return plan({
    operation: "action_observation",
    expected_version: expectedVersion,
    ...(currentAction ? { current_action: currentAction } : {}),
    observed_at: "2026-07-14T11:00:00.000Z",
    message: observedMessage,
    triage: triage(),
  });
}

function triage() {
  return { kind: "direct_mention", reason: "The connected operator was directly mentioned." };
}

function plan(inputs: Record<string, unknown>): Transition {
  const subjects = prepareSubjects(inputs);
  const draft = prepare({
    ...inputs,
    action_id_digest: sha256(subjects.action_id_subject),
  });
  return finalize({
    ...draft,
    idempotency_digest: sha256(draft.idempotency_binding.subject),
  }).transition as Transition;
}

function sha256(value: string): string {
  return `sha256:${createHash("sha256").update(value).digest("hex")}`;
}

function actionFrom(transition: Transition): Record<string, unknown> {
  return ((transition.event.payload as { action: Record<string, unknown> }).action);
}

function message({
  messageLocator,
  occurredAt,
  authorId,
  authorName = "George",
  preview,
}: {
  readonly messageLocator: string;
  readonly occurredAt: string;
  readonly authorId: string;
  readonly authorName?: string;
  readonly preview: string;
}): Message {
  return {
    provider: "slack",
    external_tenant_ref: "workspace",
    connected_subject_ref: "operator-1",
    message_locator: messageLocator,
    thread_locator: "slack://workspace/analytics/thread-100",
    author: { external_id: authorId, display_name: authorName },
    conversation: { external_id: "analytics", display_name: "analytics", type: "channel" },
    occurred_at: occurredAt,
    preview,
    permalink: "https://example.slack.com/archives/analytics/p100",
    context: [],
  };
}

function nativeRunxBinaryForTest(): string {
  const configured = process.env.RUNX_RUST_CLI_BIN;
  if (configured) return configured;
  const candidate = path.resolve("crates/target/debug/runx");
  return existsSync(candidate) ? candidate : "runx";
}

type Message = Record<string, unknown>;

type Transition = {
  readonly idempotency_key: string;
  readonly event: Record<string, unknown> & { readonly payload: unknown };
};
