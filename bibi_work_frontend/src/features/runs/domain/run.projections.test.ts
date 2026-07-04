import { describe, expect, it } from "vitest";
import type { RunEvent } from "../../../shared/contracts/platform";
import { createEmptyRunProjection } from "./run.types";
import { mergeRunEvents, projectRunEvent, projectRunEvents } from "./run.projections";

const baseEvent = {
  id: "00000000-0000-0000-0000-000000000001",
  tenantId: "10000000-0000-0000-0000-000000000001",
  conversationId: "20000000-0000-0000-0000-000000000001",
  runId: "30000000-0000-0000-0000-000000000001",
  traceId: "trace",
  createdAt: "2026-06-21T00:00:00Z"
} satisfies Omit<RunEvent, "seq" | "eventId" | "type" | "payload">;

function event(partial: Pick<RunEvent, "seq" | "eventId" | "type" | "payload">): RunEvent {
  return { ...baseEvent, ...partial };
}

describe("run event projections", () => {
  it("sorts events by seq and builds a compact run projection", () => {
    const projection = projectRunEvents([
      event({
        seq: 3,
        eventId: "message.completed.1",
        type: "message.completed",
        payload: { message_id: "m1", content: "hello world" }
      }),
      event({
        seq: 1,
        eventId: "run.started.1",
        type: "run.started",
        payload: { status: "running" }
      }),
      event({
        seq: 2,
        eventId: "message.delta.1",
        type: "message.delta",
        payload: { message_id: "m1", delta: "hello" }
      }),
      event({
        seq: 4,
        eventId: "task.updated.1",
        type: "task.updated",
        payload: { task_id: "task-1", title: "检索资料", status: "running" }
      })
    ]);

    expect(projection.status).toBe("running");
    expect(projection.lastSeq).toBe(4);
    expect(projection.messages).toEqual([
      { id: "m1", role: "assistant", content: "hello world", status: "completed" }
    ]);
    expect(projection.tasks[0]).toMatchObject({ id: "task-1", status: "running" });
  });

  it("projects user messages while keeping run lifecycle events out of chat messages", () => {
    const projection = projectRunEvents([
      event({
        seq: 1,
        eventId: "message.completed.user.1",
        type: "message.completed",
        payload: { message_id: "user-1", role: "user", content: "你好" }
      }),
      event({
        seq: 2,
        eventId: "run.queued.1",
        type: "run.queued",
        payload: { status: "queued" }
      }),
      event({
        seq: 3,
        eventId: "run.started.1",
        type: "run.started",
        payload: { status: "running" }
      }),
      event({
        seq: 4,
        eventId: "run.completed.1",
        type: "run.completed",
        payload: { status: "completed" }
      })
    ]);

    expect(projection.status).toBe("completed");
    expect(projection.messages).toEqual([
      { id: "user-1", role: "user", content: "你好", status: "completed" }
    ]);
    expect(projection.runEvents.map((runEvent) => runEvent.type)).toEqual([
      "message.completed",
      "run.queued",
      "run.started",
      "run.completed"
    ]);
  });

  it("deduplicates events using event_id", () => {
    const started = event({
      seq: 1,
      eventId: "tool.call.started.1",
      type: "tool.call.started",
      payload: { tool_call_id: "tool-1", tool_name: "file_search" }
    });
    const projection = projectRunEvent(
      started,
      projectRunEvent(started, createEmptyRunProjection())
    );

    expect(projection.toolCalls).toHaveLength(1);
    expect(projection.events).toHaveLength(1);
  });

  it("projects validated tool result views and drops invalid views", () => {
    const projection = projectRunEvents([
      event({
        seq: 1,
        eventId: "tool.call.completed.1",
        type: "tool.call.completed",
        payload: {
          tool_call_id: "tool-1",
          tool_name: "query_sales",
          output_summary: "ok",
          views: [
            {
              kind: "table",
              columns: [{ key: "region", label: "Region", type: "string" }],
              rows_preview: [{ region: "east" }]
            },
            { kind: "map", format: "geojson" }
          ]
        }
      })
    ]);

    expect(projection.toolCalls[0]).toMatchObject({
      id: "tool-1",
      name: "query_sales",
      outputSummary: "ok"
    });
    expect(projection.toolCalls[0].views).toEqual([
      {
        kind: "table",
        title: undefined,
        columns: [{ key: "region", label: "Region", type: "string" }],
        rowsPreview: [{ region: "east" }],
        dataRef: undefined
      }
    ]);
  });

  it("preserves started tool input details when completion omits them", () => {
    const projection = projectRunEvents([
      event({
        seq: 1,
        eventId: "tool.call.started.1",
        type: "tool.call.started",
        payload: {
          tool_call_id: "tool-1",
          tool_name: "ls",
          input_summary: '{"path":"/local/main/"}'
        }
      }),
      event({
        seq: 2,
        eventId: "tool.call.completed.1",
        type: "tool.call.completed",
        payload: {
          tool_call_id: "tool-1",
          tool_name: "ls",
          output_summary: "2 entries",
          views: [
            {
              kind: "table",
              columns: [{ key: "path", label: "path", type: "string" }],
              rows_preview: [{ path: "/local/main/readme.md" }]
            }
          ]
        }
      })
    ]);

    expect(projection.toolCalls[0]).toMatchObject({
      id: "tool-1",
      name: "ls",
      status: "completed",
      inputSummary: '{"path":"/local/main/"}',
      outputSummary: "2 entries"
    });
  });

  it("merges cached and replayed events by event_id before projection", () => {
    const cached = event({
      seq: 2,
      eventId: "message.delta.1",
      type: "message.delta",
      payload: { message_id: "m1", delta: "llo" }
    });
    const replayed = event({
      seq: 1,
      eventId: "message.delta.0",
      type: "message.delta",
      payload: { message_id: "m1", delta: "he" }
    });

    const merged = mergeRunEvents([cached], [replayed, cached]);

    expect(merged.map((runEvent) => runEvent.seq)).toEqual([1, 2]);
    expect(projectRunEvents(merged).messages[0].content).toBe("hello");
  });

  it("marks streamed messages completed when the run completes", () => {
    const projection = projectRunEvents([
      event({
        seq: 1,
        eventId: "message.delta.1",
        type: "message.delta",
        payload: { message_id: "m1", delta: "hello" }
      }),
      event({
        seq: 2,
        eventId: "run.completed.1",
        type: "run.completed",
        payload: { status: "completed" }
      })
    ]);

    expect(projection.status).toBe("completed");
    expect(projection.messages[0]).toMatchObject({ content: "hello", status: "completed" });
  });

  it("keeps assistant streams from different runs separate when message_id is absent", () => {
    const firstRunId = "30000000-0000-0000-0000-000000000001";
    const secondRunId = "30000000-0000-0000-0000-000000000002";
    const projection = projectRunEvents([
      event({
        seq: 1,
        eventId: "message.delta.1",
        type: "message.delta",
        payload: { run_id: firstRunId, content: "first " }
      }),
      event({
        seq: 2,
        eventId: "message.delta.2",
        type: "message.delta",
        payload: { run_id: firstRunId, content: "reply" }
      }),
      event({
        seq: 3,
        eventId: "run.completed.1",
        type: "run.completed",
        payload: { run_id: firstRunId, status: "completed" }
      }),
      event({
        seq: 4,
        eventId: "message.delta.3",
        type: "message.delta",
        payload: { run_id: secondRunId, content: "second " }
      }),
      event({
        seq: 5,
        eventId: "message.delta.4",
        type: "message.delta",
        payload: { run_id: secondRunId, content: "reply" }
      })
    ]);

    expect(projection.messages).toEqual([
      {
        id: `assistant.${firstRunId}`,
        role: "assistant",
        content: "first reply",
        status: "completed"
      },
      {
        id: `assistant.${secondRunId}`,
        role: "assistant",
        content: "second reply",
        status: "streaming"
      }
    ]);
  });

  it("splits implicit assistant streams after tool calls and keeps timeline order", () => {
    const runId = baseEvent.runId;
    const assistantMessageId = `assistant.${runId}.1`;
    const projection = projectRunEvents([
      event({
        seq: 1,
        eventId: "message.completed.user.1",
        type: "message.completed",
        payload: { message_id: "user-1", role: "user", content: "查看当前目录" }
      }),
      event({
        seq: 2,
        eventId: "message.delta.blank.1",
        type: "message.delta",
        payload: { run_id: runId, content: "\n" }
      }),
      event({
        seq: 3,
        eventId: "tool.call.started.1",
        type: "tool.call.started",
        payload: { run_id: runId, tool_call_id: "tool-1", tool_name: "ls" }
      }),
      event({
        seq: 4,
        eventId: "tool.call.completed.1",
        type: "tool.call.completed",
        payload: { run_id: runId, tool_call_id: "tool-1", tool_name: "ls" }
      }),
      event({
        seq: 5,
        eventId: "message.delta.1",
        type: "message.delta",
        payload: { run_id: runId, content: "当前目录下有 " }
      }),
      event({
        seq: 6,
        eventId: "message.delta.2",
        type: "message.delta",
        payload: { run_id: runId, content: "readme.md" }
      }),
      event({
        seq: 7,
        eventId: "run.completed.1",
        type: "run.completed",
        payload: { run_id: runId, status: "completed" }
      })
    ]);

    expect(projection.messages).toEqual([
      { id: "user-1", role: "user", content: "查看当前目录", status: "completed" },
      {
        id: assistantMessageId,
        role: "assistant",
        content: "当前目录下有 readme.md",
        status: "completed"
      }
    ]);
    expect(projection.timeline).toEqual([
      { kind: "message", id: "user-1" },
      { kind: "tool_call", id: "tool-1" },
      { kind: "message", id: assistantMessageId }
    ]);
  });

  it("projects approval and file change events into inspector state", () => {
    const projection = projectRunEvents([
      event({
        seq: 1,
        eventId: "approval.requested.1",
        type: "approval.requested",
        payload: { approval_id: "approval-1", tool_name: "local_exec", risk_level: "high" }
      }),
      event({
        seq: 2,
        eventId: "file.changed.1",
        type: "file.changed",
        payload: { path: "/workspace/report.md", revision: 2, content_hash: "sha256" }
      })
    ]);

    expect(projection.status).toBe("waiting_approval");
    expect(projection.approvals[0]).toMatchObject({ id: "approval-1", toolName: "local_exec" });
    expect(projection.files[0]).toMatchObject({ path: "/workspace/report.md", revision: 2 });
  });
});
