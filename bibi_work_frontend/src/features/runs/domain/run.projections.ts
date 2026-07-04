import type { JsonRecord, JsonValue } from "../../../shared/types/json";
import { asRecord, stringFromJson } from "../../../shared/types/json";
import type {
  ApprovalProjection,
  FileChangeProjection,
  RunProjection,
  SubagentProjection,
  TaskProjection,
  TimelineProjectionItem,
  ToolCallProjection
} from "./run.types";
import { createEmptyRunProjection, type TimelineMessage } from "./run.types";
import type { RunEvent } from "../../../shared/contracts/platform";
import { parseToolResultViews } from "./tool-result-view.schema";

export function projectRunEvents(events: RunEvent[]): RunProjection {
  return mergeRunEvents([], events).reduce(
    (projection, event) => projectRunEvent(event, projection),
    createEmptyRunProjection()
  );
}

export function mergeRunEvents(existing: RunEvent[], incoming: RunEvent[]): RunEvent[] {
  const eventsByKey = new Map<string, RunEvent>();
  for (const event of [...existing, ...incoming]) {
    eventsByKey.set(runEventKey(event), event);
  }
  return [...eventsByKey.values()].sort((left, right) => left.seq - right.seq);
}

function runEventKey(event: RunEvent): string {
  return event.eventId || `${event.id}:${event.seq}`;
}

export function projectRunEvent(event: RunEvent, previous: RunProjection): RunProjection {
  const eventKey = event.eventId || `${event.id}:${event.seq}`;
  if (previous.seenEventKeys.has(eventKey)) {
    return previous;
  }

  const next: RunProjection = {
    ...previous,
    activeRunId: event.runId ?? previous.activeRunId,
    lastSeq: Math.max(previous.lastSeq, event.seq),
    seenEventKeys: new Set(previous.seenEventKeys).add(eventKey),
    runEvents: [...previous.runEvents, event],
    events: [...previous.events, event]
  };
  const payload = asRecord(event.payload);

  switch (event.type) {
    case "run.queued":
    case "run.started":
    case "run.failed":
    case "run.cancelled":
      return { ...next, status: statusFromEvent(event.type, payload) };
    case "run.completed":
      return {
        ...next,
        status: statusFromEvent(event.type, payload),
        messages: completeStreamingMessages(next.messages)
      };
    case "message.delta": {
      const result = upsertMessageDelta(next.messages, payload, previous, event);
      return {
        ...next,
        status: runningStatus(next.status),
        messages: result.messages,
        timeline: upsertTimelineItem(next.timeline, result.timelineItem)
      };
    }
    case "message.completed": {
      const result = completeMessage(next.messages, payload, previous, event);
      return {
        ...next,
        messages: result.messages,
        timeline: upsertTimelineItem(next.timeline, result.timelineItem)
      };
    }
    case "tool.call.started":
      return {
        ...next,
        toolCalls: upsertToolCall(next.toolCalls, payload, "running"),
        timeline: upsertTimelineItem(next.timeline, {
          kind: "tool_call",
          id: toolCallId(payload)
        })
      };
    case "tool.call.completed":
      return {
        ...next,
        toolCalls: upsertToolCall(next.toolCalls, payload, "completed"),
        timeline: upsertTimelineItem(next.timeline, {
          kind: "tool_call",
          id: toolCallId(payload)
        })
      };
    case "tool.call.failed":
      return {
        ...next,
        toolCalls: upsertToolCall(next.toolCalls, payload, "failed"),
        timeline: upsertTimelineItem(next.timeline, {
          kind: "tool_call",
          id: toolCallId(payload)
        })
      };
    case "task.created":
    case "task.updated":
    case "task.completed":
      return { ...next, tasks: upsertTask(next.tasks, payload, event.type) };
    case "subagent.started":
    case "subagent.completed":
      return { ...next, subagents: upsertSubagent(next.subagents, payload, event.type) };
    case "approval.requested":
      return {
        ...next,
        status: "waiting_approval",
        approvals: upsertApproval(next.approvals, payload, "pending")
      };
    case "approval.completed":
      return {
        ...next,
        status: "running",
        approvals: upsertApproval(next.approvals, payload, "completed")
      };
    case "file.changed":
      return { ...next, files: upsertFile(next.files, payload) };
    case "memory.candidate.created":
      return { ...next, memoryCandidateCount: next.memoryCandidateCount + 1 };
    default:
      return next;
  }
}

function statusFromEvent(type: string, payload: JsonRecord): string {
  return stringFromJson(payload.status, type.replace("run.", ""));
}

function runningStatus(status: string): string {
  return status === "idle" || status === "queued" ? "running" : status;
}

interface MessageProjectionResult {
  messages: TimelineMessage[];
  timelineItem?: TimelineProjectionItem;
}

function upsertMessageDelta(
  messages: TimelineMessage[],
  payload: JsonRecord,
  previous: RunProjection,
  event: RunEvent
): MessageProjectionResult {
  const id = messageId(payload, previous, event);
  const role = stringFromJson(payload.role, "assistant") as TimelineMessage["role"];
  const delta = stringFromJson(payload.delta, stringFromJson(payload.content));
  const existing = messages.find((message) => message.id === id);
  if (!existing) {
    if (!delta.trim()) {
      return { messages };
    }
    return {
      messages: [...messages, { id, role, content: delta, status: "streaming" }],
      timelineItem: { kind: "message", id }
    };
  }
  return {
    messages: messages.map((message) =>
      message.id === id ? { ...message, content: `${message.content}${delta}` } : message
    ),
    timelineItem: { kind: "message", id }
  };
}

function completeMessage(
  messages: TimelineMessage[],
  payload: JsonRecord,
  previous: RunProjection,
  event: RunEvent
): MessageProjectionResult {
  const id = messageId(payload, previous, event);
  const finalContent = stringFromJson(payload.content);
  const existing = messages.find((message) => message.id === id);
  if (!existing) {
    if (!finalContent.trim()) {
      return { messages };
    }
    return {
      messages: [
        ...messages,
        {
          id,
          role: stringFromJson(payload.role, "assistant") as TimelineMessage["role"],
          content: finalContent,
          status: "completed"
        }
      ],
      timelineItem: { kind: "message", id }
    };
  }
  return {
    messages: messages.map((message) =>
      message.id === id
        ? { ...message, content: finalContent || message.content, status: "completed" }
        : message
    ),
    timelineItem: { kind: "message", id }
  };
}

function completeStreamingMessages(messages: TimelineMessage[]): TimelineMessage[] {
  return messages.map((message) =>
    message.status === "streaming" ? { ...message, status: "completed" } : message
  );
}

function upsertToolCall(
  toolCalls: ToolCallProjection[],
  payload: JsonRecord,
  status: ToolCallProjection["status"]
): ToolCallProjection[] {
  const id = toolCallId(payload);
  const next: ToolCallProjection = {
    id,
    name: stringFromJson(payload.tool_name, stringFromJson(payload.name, "tool")),
    status,
    views: parseToolResultViews(payload.views)
  };
  const riskLevel = stringFromJson(payload.risk_level);
  const inputSummary = stringFromJson(payload.input_summary);
  const outputSummary = stringFromJson(payload.output_summary);
  const errorSummary = stringFromJson(payload.error_summary);
  if (riskLevel) {
    next.riskLevel = riskLevel;
  }
  if (inputSummary) {
    next.inputSummary = inputSummary;
  }
  if (outputSummary) {
    next.outputSummary = outputSummary;
  }
  if (errorSummary) {
    next.errorSummary = errorSummary;
  }
  return upsertById(toolCalls, next);
}

function upsertTask(
  tasks: TaskProjection[],
  payload: JsonRecord,
  eventType: string
): TaskProjection[] {
  const id = stringFromJson(payload.task_id, stringFromJson(payload.id, "task"));
  const next: TaskProjection = {
    id,
    title: stringFromJson(payload.title, "Task"),
    status: stringFromJson(payload.status, eventType.replace("task.", "")),
    summary: stringFromJson(payload.summary) || undefined
  };
  return upsertById(tasks, next);
}

function upsertSubagent(
  subagents: SubagentProjection[],
  payload: JsonRecord,
  eventType: string
): SubagentProjection[] {
  const id = stringFromJson(payload.subagent_id, stringFromJson(payload.id, "subagent"));
  const next: SubagentProjection = {
    id,
    name: stringFromJson(payload.name, "Subagent"),
    status: stringFromJson(payload.status, eventType.replace("subagent.", "")),
    summary: stringFromJson(payload.summary) || undefined
  };
  return upsertById(subagents, next);
}

function upsertApproval(
  approvals: ApprovalProjection[],
  payload: JsonRecord,
  status: string
): ApprovalProjection[] {
  const id = stringFromJson(payload.approval_id, stringFromJson(payload.id, "approval"));
  const next: ApprovalProjection = {
    id,
    status: stringFromJson(payload.status, status),
    riskLevel: stringFromJson(payload.risk_level) || undefined,
    toolName: stringFromJson(payload.tool_name) || undefined
  };
  return upsertById(approvals, next);
}

function upsertFile(files: FileChangeProjection[], payload: JsonRecord): FileChangeProjection[] {
  const path = stringFromJson(payload.path, "/workspace/unknown");
  const revision = numeric(payload.revision);
  const next: FileChangeProjection = {
    path,
    revision,
    contentHash: stringFromJson(payload.content_hash) || undefined,
    reason: stringFromJson(payload.reason) || undefined
  };
  return upsertBy(files, next, (file) => file.path);
}

function messageId(payload: JsonRecord, previous: RunProjection, event: RunEvent): string {
  const explicitId = stringFromJson(payload.message_id, stringFromJson(payload.id));
  if (explicitId) {
    return explicitId;
  }
  const role = stringFromJson(payload.role, "assistant");
  const runId = stringFromJson(payload.run_id, stringFromJson(payload.runId, event.runId));
  const baseId = runId ? `${role}.${runId}` : `${role}-message`;
  if (role !== "assistant") {
    return baseId;
  }
  const segment = implicitAssistantSegment(previous.runEvents, runId);
  return segment > 0 ? `${baseId}.${segment}` : baseId;
}

function implicitAssistantSegment(runEvents: RunEvent[], runId: string): number {
  const toolCallIds = new Set<string>();
  for (const event of runEvents) {
    if (!event.type.startsWith("tool.call.")) {
      continue;
    }
    const payload = asRecord(event.payload);
    const eventRunId = stringFromJson(payload.run_id, stringFromJson(payload.runId, event.runId));
    if (runId && eventRunId && eventRunId !== runId) {
      continue;
    }
    toolCallIds.add(toolCallId(payload, event));
  }
  return toolCallIds.size;
}

function toolCallId(payload: JsonRecord, event?: RunEvent): string {
  return stringFromJson(
    payload.tool_call_id,
    stringFromJson(payload.id, event?.eventId || "tool-call")
  );
}

function upsertTimelineItem(
  timeline: TimelineProjectionItem[],
  item?: TimelineProjectionItem
): TimelineProjectionItem[] {
  if (!item) {
    return timeline;
  }
  if (timeline.some((existing) => timelineItemKey(existing) === timelineItemKey(item))) {
    return timeline;
  }
  return [...timeline, item];
}

function timelineItemKey(item: TimelineProjectionItem): string {
  return `${item.kind}:${item.id}`;
}

function numeric(value: JsonValue | undefined): number | undefined {
  return typeof value === "number" ? value : undefined;
}

function upsertById<T extends { id: string }>(items: T[], item: T): T[] {
  return upsertBy(items, item, (value) => value.id);
}

function upsertBy<T>(items: T[], item: T, keyOf: (value: T) => string): T[] {
  const key = keyOf(item);
  if (!items.some((existing) => keyOf(existing) === key)) {
    return [...items, item];
  }
  return items.map((existing) => (keyOf(existing) === key ? { ...existing, ...item } : existing));
}
